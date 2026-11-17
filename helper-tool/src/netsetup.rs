// The helper's allow-list of networksetup commands and the one path every
// write takes. A `Write` can only be built from validated input, its argv is
// fixed here, and `apply` re-checks the service against the live list before
// running it. Before the first write to a service, `run_write` records the
// service's original settings in the journal and refuses the write if it
// can't. Nothing here builds a shell string.

use crate::error::{ErrCode, HelperError};
use crate::getinfo::{self, Mode, View};
use crate::journal::{self, Entry, State};
use crate::protocol::Ctx;
use crate::runner::{CmdOutput, CommandRunner};
use crate::validate::{self, Service};
use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard};

/// The only networksetup a release build ever runs.
pub const SYSTEM_NETWORKSETUP: &str = "/usr/sbin/networksetup";

const ERROR_PREFIX: &str = "** Error";

/// Serialises every write (list check + journal + write) across all client
/// threads.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn write_lock() -> MutexGuard<'static, ()> {
    // A panic in another client thread must not stop all later writes.
    WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// An allow-listed write with validated arguments. Build it with
/// `Write::set_manual` / `Write::set_dhcp`; the fields are private so no
/// unchecked value can reach the argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    SetManual {
        service: String,
        ip: Ipv4Addr,
        mask: Ipv4Addr,
        gateway: Option<Ipv4Addr>,
    },
    SetDhcp {
        service: String,
    },
}

impl Write {
    /// Validate everything that can be checked without running anything.
    /// The service is matched against the live list later, in `apply`.
    pub fn set_manual(
        service: &str,
        ip: &str,
        mask: &str,
        gateway: &str,
    ) -> Result<Self, HelperError> {
        validate::check_service_shape(service)?;
        let ip = validate::parse_host_ip(ip)?;
        let mask = validate::parse_subnet_mask(mask)?;
        let gateway = validate::parse_gateway(gateway)?;
        Ok(Write::SetManual {
            service: service.to_string(),
            ip,
            mask,
            gateway,
        })
    }

    pub fn set_dhcp(service: &str) -> Result<Self, HelperError> {
        validate::check_service_shape(service)?;
        Ok(Write::SetDhcp {
            service: service.to_string(),
        })
    }

    pub fn service(&self) -> &str {
        match self {
            Write::SetManual { service, .. } | Write::SetDhcp { service } => service,
        }
    }

    /// The exact argv passed to networksetup. With no gateway the 4th
    /// `-setmanual` argument stays `""`, exactly as v1.13.1 has always sent
    /// it (field-proven), so it is always five arguments.
    pub fn argv(&self) -> Vec<String> {
        match self {
            Write::SetManual {
                service,
                ip,
                mask,
                gateway,
            } => vec![
                "-setmanual".to_string(),
                service.clone(),
                ip.to_string(),
                mask.to_string(),
                gateway.map(|g| g.to_string()).unwrap_or_default(),
            ],
            Write::SetDhcp { service } => vec!["-setdhcp".to_string(), service.clone()],
        }
    }

    /// Success text, unchanged from v1.13.1.
    pub fn done_message(&self) -> String {
        match self {
            Write::SetManual { service, ip, .. } => format!("Set {} to {}", service, ip),
            Write::SetDhcp { service } => format!("Set {} to DHCP", service),
        }
    }

    /// The settings this write gives, for messages.
    fn describe(&self) -> String {
        match self {
            Write::SetDhcp { .. } => "DHCP".to_string(),
            Write::SetManual {
                ip, mask, gateway, ..
            } => match gateway {
                Some(g) => format!("Manual {}, mask {}, router {}", ip, mask, g),
                None => format!("Manual {}, mask {}, no router", ip, mask),
            },
        }
    }

    // Two comparisons, on purpose. Before a restore, `matches_exactly`
    // decides whether the write can be skipped: it must be strict, or a
    // router that wasn't in the original would be left in place. After the
    // write, `verifies` checks the read-back: it is lenient about the router
    // when none was recorded, because what macOS shows after `-setmanual …
    // ""` isn't settled (TODO(verify) on the travel Mac). Both follow the
    // rescue script: plan_restore's "unchanged" check and verify_network.

    /// Skip check (plan_restore's "unchanged"): DHCP = mode only; Manual =
    /// mode, IP, mask and router all equal, where no recorded router only
    /// matches no router now.
    fn matches_exactly(&self, now: &View) -> bool {
        match self {
            Write::SetDhcp { .. } => now.mode == Mode::Dhcp,
            Write::SetManual {
                ip, mask, gateway, ..
            } => {
                now.mode == Mode::Manual
                    && same_addr(&now.ip, ip)
                    && same_addr(&now.mask, mask)
                    && match gateway {
                        None => now.router.is_none(),
                        Some(g) => same_addr(&now.router, g),
                    }
            }
        }
    }

    /// Read-back check (verify_network): DHCP = mode only (not whether a
    /// lease has arrived); Manual = mode, IP and mask, plus the router only
    /// if one was recorded.
    fn verifies(&self, now: &View) -> bool {
        match self {
            Write::SetDhcp { .. } => now.mode == Mode::Dhcp,
            Write::SetManual {
                ip, mask, gateway, ..
            } => {
                now.mode == Mode::Manual
                    && same_addr(&now.ip, ip)
                    && same_addr(&now.mask, mask)
                    && match gateway {
                        None => true,
                        Some(g) => same_addr(&now.router, g),
                    }
            }
        }
    }
}

/// A -getinfo address (text, as the script compares it) equals `want`.
fn same_addr(have: &Option<String>, want: &Ipv4Addr) -> bool {
    have.as_deref() == Some(want.to_string().as_str())
}

/// Run an allow-listed write: hold the global lock, check the service is an
/// enabled service on this Mac, then run it. Returns the success text.
pub fn apply(ctx: &Ctx, write: &Write) -> Result<String, HelperError> {
    let _guard = write_lock();
    let services = list_services(ctx.runner, ctx.networksetup)?;
    validate::check_service_enabled(write.service(), &services)?;
    run_write(ctx, write)?;
    Ok(write.done_message())
}

/// Read-only: `networksetup -listallnetworkservices`, parsed.
pub fn list_services(
    runner: &dyn CommandRunner,
    program: &str,
) -> Result<Vec<Service>, HelperError> {
    let out = runner
        .run(program, &["-listallnetworkservices"])
        .map_err(spawn_error)?;
    if command_failed(&out) {
        return Err(HelperError::new(
            ErrCode::CommandFailed,
            format!("Could not list network services: {}", failure_text(&out)),
        ));
    }
    Ok(validate::parse_service_list(&out.stdout))
}

