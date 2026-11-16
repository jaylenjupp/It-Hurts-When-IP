// The helper's allow-list of networksetup commands and the one path every
// write takes. A `Write` can only be built from validated input, its argv is
// fixed here, and `apply` re-checks the service against the live list before
// running it. Nothing here builds a shell string.

use crate::error::{ErrCode, HelperError};
use crate::runner::{CmdOutput, CommandRunner};
use crate::validate::{self, Service};
use std::net::Ipv4Addr;
use std::sync::Mutex;

/// The only networksetup a release build ever runs.
pub const SYSTEM_NETWORKSETUP: &str = "/usr/sbin/networksetup";

const ERROR_PREFIX: &str = "** Error";

/// Serialises every write (list check + write) across all client threads.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

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
}

/// Run an allow-listed write: hold the global lock, check the service is an
/// enabled service on this Mac, then run it. Returns the success text.
pub fn apply(
    runner: &dyn CommandRunner,
    program: &str,
    write: &Write,
) -> Result<String, HelperError> {
    // A panic in another client thread must not stop all later writes.
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let services = list_services(runner, program)?;
    validate::check_service_enabled(write.service(), &services)?;
    run_write(runner, program, write)?;
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
/// service. Every write funnels through here.
fn run_write(runner: &dyn CommandRunner, program: &str, write: &Write) -> Result<(), HelperError> {
    // Package 2.2 hook: before the FIRST change to `write.service()`, journal
    // that service's original config here, and refuse the write if the
    // journal can't be written. The LaunchDaemon runs with Umask 077, so any
    // file created here must set its mode explicitly.
    let argv = write.argv();
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let result = match runner.run(program, &args) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::FakeRunner;

    const NS: &str = "/fake/networksetup";
    const SERVICES: &str =
        include_str!("../../tools/fake-networksetup/fixtures/default/services.txt");
    const UNKNOWN_STDOUT: &str = include_str!(
        "../../tools/fake-networksetup/fixtures/mac-studio/errors/getinfo-nonexistent.stdout.txt"
    );

    fn out(code: Option<i32>, stdout: &str, stderr: &str) -> CmdOutput {
        CmdOutput {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
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
    fn apply_lists_then_writes() {
        let fake = FakeRunner::new();
        fake.push_ok(SERVICES).push_ok("");
        let w = Write::set_dhcp("Wi-Fi").unwrap();
        assert_eq!(apply(&fake, NS, &w), Ok("Set Wi-Fi to DHCP".to_string()));
        assert_eq!(
            fake.calls(),
            vec![
                (NS.to_string(), strings(&["-listallnetworkservices"])),
                (NS.to_string(), strings(&["-setdhcp", "Wi-Fi"])),
            ]
        );
    }

    #[test]
    fn apply_reports_stdout_only_errors_as_failure() {
        for code in [Some(0), Some(4)] {
            let fake = FakeRunner::new();
            fake.push_ok(SERVICES).push(code, UNKNOWN_STDOUT, "");
            let w = Write::set_dhcp("Wi-Fi").unwrap();
            let err = apply(&fake, NS, &w).unwrap_err();
            assert_eq!(err.code, ErrCode::CommandFailed);
            assert!(err.message.contains("** Error"), "{}", err.message);
        }
    }

    #[test]
    fn apply_refuses_when_the_list_cannot_be_read() {
        let fake = FakeRunner::new();
        fake.push(Some(1), "", "boom");
        let w = Write::set_dhcp("Wi-Fi").unwrap();
        let err = apply(&fake, NS, &w).unwrap_err();
        assert_eq!(err.code, ErrCode::CommandFailed);
        assert!(err.message.contains("boom"));
        assert_eq!(fake.calls().len(), 1, "no write after a failed list");
    }

    #[test]
    fn apply_reports_spawn_errors() {
        let fake = FakeRunner::new();
        fake.push_ok(SERVICES); // nothing queued for the write: FakeRunner errors
        let w = Write::set_dhcp("Wi-Fi").unwrap();
        let err = apply(&fake, NS, &w).unwrap_err();
        assert_eq!(err.code, ErrCode::CommandFailed);
        assert!(
            err.message.starts_with("Could not run networksetup"),
            "{}",
            err.message
        );
    }
}