/// The ONLY function that runs a networksetup command which changes a
/// service. Every write funnels through here, and none runs unless the
/// service's original settings are in the journal. Call with the write lock
/// held.
fn run_write(ctx: &Ctx, write: &Write) -> Result<(), HelperError> {
    ensure_recorded(ctx, write.service())?;
    let argv = write.argv();
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let result = match ctx.runner.run(ctx.networksetup, &args) {
        Err(e) => Err(spawn_error(e)),
        Ok(out) if command_failed(&out) => {
            Err(HelperError::new(ErrCode::CommandFailed, failure_text(&out)))
        }
        Ok(_) => Ok(()),
    };
    // Debug formatting escapes newlines/quotes so a name can't forge log lines.
    match &result {
        Ok(()) => println!("{} {:?}: ok", argv[0], write.service()),
        Err(e) => eprintln!("{} {:?}: failed: {:?}", argv[0], write.service(), e.message),
    }
    result
}

/// Before the first change to `service`: read its original settings and add
/// them to the journal. A service that already has an entry is never read
/// again or overwritten. Fails closed: if anything goes wrong, nothing may be
/// changed. Call with the write lock held.
fn ensure_recorded(ctx: &Ctx, service: &str) -> Result<(), HelperError> {
    let mut entries = match journal::load(ctx.journal) {
        State::None => Vec::new(),
        State::Ok(entries) => entries,
        State::Unusable(why) => {
            return Err(journal_failed(&format!(
                "its journal of original settings can't be used: {}",
                why
            )))
        }
    };
    if entries.iter().any(|e| e.service == service) {
        return Ok(());
    }
    let read = |flag: &str| read_text(ctx, flag, service).map_err(|why| journal_failed(&why));
    let getinfo = read("-getinfo")?;
    let dns = read("-getdnsservers")?;
    let search = read("-getsearchdomains")?;
    entries.push(Entry {
        service: service.to_string(),
        recorded_at: journal::utc_stamp(ctx.clock.now_secs()),
        getinfo,
        dns,
        search,
    });
    journal::write(ctx.journal, &entries).map_err(|why| journal_failed(&why))?;
    println!("journal: recorded the original settings of {:?}", service);
    Ok(())
}

/// The refusal for item "record before first change" (owner decision P2-D4).
fn journal_failed(reason: &str) -> HelperError {
    HelperError::new(
        ErrCode::JournalFailed,
        format!(
            "Nothing was changed. The helper could not record this service's original \
             settings ({}). Reinstall It Hurts When IP from the latest .pkg, then try again.",
            reason
        ),
    )
}

/// Read-only `networksetup <flag> <service>`: its stdout, or a one-line
/// reason (same failure rule as every other command).
fn read_text(ctx: &Ctx, flag: &str, service: &str) -> Result<String, String> {
    match ctx.runner.run(ctx.networksetup, &[flag, service]) {
        Err(e) => Err(format!("could not run networksetup {}: {}", flag, e)),
        Ok(out) if command_failed(&out) => Err(format!(
            "networksetup {} failed: {}",
            flag,
            one_line(&failure_text(&out))
        )),
        Ok(out) => Ok(out.stdout),
    }
}

/// restore_service: put a journaled service back to its recorded original
/// IPv4 settings, then remove its entry so the next change records a fresh
/// original. IPv4 only: the recorded DNS servers and search domains stay in
/// the journal text but are not restored by this version.
///
/// Order: shape check, lock, journal entry, original parsed and validated
/// (nothing run yet), service listed and enabled, current settings read. If
/// they already match the original exactly (`matches_exactly`), nothing is
/// written. Otherwise one write through `run_write`, then one -getinfo to
/// verify (`verifies`). The entry is removed only once the service matches
/// its original.
pub fn restore(ctx: &Ctx, service: &str) -> Result<String, HelperError> {
    validate::check_service_shape(service)?;
    let _guard = write_lock();
    let entries = match journal::load(ctx.journal) {
        State::None => Vec::new(),
        State::Ok(entries) => entries,
        State::Unusable(why) => {
            return Err(HelperError::new(
                ErrCode::JournalUnusable,
                format!(
                    "Nothing was changed. The helper's journal of original settings can't be \
                     used ({}). Reinstall It Hurts When IP from the latest .pkg, then try again.",
                    why
                ),
            ))
        }
    };
    let entry = entries
        .iter()
        .find(|e| e.service == service)
        .ok_or_else(|| {
            HelperError::new(
                ErrCode::NotJournaled,
                format!(
                    "Nothing was changed. The helper has no record of the original settings \
                     of {:?}, so there is nothing to restore.",
                    service
                ),
            )
        })?;
    let original = original_write(entry)?;

    let services = list_services(ctx.runner, ctx.networksetup)?;
    validate::check_service_enabled(service, &services)?;
    let now = read_text(ctx, "-getinfo", service).map_err(|why| {
        HelperError::new(
            ErrCode::CommandFailed,
            format!(
                "Nothing was changed. Could not read the current settings of {:?}: {}",
                service, why
            ),
        )
    })?;
    if original.matches_exactly(&getinfo::view(&now)) {
        let warning = forget(ctx, &entries, service);
        return Ok(format!(
            "{} already matches its original settings ({}).{}",
            service,
            original.describe(),
            warning
        ));
    }

    run_write(ctx, &original)?;

    let after = read_text(ctx, "-getinfo", service).map_err(|why| {
        verify_failed(&original, &format!("they could not be read back: {}", why))
    })?;
    let after = getinfo::view(&after);
    if !original.verifies(&after) {
        return Err(verify_failed(
            &original,
            &format!("it now reads as {}", describe_view(&after)),
        ));
    }
    let warning = forget(ctx, &entries, service);
    Ok(format!(
        "Restored {} to its original settings ({}).{}",
        service,
        original.describe(),
        warning
    ))
}

/// The write that puts back a recorded original, read the way the rescue
/// script's plan_restore reads it. Recorded addresses go through the same
/// validators as client input; a missing router becomes the empty 4th
/// argument.
fn original_write(entry: &Entry) -> Result<Write, HelperError> {
    let unsupported = |why: &str| {
        HelperError::new(
            ErrCode::UnsupportedOriginal,
            format!(
                "Nothing was changed. The original settings recorded for {:?} can't be \
                 restored automatically. {}",
                entry.service, why
            ),
        )
    };
    let original = getinfo::view(&entry.getinfo);
    let write = match original.mode {
        Mode::Dhcp => Write::set_dhcp(&entry.service),
        Mode::Manual => Write::set_manual(
            &entry.service,
            original.ip.as_deref().unwrap_or(""),
            original.mask.as_deref().unwrap_or(""),
            original.router.as_deref().unwrap_or(""),
        ),
        Mode::Unknown => return Err(unsupported("They are neither DHCP nor Manual.")),
    };
    write.map_err(|e| unsupported(&e.message))
}

fn verify_failed(original: &Write, detail: &str) -> HelperError {
    HelperError::new(
        ErrCode::VerifyFailed,
        format!(
            "networksetup accepted the change, but {:?} does not read back as its original \
             settings ({}): {}. The record of its original settings was kept, so you can try \
             again.",
            original.service(),
            original.describe(),
            detail
        ),
    )
}

fn describe_view(v: &View) -> String {
    let addr = |a: &Option<String>| a.clone().unwrap_or_else(|| "none".to_string());
    match v.mode {
        Mode::Dhcp => "DHCP".to_string(),
        Mode::Manual => format!(
            "Manual {}, mask {}, router {}",
            addr(&v.ip),
            addr(&v.mask),
            addr(&v.router)
        ),
        Mode::Unknown => "neither DHCP nor Manual".to_string(),
    }
}

/// Remove `service`'s entry after a restore. A failure here doesn't undo the
/// restore: it comes back as a warning to append to the success text.
fn forget(ctx: &Ctx, entries: &[Entry], service: &str) -> String {
    let rest: Vec<Entry> = entries
        .iter()
        .filter(|e| e.service != service)
        .cloned()
        .collect();
    match journal::write(ctx.journal, &rest) {
        Ok(()) => {
            println!(
                "journal: removed the record of {:?} after restoring it",
                service
            );
            String::new()
        }
        Err(why) => {
            eprintln!(
                "journal: could not remove the record of {:?}: {}",
                service, why
            );
            format!(
                " Warning: the helper could not update its journal ({}), so its record of \
                 the original settings of {} was kept.",
                why, service
            )
        }
    }
}

fn spawn_error(e: std::io::Error) -> HelperError {
    HelperError::new(
        ErrCode::CommandFailed,
        format!("Could not run networksetup: {}", e),
    )
}

/// True if any line (ignoring leading spaces) starts with `** Error`.
fn has_error_line(text: &str) -> bool {
    text.lines()
        .any(|l| l.trim_start().starts_with(ERROR_PREFIX))
}

/// A command failed if it didn't exit 0 (including being killed by a
/// signal) or printed an `** Error` line on stdout or stderr. networksetup
/// reports some errors on stdout with exit 0, so the exit code alone is not
/// enough.
pub fn command_failed(out: &CmdOutput) -> bool {
    out.code != Some(0) || has_error_line(&out.stdout) || has_error_line(&out.stderr)
}

/// What to tell the client: trimmed stdout and stderr (both, when present),
/// or the exit status if networksetup printed nothing.
pub fn failure_text(out: &CmdOutput) -> String {
    let parts: Vec<&str> = [out.stdout.trim(), out.stderr.trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    if !parts.is_empty() {
        return parts.join("\n");
    }
    match out.code {
        Some(code) => format!("networksetup exited with code {}", code),
        None => "networksetup was stopped by a signal".to_string(),
    }
}

/// Multi-line text as one line, for a reason inside a sentence.
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::FakeRunner;
    use crate::testutil::{FixedClock, TempDir};
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::PathBuf;

    const NS: &str = "/fake/networksetup";
    const SERVICES: &str =
        include_str!("../../tools/fake-networksetup/fixtures/default/services.txt");
    const UNKNOWN_STDOUT: &str = include_str!(
        "../../tools/fake-networksetup/fixtures/mac-studio/errors/getinfo-nonexistent.stdout.txt"
    );
    /// 2026-11-16T09:30:00Z
    const T0: u64 = 1_794_821_400;

    const WIFI_DHCP: &str = "DHCP Configuration\nIP address: 10.20.30.40\n\
                             Subnet mask: 255.255.252.0\nRouter: 10.20.28.1\nClient ID: \n\
                             IPv6: Automatic\nWi-Fi ID: 66:77:88:99:aa:bb\n";
    const WIFI_MANUAL: &str = "Manual Configuration\nIP address: 10.9.8.7\n\
                               Subnet mask: 255.255.255.0\nRouter: 10.9.8.1\n";
    const DNS_NONE: &str = "There aren't any DNS Servers set on Wi-Fi.\n";
    const SEARCH_NONE: &str = "There aren't any Search Domains set on Wi-Fi.\n";
    const REFUSAL_START: &str =
        "Nothing was changed. The helper could not record this service's original settings (";
    const REFUSAL_END: &str = "). Reinstall It Hurts When IP from the latest .pkg, then try again.";

    /// A FakeRunner plus a journal path inside a temp dir.
    struct Env {
        dir: TempDir,
        journal: PathBuf,
        fake: FakeRunner,
        clock: FixedClock,
    }

    impl Env {
        fn new() -> Self {
            Self::at("journal.json")
        }

        /// Journal at `rel` inside the temp dir.
        fn at(rel: &str) -> Self {
            let dir = TempDir::new();
            let journal = dir.join(rel);
            Env {
                dir,
                journal,
                fake: FakeRunner::new(),
                clock: FixedClock(T0),
            }
        }

        fn ctx(&self) -> Ctx<'_> {
            Ctx {
                runner: &self.fake,
                networksetup: NS,
                journal: &self.journal,
                clock: &self.clock,
            }
        }

        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.fake.calls()
        }

        /// Queue the three journal reads (getinfo, dns, search).
        fn push_reads(&self, getinfo: &str) -> &FakeRunner {
            self.fake
                .push_ok(getinfo)
                .push_ok(DNS_NONE)
                .push_ok(SEARCH_NONE)
        }

        fn put_journal(&self, entries: &[Entry]) {
            journal::write(&self.journal, entries).unwrap();
        }

        fn journal_text(&self) -> Option<String> {
            fs::read_to_string(&self.journal).ok()
        }

        fn entries(&self) -> Vec<Entry> {
            match journal::load(&self.journal) {
                State::Ok(e) => e,
                other => panic!("journal not ok: {:?}", other),
            }
        }

        fn assert_no_set(&self, what: &str) {
            for (_, args) in self.calls() {
                assert!(
                    !args.first().is_some_and(|a| a.starts_with("-set")),
                    "{}: a write ran: {:?}",
                    what,
                    args
                );
            }
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn call(args: &[&str]) -> (String, Vec<String>) {
        (NS.to_string(), strings(args))
    }

    fn reads(svc: &str) -> Vec<(String, Vec<String>)> {
        vec![
            call(&["-getinfo", svc]),
            call(&["-getdnsservers", svc]),
            call(&["-getsearchdomains", svc]),
        ]
    }

    fn entry(svc: &str, getinfo: &str) -> Entry {
        Entry {
            service: svc.to_string(),
            recorded_at: "2026-11-01T10:00:00Z".to_string(),
            getinfo: getinfo.to_string(),
            dns: String::new(),
            search: String::new(),
        }
    }

    fn out(code: Option<i32>, stdout: &str, stderr: &str) -> CmdOutput {
        CmdOutput {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    /// Permission bits including setuid/setgid/sticky.
    fn mode(path: &std::path::Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0xFFF
    }

    #[test]
    fn failure_detection() {
        assert!(!command_failed(&out(Some(0), "", "")));
        assert!(!command_failed(&out(Some(0), SERVICES, "")));
        // stdout-only error, exit 0 and exit 4 (the real unknown-service text).
        assert!(command_failed(&out(Some(0), UNKNOWN_STDOUT, "")));
        assert!(command_failed(&out(Some(4), UNKNOWN_STDOUT, "")));
        assert!(command_failed(&out(Some(0), "", "** Error: nope\n")));
        assert!(command_failed(&out(
            Some(0),
            "ok\n  ** Error: indented\n",
            ""
        )));
        assert!(command_failed(&out(Some(1), "", "")));
        assert!(command_failed(&out(None, "", "")));
    }

    #[test]
    fn failure_text_uses_both_streams_then_exit_status() {
        assert_eq!(
            failure_text(&out(Some(4), UNKNOWN_STDOUT, "")),
            "No Such Service IHWIP is not a recognized network service.\n\
             ** Error: The parameters were not valid."
        );
        assert_eq!(
            failure_text(&out(Some(1), "  out \n", "\n err\n")),
            "out\nerr"
        );
        assert_eq!(
            failure_text(&out(Some(1), "", " \n")),
            "networksetup exited with code 1"
        );
        assert_eq!(
            failure_text(&out(None, "", "")),
            "networksetup was stopped by a signal"
        );
    }

    #[test]
    fn argv_is_fixed_and_keeps_empty_gateway() {
        let w = Write::set_manual("Dante Primary", "192.168.10.50", "255.255.255.0", "").unwrap();
        assert_eq!(
            w.argv(),
            strings(&[
                "-setmanual",
                "Dante Primary",
                "192.168.10.50",
                "255.255.255.0",
                ""
            ])
        );
        let w = Write::set_manual("Wi-Fi", "10.0.0.5", "255.255.252.0", "10.0.0.1").unwrap();
        assert_eq!(
            w.argv(),
            strings(&[
                "-setmanual",
                "Wi-Fi",
                "10.0.0.5",
                "255.255.252.0",
                "10.0.0.1"
            ])
        );
        assert_eq!(
            Write::set_dhcp("USB 10/100/1000 LAN").unwrap().argv(),
            strings(&["-setdhcp", "USB 10/100/1000 LAN"])
        );
    }

    #[test]
    fn apply_lists_records_then_writes() {
        let env = Env::new();
        env.fake.push_ok(SERVICES);
        env.push_reads(WIFI_DHCP).push_ok("");
        let w = Write::set_dhcp("Wi-Fi").unwrap();
        assert_eq!(apply(&env.ctx(), &w), Ok("Set Wi-Fi to DHCP".to_string()));
        let mut want = vec![call(&["-listallnetworkservices"])];
        want.extend(reads("Wi-Fi"));
        want.push(call(&["-setdhcp", "Wi-Fi"]));
        assert_eq!(env.calls(), want);
    }

    #[test]
    fn apply_reports_stdout_only_errors_as_failure() {
        for code in [Some(0), Some(4)] {
            let env = Env::new();
            env.fake.push_ok(SERVICES);
            env.push_reads(WIFI_DHCP).push(code, UNKNOWN_STDOUT, "");
            let w = Write::set_dhcp("Wi-Fi").unwrap();
            let err = apply(&env.ctx(), &w).unwrap_err();
            assert_eq!(err.code, ErrCode::CommandFailed);
            assert!(err.message.contains("** Error"), "{}", err.message);
            // The original was recorded before the write was tried.
            assert_eq!(env.entries().len(), 1);
        }
    }

    #[test]
    fn apply_refuses_when_the_list_cannot_be_read() {
        let env = Env::new();
        env.fake.push(Some(1), "", "boom");
        let w = Write::set_dhcp("Wi-Fi").unwrap();
        let err = apply(&env.ctx(), &w).unwrap_err();
        assert_eq!(err.code, ErrCode::CommandFailed);
        assert!(err.message.contains("boom"));
        assert_eq!(env.calls().len(), 1, "no write after a failed list");
        assert_eq!(env.journal_text(), None);
    }

    #[test]
    fn apply_reports_spawn_errors() {
        let env = Env::new();
        env.fake.push_ok(SERVICES);
        env.push_reads(WIFI_DHCP); // nothing queued for the write: FakeRunner errors
        let w = Write::set_dhcp("Wi-Fi").unwrap();
        let err = apply(&env.ctx(), &w).unwrap_err();
        assert_eq!(err.code, ErrCode::CommandFailed);
        assert!(
            err.message.starts_with("Could not run networksetup"),
            "{}",
            err.message
        );
    }

    // --- journal: record before the first change ---

    #[test]
    fn first_change_records_the_raw_original_then_writes() {
        let env = Env::at("com.ithurtswhenip.helper/journal.json");
        env.fake.push_ok(SERVICES);
        env.fake
            .push_ok(WIFI_DHCP)
            .push_ok("10.20.28.1\n1.1.1.1\n")
            .push_ok("show.example\n")
            .push_ok("");
        let w = Write::set_manual("Wi-Fi", "10.9.8.7", "255.255.255.0", "10.9.8.1").unwrap();
        assert_eq!(
            apply(&env.ctx(), &w),
            Ok("Set Wi-Fi to 10.9.8.7".to_string())
        );
        assert_eq!(
            env.calls(),
            vec![
                call(&["-listallnetworkservices"]),
                call(&["-getinfo", "Wi-Fi"]),
                call(&["-getdnsservers", "Wi-Fi"]),
                call(&["-getsearchdomains", "Wi-Fi"]),
                call(&[
                    "-setmanual",
                    "Wi-Fi",
                    "10.9.8.7",
                    "255.255.255.0",
                    "10.9.8.1"
                ]),
            ]
        );
        assert_eq!(
            env.entries(),
            vec![Entry {
                service: "Wi-Fi".to_string(),
                recorded_at: "2026-11-16T09:30:00Z".to_string(),
                getinfo: WIFI_DHCP.to_string(),
                dns: "10.20.28.1\n1.1.1.1\n".to_string(),
                search: "show.example\n".to_string(),
            }]
        );
        assert_eq!(mode(&env.journal), 0o644);
        assert_eq!(mode(env.journal.parent().unwrap()), 0o755);
    }

    #[test]
    fn later_changes_never_reread_or_overwrite_the_original() {
        let env = Env::new();
        env.fake.push_ok(SERVICES);
        env.push_reads(WIFI_DHCP).push_ok("");
        apply(&env.ctx(), &Write::set_dhcp("Wi-Fi").unwrap()).unwrap();
        let first = env.journal_text().unwrap();

        // Same service again: list + write only, journal byte-for-byte equal.
        let before = env.calls().len();
        env.fake.push_ok(SERVICES).push_ok("");
        let w = Write::set_manual("Wi-Fi", "10.1.1.1", "255.0.0.0", "").unwrap();
        apply(&env.ctx(), &w).unwrap();
        assert_eq!(
            env.calls()[before..].to_vec(),
            vec![
                call(&["-listallnetworkservices"]),
                call(&["-setmanual", "Wi-Fi", "10.1.1.1", "255.0.0.0", ""]),
            ]
        );
        assert_eq!(env.journal_text().unwrap(), first);

        // Another service is appended; the first entry stays as it was.
        env.fake.push_ok(SERVICES);
        env.push_reads("DHCP Configuration\n").push_ok("");
        apply(&env.ctx(), &Write::set_dhcp("Dante Primary").unwrap()).unwrap();
        let entries = env.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].getinfo, WIFI_DHCP);
        assert_eq!(entries[1].service, "Dante Primary");
    }

    fn assert_refused(env: &Env, err: &HelperError, reason_has: &str) {
        assert_eq!(err.code, ErrCode::JournalFailed, "{}", err.message);
        assert!(err.message.starts_with(REFUSAL_START), "{}", err.message);
        assert!(err.message.ends_with(REFUSAL_END), "{}", err.message);
        assert!(err.message.contains(reason_has), "{}", err.message);
        env.assert_no_set(&err.message);
    }

    #[test]
    fn a_failed_read_refuses_the_write() {
        type Queue = fn(&FakeRunner);
        let cases: [(Queue, &str); 5] = [
            // -getinfo: the real unknown-service text, stdout only, exit 4.
            (
                |f| {
                    f.push(Some(4), UNKNOWN_STDOUT, "");
                },
                "networksetup -getinfo failed: No Such Service IHWIP is not a recognized \
                 network service. ** Error: The parameters were not valid.",
            ),
            // -getinfo: ** Error on stdout with exit 0.
            (
                |f| {
                    f.push(Some(0), "** Error: The parameters were not valid.\n", "");
                },
                "networksetup -getinfo failed: ** Error",
            ),
            // -getdnsservers: exit 1 with stderr.
            (
                |f| {
                    f.push_ok(WIFI_DHCP).push(Some(1), "", "dns broke\n");
                },
                "networksetup -getdnsservers failed: dns broke",
            ),
            // -getsearchdomains: ** Error on stderr with exit 0.
            (
                |f| {
                    f.push_ok(WIFI_DHCP)
                        .push_ok(DNS_NONE)
                        .push(Some(0), "", "** Error: x");
                },
                "networksetup -getsearchdomains failed: ** Error: x",
            ),
            // -getsearchdomains could not be run at all.
            (
                |f| {
                    f.push_ok(WIFI_DHCP).push_ok(DNS_NONE);
                },
                "could not run networksetup -getsearchdomains",
            ),
        ];
        for (queue, reason) in cases {
            for v1 in [true, false] {
                let env = Env::new();
                env.fake.push_ok(SERVICES);
                queue(&env.fake);
                let w = if v1 {
                    Write::set_dhcp("Wi-Fi").unwrap()
                } else {
                    Write::set_manual("Wi-Fi", "10.0.0.5", "255.0.0.0", "").unwrap()
                };
                let err = apply(&env.ctx(), &w).unwrap_err();
                assert_refused(&env, &err, reason);
                assert_eq!(env.journal_text(), None, "nothing journaled");
            }
        }
    }

    #[test]
    fn an_unusable_journal_refuses_the_write_and_is_left_alone() {
        let cases: [(&str, &str); 3] = [
            (
                "{\"schema\":1,\"services\":[{\"service\":\"Da",
                "not valid JSON",
            ),
            ("{\"schema\":2,\"services\":[]}", "unknown schema 2"),
            (
                "{\"schema\":1,\"services\":[{\"service\":\"Wi-Fi\",\"getinfo\":\"\"},\
                 {\"service\":\"Wi-Fi\",\"getinfo\":\"\"}]}",
                "listed twice",
            ),
        ];
        for (text, reason) in cases {
            let env = Env::new();
            fs::write(&env.journal, text).unwrap();
            env.fake.push_ok(SERVICES);
            let err = apply(&env.ctx(), &Write::set_dhcp("Wi-Fi").unwrap()).unwrap_err();
            assert_refused(&env, &err, "its journal of original settings can't be used");
            assert!(err.message.contains(reason), "{}", err.message);
            assert_eq!(env.calls(), vec![call(&["-listallnetworkservices"])]);
            assert_eq!(env.journal_text().unwrap(), text);
        }

        // A symlink where the journal goes.
        let env = Env::new();
        let target = env.dir.join("elsewhere.json");
        fs::write(&target, "{\"schema\":1,\"services\":[]}").unwrap();
        std::os::unix::fs::symlink(&target, &env.journal).unwrap();
        env.fake.push_ok(SERVICES);
        let err = apply(&env.ctx(), &Write::set_dhcp("Wi-Fi").unwrap()).unwrap_err();
        assert_refused(&env, &err, "not a regular file");
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "{\"schema\":1,\"services\":[]}"
        );
    }

    #[test]
    fn an_unwritable_journal_refuses_the_write() {
        // The journal's folder is a file.
        let env = Env::at("folder/journal.json");
        fs::write(env.dir.join("folder"), "not a folder").unwrap();
        env.fake.push_ok(SERVICES);
        let err = apply(&env.ctx(), &Write::set_dhcp("Wi-Fi").unwrap()).unwrap_err();
        assert_refused(&env, &err, "can't be used: cannot be read");
        assert_eq!(env.calls(), vec![call(&["-listallnetworkservices"])]);

        // The journal's folder is read-only: the reads run, the write doesn't.
        let env = Env::at("ro/journal.json");
        let ro = env.dir.join("ro");
        fs::create_dir(&ro).unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
        env.fake.push_ok(SERVICES);
        env.push_reads(WIFI_DHCP).push_ok("");
        let err = apply(&env.ctx(), &Write::set_dhcp("Wi-Fi").unwrap()).unwrap_err();
        assert_refused(&env, &err, "could not create");
        let mut want = vec![call(&["-listallnetworkservices"])];
        want.extend(reads("Wi-Fi"));
        assert_eq!(env.calls(), want);
        assert_eq!(fs::read_dir(&ro).unwrap().count(), 0);

        // The journal's parent folder is missing.
        let env = Env::at("a/b/journal.json");
        env.fake.push_ok(SERVICES);
        env.push_reads(WIFI_DHCP).push_ok("");
        let err = apply(&env.ctx(), &Write::set_dhcp("Wi-Fi").unwrap()).unwrap_err();
        assert_refused(&env, &err, "could not create");
    }

    #[test]
    fn a_journal_that_would_pass_64_kb_refuses_the_write() {
        let env = Env::new();
        // One big entry that leaves less room than the next entry needs.
        let base = journal::to_json(&[entry("Dante Primary", "")]).len();
        let big = vec![entry(
            "Dante Primary",
            &"x".repeat(journal::MAX_BYTES - base - 100),
        )];
        env.put_journal(&big);
        let before = env.journal_text().unwrap();
        env.fake.push_ok(SERVICES);
        env.push_reads(WIFI_DHCP).push_ok("");
        let err = apply(&env.ctx(), &Write::set_dhcp("Wi-Fi").unwrap()).unwrap_err();
        assert_refused(&env, &err, "the journal would be larger than 64 KB");
        assert_eq!(env.journal_text().unwrap(), before);
    }

    /// The rescue script's tests read this file (test 23b), so it pins the
    /// journal format both sides rely on.
    #[test]
    fn recorded_journal_matches_the_shared_fixture() {
        const FIXTURE: &str =
            include_str!("../../recovery/tests/fixtures/journal-from-helper.json");
        let services = "An asterisk (*) denotes that a network service is disabled.\n\
                        Dante Primary\nShow \"A\", B\nCaf\u{e9} Ethernet\nWi-Fi\n\
                        USB 10/100/1000 LAN\n";
        let originals: [(&str, &str, &str, &str); 5] = [
            (
                "Dante Primary",
                "Manual Configuration\nIP address: 192.168.10.21\nSubnet mask: 255.255.255.0\n\
                 Router: \nIPv6: Automatic\nIPv6 IP address: none\nIPv6 Router: none\n\
                 Ethernet Address: 00:11:22:33:44:66\n",
                "There aren't any DNS Servers set on Dante Primary.\n",
                "There aren't any Search Domains set on Dante Primary.\n",
            ),
            (
                "Show \"A\", B",
                "DHCP Configuration\nIP address: 192.168.50.7\nSubnet mask: 255.255.255.0\n\
                 Router: 192.168.50.1\nClient ID: \nIPv6: Automatic\nIPv6 IP address: none\n\
                 IPv6 Router: none\nEthernet Address: 00:11:22:33:44:88\n",
                "192.168.50.1\n1.1.1.1\n",
                "show.example\n",
            ),
            (
                "Caf\u{e9} Ethernet",
                "Manual Configuration\nIP address: 10.50.0.5\nSubnet mask: 255.255.0.0\n\
                 Router: 10.50.0.1\nIPv6: Automatic\nIPv6 IP address: none\nIPv6 Router: none\n\
                 Ethernet Address: 00:11:22:33:44:99\n",
                "10.50.0.1\n",
                "There aren't any Search Domains set on Caf\u{e9} Ethernet.\n",
            ),
            (
                "Wi-Fi",
                "DHCP Configuration\nIP address: 10.20.30.40\nSubnet mask: 255.255.252.0\n\
                 Router: 10.20.28.1\nClient ID: \nIPv6: Automatic\nIPv6 IP address: none\n\
                 IPv6 Router: none\nWi-Fi ID: 66:77:88:99:aa:bb\n",
                "There aren't any DNS Servers set on Wi-Fi.\n",
                "There aren't any Search Domains set on Wi-Fi.\n",
            ),
            (
                "USB 10/100/1000 LAN",
                "DHCP Configuration\nIP address: 192.168.1.42\nSubnet mask: 255.255.255.0\n\
                 Router: 192.168.1.1\nClient ID: \nIPv6: Automatic\nIPv6 IP address: none\n\
                 IPv6 Router: none\nEthernet Address: 00:11:22:33:44:55\n",
                "There aren't any DNS Servers set on USB 10/100/1000 LAN.\n",
                "There aren't any Search Domains set on USB 10/100/1000 LAN.\n",
            ),
        ];
        let env = Env::new();
        for (i, (svc, getinfo, dns, search)) in originals.iter().enumerate() {
            env.fake
                .push_ok(services)
                .push_ok(getinfo)
                .push_ok(dns)
                .push_ok(search)
                .push_ok("");
            // One minute apart.
            let clock = FixedClock(T0 + 60 * i as u64);
            let ctx = Ctx {
                clock: &clock,
                ..env.ctx()
            };
            apply(&ctx, &Write::set_dhcp(svc).unwrap()).unwrap();
        }
        assert_eq!(env.journal_text().unwrap(), FIXTURE);
        // And it reads back as exactly what was recorded.
        let entries = env.entries();
        for (e, (svc, getinfo, dns, search)) in entries.iter().zip(originals.iter()) {
            assert_eq!(
                (&e.service[..], &e.getinfo[..], &e.dns[..], &e.search[..]),
                (*svc, *getinfo, *dns, *search)
            );
        }
    }

    // --- restore ---

    const DANTE_NO_ROUTER: &str = "Manual Configuration\nIP address: 192.168.10.21\n\
                                   Subnet mask: 255.255.255.0\nRouter: \nIPv6: Automatic\n";
    const DANTE_DHCP_NOW: &str = "DHCP Configuration\nIP address: 169.254.99.99\n\
                                  Subnet mask: 255.255.0.0\nRouter:\n";

    #[test]
    fn restore_a_dhcp_original() {
        let env = Env::new();
        env.put_journal(&[
            entry("Dante Primary", DANTE_NO_ROUTER),
            entry("Wi-Fi", WIFI_DHCP),
        ]);
        env.fake
            .push_ok(SERVICES)
            .push_ok(WIFI_MANUAL)
            .push_ok("")
            .push_ok(WIFI_DHCP);
        assert_eq!(
            restore(&env.ctx(), "Wi-Fi"),
            Ok("Restored Wi-Fi to its original settings (DHCP).".to_string())
        );
        assert_eq!(
            env.calls(),
            vec![
                call(&["-listallnetworkservices"]),
                call(&["-getinfo", "Wi-Fi"]),
                call(&["-setdhcp", "Wi-Fi"]),
                call(&["-getinfo", "Wi-Fi"]),
            ]
        );
        // The Wi-Fi entry is gone, the other one is unchanged.
        assert_eq!(env.entries(), vec![entry("Dante Primary", DANTE_NO_ROUTER)]);
        assert_eq!(mode(&env.journal), 0o644);
    }

    #[test]
    fn restore_a_manual_original_with_and_without_router() {
        // With a router: exact five-argument argv.
        let env = Env::new();
        env.put_journal(&[entry("Wi-Fi", WIFI_MANUAL)]);
        env.fake
            .push_ok(SERVICES)
            .push_ok(WIFI_DHCP)
            .push_ok("")
            .push_ok(WIFI_MANUAL);
        assert_eq!(
            restore(&env.ctx(), "Wi-Fi"),
            Ok("Restored Wi-Fi to its original settings \
                (Manual 10.9.8.7, mask 255.255.255.0, router 10.9.8.1)."
                .to_string())
        );
        assert_eq!(
            env.calls()[2],
            call(&[
                "-setmanual",
                "Wi-Fi",
                "10.9.8.7",
                "255.255.255.0",
                "10.9.8.1"
            ])
        );
        assert_eq!(env.entries(), vec![]);
        assert_eq!(
            env.journal_text().unwrap(),
            "{\"schema\":1,\"services\":[]}\n"
        );

        // Without a router ("Router: " empty): the 4th argument is "".
        let env = Env::new();
        env.put_journal(&[entry("Dante Primary", DANTE_NO_ROUTER)]);
        env.fake
            .push_ok(SERVICES)
            .push_ok(DANTE_DHCP_NOW)
            .push_ok("")
            .push_ok(DANTE_NO_ROUTER);
        assert_eq!(
            restore(&env.ctx(), "Dante Primary"),
            Ok("Restored Dante Primary to its original settings \
                (Manual 192.168.10.21, mask 255.255.255.0, no router)."
                .to_string())
        );
        assert_eq!(
            env.calls(),
            vec![
                call(&["-listallnetworkservices"]),
                call(&["-getinfo", "Dante Primary"]),
                call(&[
                    "-setmanual",
                    "Dante Primary",
                    "192.168.10.21",
                    "255.255.255.0",
                    ""
                ]),
                call(&["-getinfo", "Dante Primary"]),
            ]
        );
        assert_eq!(env.entries(), vec![]);
    }

    #[test]
    fn restore_skips_the_write_when_the_service_already_matches() {
        for (original, now) in [
            (WIFI_DHCP, "DHCP Configuration\nIP address: 10.1.1.1\n"),
            (WIFI_MANUAL, WIFI_MANUAL),
            // No router recorded and none now ("Router: " vs missing line).
            (
                "Manual Configuration\nIP address: 10.9.8.7\nSubnet mask: 255.255.255.0\n",
                "Manual Configuration\nIP address: 10.9.8.7\nSubnet mask: 255.255.255.0\n\
                 Router: \n",
            ),
        ] {
            let env = Env::new();
            env.put_journal(&[
                entry("Wi-Fi", original),
                entry("Dante Primary", DANTE_NO_ROUTER),
            ]);
            env.fake.push_ok(SERVICES).push_ok(now);
            let text = restore(&env.ctx(), "Wi-Fi").unwrap();
            assert!(
                text.starts_with("Wi-Fi already matches its original settings ("),
                "{}",
                text
            );
            assert_eq!(
                env.calls(),
                vec![
                    call(&["-listallnetworkservices"]),
                    call(&["-getinfo", "Wi-Fi"])
                ]
            );
            assert_eq!(env.entries(), vec![entry("Dante Primary", DANTE_NO_ROUTER)]);
        }
    }

    /// The skip check compares the router exactly (as the rescue script's
    /// plan does); only the read-back after the write is lenient about it.
    #[test]
    fn restore_writes_when_only_the_router_differs() {
        const DANTE_WITH_ROUTER: &str = "Manual Configuration\nIP address: 192.168.10.21\n\
                                         Subnet mask: 255.255.255.0\nRouter: 192.168.10.1\n";

        // Original has no router, now one is set: -setmanual … "" runs. The
        // fake's read-back still shows a router; verify accepts that because
        // none was recorded.
        let env = Env::new();
        env.put_journal(&[entry("Dante Primary", DANTE_NO_ROUTER)]);
        env.fake
            .push_ok(SERVICES)
            .push_ok(DANTE_WITH_ROUTER)
            .push_ok("")
            .push_ok(DANTE_WITH_ROUTER);
        assert_eq!(
            restore(&env.ctx(), "Dante Primary"),
            Ok("Restored Dante Primary to its original settings \
                (Manual 192.168.10.21, mask 255.255.255.0, no router)."
                .to_string())
        );
        assert_eq!(
            env.calls(),
            vec![
                call(&["-listallnetworkservices"]),
                call(&["-getinfo", "Dante Primary"]),
                call(&[
                    "-setmanual",
                    "Dante Primary",
                    "192.168.10.21",
                    "255.255.255.0",
                    ""
                ]),
                call(&["-getinfo", "Dante Primary"]),
            ]
        );
        assert_eq!(env.entries(), vec![]);

        // Original has a router, now there is none: -setmanual with the router.
        let env = Env::new();
        env.put_journal(&[entry("Dante Primary", DANTE_WITH_ROUTER)]);
        env.fake
            .push_ok(SERVICES)
            .push_ok(DANTE_NO_ROUTER)
            .push_ok("")
            .push_ok(DANTE_WITH_ROUTER);
        assert_eq!(
            restore(&env.ctx(), "Dante Primary"),
            Ok("Restored Dante Primary to its original settings \
                (Manual 192.168.10.21, mask 255.255.255.0, router 192.168.10.1)."
                .to_string())
        );
        assert_eq!(
            env.calls()[2],
            call(&[
                "-setmanual",
                "Dante Primary",
                "192.168.10.21",
                "255.255.255.0",
                "192.168.10.1"
            ])
        );
        assert_eq!(env.entries(), vec![]);
    }

    #[test]
    fn restore_needs_a_journal_entry() {
        // No journal at all, and a journal without this service.
        for others in [None, Some(vec![entry("Dante Primary", DANTE_NO_ROUTER)])] {
            let env = Env::new();
            if let Some(o) = &others {
                env.put_journal(o);
            }
            let before = env.journal_text();
            let err = restore(&env.ctx(), "Wi-Fi").unwrap_err();
            assert_eq!(err.code, ErrCode::NotJournaled);
            assert!(
                err.message.starts_with("Nothing was changed."),
                "{}",
                err.message
            );
            assert!(env.calls().is_empty());
            assert_eq!(env.journal_text(), before);
        }
    }

    #[test]
    fn restore_refuses_an_unusable_journal_or_a_bad_name() {
        let env = Env::new();
        fs::write(&env.journal, "{\"schema\":2,\"services\":[]}").unwrap();
        let err = restore(&env.ctx(), "Wi-Fi").unwrap_err();
        assert_eq!(err.code, ErrCode::JournalUnusable);
        assert!(err.message.contains("unknown schema 2"), "{}", err.message);
        assert!(err.message.contains("Reinstall"), "{}", err.message);
        assert!(env.calls().is_empty());

        let env = Env::new();
        let err = restore(&env.ctx(), "Wi-Fi\nreboot").unwrap_err();
        assert_eq!(err.code, ErrCode::InvalidService);
        assert!(env.calls().is_empty());
    }

    #[test]
    fn restore_refuses_an_unsupported_or_invalid_original_without_running_anything() {
        for (getinfo, says) in [
            (
                "BOOTP Configuration\nIP address: 10.0.0.5\n",
                "neither DHCP nor Manual",
            ),
            ("", "neither DHCP nor Manual"),
            (
                "Manually Using DHCP Router Configuration\nIP address: 10.0.0.5\n",
                "neither DHCP nor Manual",
            ),
            (
                "Manual Configuration\nIP address: 127.0.0.1\nSubnet mask: 255.0.0.0\n",
                "loopback",
            ),
            (
                "Manual Configuration\nIP address: 10.0.0.5; rm -rf /\nSubnet mask: 255.0.0.0\n",
                "not a valid IPv4 address",
            ),
            (
                "Manual Configuration\nIP address: 010.0.0.8\nSubnet mask: 255.255.255.0\n",
                "not a valid IPv4 address",
            ),
            (
                "Manual Configuration\nIP address: 10.0.0.6\nSubnet mask: 255.0.255.0\n",
                "Subnet mask",
            ),
            (
                "Manual Configuration\nIP address: 10.0.0.6\n",
                "Subnet mask",
            ),
            (
                "Manual Configuration\nIP address: 10.0.0.7\nSubnet mask: 255.255.255.0\n\
                 Router: 10.0.0.1 -x\n",
                "Gateway",
            ),
        ] {
            let env = Env::new();
            env.put_journal(&[entry("Wi-Fi", getinfo)]);
            let before = env.journal_text();
            let err = restore(&env.ctx(), "Wi-Fi").unwrap_err();
            assert_eq!(err.code, ErrCode::UnsupportedOriginal, "{:?}", getinfo);
            assert!(
                err.message.starts_with(
                    "Nothing was changed. The original settings recorded for \"Wi-Fi\" can't \
                     be restored automatically."
                ),
                "{}",
                err.message
            );
            assert!(err.message.contains(says), "{}", err.message);
            assert!(env.calls().is_empty(), "{:?}", getinfo);
            assert_eq!(env.journal_text(), before);
        }
    }

    #[test]
    fn restore_checks_the_service_is_listed_and_enabled() {
        for (svc, code) in [
            ("Old Adapter", ErrCode::ServiceDisabled),
            ("Gone Adapter", ErrCode::UnknownService),
        ] {
            let env = Env::new();
            env.put_journal(&[entry(svc, WIFI_DHCP)]);
            env.fake.push_ok(SERVICES);
            let err = restore(&env.ctx(), svc).unwrap_err();
            assert_eq!(err.code, code);
            assert_eq!(env.calls(), vec![call(&["-listallnetworkservices"])]);
            assert_eq!(env.entries().len(), 1);
        }
    }

    #[test]
    fn restore_verify_failures_keep_the_entry() {
        // The write "worked" but the service still reads as Manual.
        let env = Env::new();
        env.put_journal(&[entry("Wi-Fi", WIFI_DHCP)]);
        let before = env.journal_text();
        env.fake
            .push_ok(SERVICES)
            .push_ok(WIFI_MANUAL)
            .push_ok("")
            .push_ok(WIFI_MANUAL);
        let err = restore(&env.ctx(), "Wi-Fi").unwrap_err();
        assert_eq!(err.code, ErrCode::VerifyFailed);
        assert!(
            err.message
                .contains("it now reads as Manual 10.9.8.7, mask 255.255.255.0, router 10.9.8.1"),
            "{}",
            err.message
        );
        assert!(err.message.contains("was kept"), "{}", err.message);
        assert_eq!(env.calls().len(), 4);
        assert_eq!(env.journal_text(), before);

        // Manual: the IP didn't take.
        let env = Env::new();
        env.put_journal(&[entry("Wi-Fi", WIFI_MANUAL)]);
        env.fake
            .push_ok(SERVICES)
            .push_ok(WIFI_DHCP)
            .push_ok("")
            .push_ok("Manual Configuration\nIP address: 10.9.8.8\nSubnet mask: 255.255.255.0\nRouter: 10.9.8.1\n");
        let err = restore(&env.ctx(), "Wi-Fi").unwrap_err();
        assert_eq!(err.code, ErrCode::VerifyFailed);
        assert_eq!(env.entries().len(), 1);

        // Manual with a recorded router: the router didn't take.
        let env = Env::new();
        env.put_journal(&[entry("Wi-Fi", WIFI_MANUAL)]);
        env.fake
            .push_ok(SERVICES)
            .push_ok(WIFI_DHCP)
            .push_ok("")
            .push_ok(
            "Manual Configuration\nIP address: 10.9.8.7\nSubnet mask: 255.255.255.0\nRouter: \n",
        );
        let err = restore(&env.ctx(), "Wi-Fi").unwrap_err();
        assert_eq!(err.code, ErrCode::VerifyFailed);
        assert_eq!(env.entries().len(), 1);

        // The read-back fails.
        let env = Env::new();
        env.put_journal(&[entry("Wi-Fi", WIFI_DHCP)]);
        env.fake
            .push_ok(SERVICES)
            .push_ok(WIFI_MANUAL)
            .push_ok("")
            .push(Some(4), UNKNOWN_STDOUT, "");
        let err = restore(&env.ctx(), "Wi-Fi").unwrap_err();
        assert_eq!(err.code, ErrCode::VerifyFailed);
        assert!(
            err.message.contains("could not be read back"),
            "{}",
            err.message
        );
        assert_eq!(env.entries().len(), 1);
    }

    #[test]
    fn restore_write_or_first_read_failures_keep_the_entry() {
        // The write fails.
        let env = Env::new();
        env.put_journal(&[entry("Wi-Fi", WIFI_DHCP)]);
        env.fake.push_ok(SERVICES).push_ok(WIFI_MANUAL).push(
            Some(1),
            "** Error: The parameters were not valid.\n",
            "",
        );
        let err = restore(&env.ctx(), "Wi-Fi").unwrap_err();
        assert_eq!(err.code, ErrCode::CommandFailed);
        assert_eq!(env.calls().len(), 3);
        assert_eq!(env.entries().len(), 1);

        // The current settings can't be read: nothing is written.
        let env = Env::new();
        env.put_journal(&[entry("Wi-Fi", WIFI_DHCP)]);
        env.fake.push_ok(SERVICES).push(Some(4), UNKNOWN_STDOUT, "");
        let err = restore(&env.ctx(), "Wi-Fi").unwrap_err();
        assert_eq!(err.code, ErrCode::CommandFailed);
        assert!(
            err.message
                .starts_with("Nothing was changed. Could not read the current settings"),
            "{}",
            err.message
        );
        env.assert_no_set("unreadable current settings");
        assert_eq!(env.entries().len(), 1);
    }

    #[test]
    fn restore_reports_success_with_a_warning_if_the_journal_cannot_be_updated() {
        let env = Env::at("ro/journal.json");
        env.put_journal(&[entry("Wi-Fi", WIFI_DHCP)]);
        let ro = env.dir.join("ro");
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
        env.fake
            .push_ok(SERVICES)
            .push_ok(WIFI_MANUAL)
            .push_ok("")
            .push_ok(WIFI_DHCP);
        let text = restore(&env.ctx(), "Wi-Fi").unwrap();
        assert!(
            text.starts_with(
                "Restored Wi-Fi to its original settings (DHCP). Warning: the helper could not \
                 update its journal (could not create"
            ),
            "{}",
            text
        );
        assert!(
            text.ends_with("so its record of the original settings of Wi-Fi was kept."),
            "{}",
            text
        );
        assert_eq!(env.calls()[2], call(&["-setdhcp", "Wi-Fi"]));
        assert_eq!(env.entries(), vec![entry("Wi-Fi", WIFI_DHCP)]);
    }
}
