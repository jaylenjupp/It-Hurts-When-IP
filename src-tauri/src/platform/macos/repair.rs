// In-app "Repair network…" through the bundled rescue script
// (recovery/ithurtswhenip-rescue.command, --repair soft|service:<name>|full).
// macOS only.
//
// Same pattern as the uninstall in `rescue.rs`, and never the helper (P3-D4):
//  - `repair_plan` runs `/bin/bash <script> --plan --repair <tier>` (no
//    privileges, read-only) and parses the records with rescue.rs's parser.
//  - `repair_run` checks the tier, refuses a full reset unless the UI passed
//    the typed word RESET, makes a fresh plan and rejects a tier/service that
//    is not in it, then runs the script once as root through /usr/bin/osascript
//    and rescue.rs's one fixed AppleScript (argv only; nothing is put into the
//    AppleScript source and no shell string is built in Rust).
//  - Debug builds never run osascript: they return the argv (decided in Rust
//    from cfg!(debug_assertions)).
//  - The app never restarts the Mac (P3-D3): it only reports `RESTART needed`.
//
// Every program runs through a CommandRunner, so the tests use FakeRunner.

use serde::Serialize;

use super::rescue::{
    detail_value, map_outcome, parse_run_output, run_script_plan, script_location,
    script_path_text, terminal_command, Outcome, PlanRecord, PlanRun, PlanText, RecordKind,
    StepLine, StepStatus, APPLESCRIPT, EXECUTE_OSASCRIPT, OSASCRIPT,
};
use crate::platform::runner::{CmdOutput, CommandRunner, SystemRunner};

/// The word the user must type in the app before a full reset can run.
const FULL_RESET_WORD: &str = "RESET";

// --- Types the UI receives ---

/// One service in a soft-refresh plan (`REPAIR renew|leave <service>`), also
/// the list the "Reset one service" picker offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairService {
    pub name: String,
    /// "renew" or "leave".
    pub action: String,
    pub detail: String,
    /// The script's service reset could take it (enabled, readable, DHCP or
    /// Manual). The service plan still has the final say.
    pub resettable: bool,
}

/// `REPAIR current <service> now=<view> dns=<a,b|-> search=<a,b|->`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceSettings {
    /// `<mode>/<ip>/<mask>/<router>` as the script prints it.
    pub now: String,
    pub dns: Vec<String>,
    pub search: Vec<String>,
}

/// What `repair_plan` returns. When the script refused (exit 2), `refused`
/// holds its message and the lists are empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairPlan {
    /// "soft", "service" or "full".
    pub tier: String,
    pub service: Option<String>,
    pub script_path: String,
    /// Display only, for the user to copy into Terminal. Never executed.
    pub terminal_command: String,
    pub refused: Option<String>,
    pub records: Vec<PlanRecord>,
    pub summary: Option<String>,
    pub backup_folder: Option<String>,
    /// `rescue: note:` lines from stderr.
    pub notices: Vec<String>,
    /// Soft: every service with what happens to it.
    pub services: Vec<RepairService>,
    /// Service: its current settings (all of which the reset clears).
    pub current: Option<ServiceSettings>,
    /// Full: the SystemConfiguration files that will be moved to the backup.
    pub moves: Vec<String>,
    /// NOTE lines (e.g. the full reset's three consequences).
    pub notes: Vec<String>,
}

/// What `repair_run` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairRun {
    pub outcome: Outcome,
    pub executed: bool,
    /// Program first, then every argument, exactly as given to the runner.
    pub argv: Vec<String>,
    pub exit_code: Option<i32>,
    /// OK / FAILED / VERIFY ok|warn|FAILED lines, in the order printed.
    pub steps: Vec<StepLine>,
    /// Number of `VERIFY warn` steps.
    pub warnings: usize,
    pub backup_folder: Option<String>,
    pub log_path: Option<String>,
    /// The script reported `RESTART needed` (files were changed by a full
    /// reset). The app never restarts the Mac itself.
    pub restart_needed: bool,
    pub restart_text: Option<String>,
    pub messages: Vec<String>,
    /// Display only: the same repair from Terminal. Never executed.
    pub terminal_command: String,
}

// --- Public API (re-exported through platform::) ---

/// Runs the rescue script's `--plan --repair <tier>` (read-only, no privileges).
pub fn repair_plan(tier: String, service: Option<String>) -> Result<RepairPlan, String> {
    let tier = parse_tier(&tier, service.as_deref())?;
    let script = script_location()?;
    let script = script_path_text(&script)?;
    repair_plan_with(&SystemRunner, &script, &tier)
}

/// Checks the tier (and RESET for a full reset) against a fresh plan, then
/// (release builds only) runs the repair with one macOS administrator prompt.
pub fn repair_run(
    tier: String,
    service: Option<String>,
    confirm: String,
) -> Result<RepairRun, String> {
    let tier = parse_tier(&tier, service.as_deref())?;
    check_confirmation(&tier, &confirm)?;
    let script = script_location()?;
    let script = script_path_text(&script)?;
    repair_for_this_build(&SystemRunner, &script, &tier, &confirm)
}

fn repair_for_this_build(
    runner: &dyn CommandRunner,
    script: &str,
    tier: &Tier,
    confirm: &str,
) -> Result<RepairRun, String> {
    repair_run_with(runner, EXECUTE_OSASCRIPT, script, tier, confirm)
}

// --- Tier ---

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tier {
    Soft,
    Service(String),
    Full,
}

impl Tier {
    fn word(&self) -> &'static str {
        match self {
            Tier::Soft => "soft",
            Tier::Service(_) => "service",
            Tier::Full => "full",
        }
    }

    fn service(&self) -> Option<&str> {
        match self {
            Tier::Service(name) => Some(name),
            _ => None,
        }
    }

    /// The value of `--repair`: one argv element, the name never split.
    fn arg(&self) -> String {
        match self {
            Tier::Service(name) => format!("service:{}", name),
            other => other.word().to_string(),
        }
    }
}

/// The UI's tier word and service name, checked before anything runs.
fn parse_tier(tier: &str, service: Option<&str>) -> Result<Tier, String> {
    match (tier, service) {
        ("soft", None) => Ok(Tier::Soft),
        ("full", None) => Ok(Tier::Full),
        ("service", Some(name)) => {
            if name.is_empty() || name.chars().any(char::is_control) {
                return Err(format!(
                    "Not a usable service name: {:?}. Nothing was changed.",
                    name
                ));
            }
            Ok(Tier::Service(name.to_string()))
        }
        ("service", None) => Err("Choose a service to reset. Nothing was changed.".to_string()),
        ("soft" | "full", Some(_)) => Err(format!(
            "A {} repair does not take a service. Nothing was changed.",
            tier
        )),
        _ => Err(format!("Unknown repair: {:?}. Nothing was changed.", tier)),
    }
}

/// A full reset runs only when the UI passes exactly the word the user typed:
/// RESET. Soft and service repairs are confirmed by their button.
fn check_confirmation(tier: &Tier, confirm: &str) -> Result<(), String> {
    if *tier == Tier::Full && confirm != FULL_RESET_WORD {
        return Err(format!(
            "A full network reset needs you to type {} first. Nothing was changed.",
            FULL_RESET_WORD
        ));
    }
    Ok(())
}

// --- Arguments ---

fn plan_args(script: &str, tier: &Tier) -> Vec<String> {
    vec![
        script.to_string(),
        "--plan".to_string(),
        "--repair".to_string(),
        tier.arg(),
    ]
}

/// The one place the repair command is built: program first, then argv.
/// The debug path returns exactly this; the release path runs exactly this.
fn repair_argv(script: &str, tier: &Tier) -> Vec<String> {
    vec![
        OSASCRIPT.to_string(),
        "-e".to_string(),
        APPLESCRIPT.to_string(),
        script.to_string(),
        "--repair".to_string(),
        tier.arg(),
        "--yes".to_string(),
    ]
}

/// Display only: one argument as the user would type it in Terminal.
fn display_arg(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ":_./-".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Display only (shown for the user to copy into Terminal; never executed).
fn repair_terminal_command(script: &str, tier: &Tier) -> String {
    format!(
        "{} --repair {}",
        terminal_command(script),
        display_arg(&tier.arg())
    )
}

// --- What a plan says (pure) ---

const CURRENT_KEYS: [&str; 3] = ["now", "dns", "search"];

/// `a,b` -> [a, b]; `-` (none) -> [].
fn value_list(v: &str) -> Vec<String> {
    if v == "-" || v.is_empty() {
        return vec![];
    }
    v.split(',').map(|s| s.trim().to_string()).collect()
}

fn repair_records<'a>(
    text: &'a PlanText,
    sub_kind: &'a str,
) -> impl Iterator<Item = &'a PlanRecord> {
    text.records
        .iter()
        .filter(move |r| r.kind == RecordKind::Repair && r.sub_kind == sub_kind)
}

/// `leave` reasons the service reset would refuse as well.
const NOT_RESETTABLE: [&str; 3] = [
    "disabled",
    "settings could not be read",
    "neither DHCP nor Manual",
];

fn soft_services(text: &PlanText) -> Vec<RepairService> {
    text.records
        .iter()
        .filter(|r| {
            r.kind == RecordKind::Repair && (r.sub_kind == "renew" || r.sub_kind == "leave")
        })
        .map(|r| RepairService {
            name: r.subject.clone(),
            action: r.sub_kind.clone(),
            detail: r.detail.clone(),
            resettable: r.sub_kind == "renew"
                || !NOT_RESETTABLE.iter().any(|p| r.detail.starts_with(p)),
        })
        .collect()
}

fn current_settings(text: &PlanText) -> Option<ServiceSettings> {
    let r = repair_records(text, "current").next()?;
    let v = |k| detail_value(&r.detail, k, &CURRENT_KEYS).unwrap_or("-");
    Some(ServiceSettings {
        now: v("now").to_string(),
        dns: value_list(v("dns")),
        search: value_list(v("search")),
    })
}

fn moves(text: &PlanText) -> Vec<String> {
    text.records
        .iter()
        .filter(|r| r.kind == RecordKind::Reset && r.sub_kind == "move")
        .map(|r| r.subject.clone())
        .collect()
}

fn notes(text: &PlanText) -> Vec<String> {
    text.records
        .iter()
        .filter(|r| r.kind == RecordKind::Note)
        .map(|r| r.detail.clone())
        .collect()
}

/// The fresh plan must be for exactly this tier (and this service, same
/// case), and a full reset must have at least one file to move.
fn check_fresh(fresh: &PlanText, tier: &Tier) -> Result<(), String> {
    let want = tier.service().unwrap_or("-");
    let matches =
        repair_records(fresh, "tier").any(|r| r.subject == tier.word() && r.detail == want);
    if !matches {
        return Err(match tier {
            Tier::Service(name) => format!(
                "A fresh plan does not offer to reset \"{}\", so nothing was run. Nothing was changed.",
                name
            ),
            _ => format!(
                "A fresh plan is not a {} repair, so nothing was run. Nothing was changed.",
                tier.word()
            ),
        });
    }
    if *tier == Tier::Full && moves(fresh).is_empty() {
        return Err(
            "A fresh plan has no network settings files to move, so nothing was run. Nothing was changed."
                .to_string(),
        );
    }
    Ok(())
}

// --- Plan and run, testable with a FakeRunner ---

fn repair_plan_with(
    runner: &dyn CommandRunner,
    script: &str,
    tier: &Tier,
) -> Result<RepairPlan, String> {
    let mut plan = RepairPlan {
        tier: tier.word().to_string(),
        service: tier.service().map(str::to_string),
        script_path: script.to_string(),
        terminal_command: repair_terminal_command(script, tier),
        refused: None,
        records: vec![],
        summary: None,
        backup_folder: None,
        notices: vec![],
        services: vec![],
        current: None,
        moves: vec![],
        notes: vec![],
    };
    match run_script_plan(runner, &plan_args(script, tier))? {
        PlanRun::Refused(msg, notices) => {
            plan.refused = Some(msg);
            plan.notices = notices;
        }
        PlanRun::Ready(text, notices) => {
            plan.services = soft_services(&text);
            plan.current = current_settings(&text);
            plan.moves = moves(&text);
            plan.notes = notes(&text);
            plan.summary = text.summary;
            plan.backup_folder = text.backup_folder;
            plan.records = text.records;
            plan.notices = notices;
        }
    }
    Ok(plan)
}

fn repair_run_with(
    runner: &dyn CommandRunner,
    execute: bool,
    script: &str,
    tier: &Tier,
    confirm: &str,
) -> Result<RepairRun, String> {
    check_confirmation(tier, confirm)?;
    let fresh = match run_script_plan(runner, &plan_args(script, tier))? {
        PlanRun::Ready(text, _) => text,
        PlanRun::Refused(msg, _) => {
            return Err(format!(
                "The rescue script would not make a fresh plan, so nothing was run: {}",
                msg
            ))
        }
    };
    check_fresh(&fresh, tier)?;

    let argv = repair_argv(script, tier);
    let terminal = repair_terminal_command(script, tier);
    if !execute {
        return Ok(not_run(
            Outcome::NotExecuted,
            false,
            argv,
            terminal,
            "Debug build: not executed.".to_string(),
        ));
    }
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    match runner.run(&argv[0], &args) {
        Ok(out) => Ok(run_result(argv, terminal, &out)),
        Err(e) => Ok(not_run(
            Outcome::Error,
            true,
            argv,
            terminal,
            format!("Could not start osascript: {}", e),
        )),
    }
}

fn not_run(
    outcome: Outcome,
    executed: bool,
    argv: Vec<String>,
    terminal_command: String,
    msg: String,
) -> RepairRun {
    RepairRun {
        outcome,
        executed,
        argv,
        exit_code: None,
        steps: vec![],
        warnings: 0,
        backup_folder: None,
        log_path: None,
        restart_needed: false,
        restart_text: None,
        messages: vec![msg],
        terminal_command,
    }
}

fn run_result(argv: Vec<String>, terminal_command: String, out: &CmdOutput) -> RepairRun {
    let text = parse_run_output(&out.stdout);
    let (outcome, extra) = map_outcome(out, &text);
    let mut messages = text.messages;
    if let Some(m) = extra {
        messages.push(m);
    }
    if outcome == Outcome::Refused && messages.is_empty() {
        messages.push("The rescue script refused to run. Nothing was changed.".to_string());
    }
    let warnings = text
        .steps
        .iter()
        .filter(|s| s.status == StepStatus::VerifyWarn)
        .count();
    // Only a run that got as far as ok/partial can have changed files.
    let restart = text
        .restart
        .filter(|_| matches!(outcome, Outcome::Ok | Outcome::Partial));
    RepairRun {
        outcome,
        executed: true,
        argv,
        exit_code: text.exit,
        steps: text.steps,
        warnings,
        backup_folder: text.backup_folder,
        log_path: text.log_path,
        restart_needed: restart.is_some(),
        restart_text: restart,
        messages,
        terminal_command,
    }
}

#[cfg(test)]
mod tests {
    // FakeRunner only: no test runs bash, osascript or the rescue script.
    use super::super::rescue::{parse_plan, BASH};
    use super::*;
    use crate::platform::runner::FakeRunner;

    const SCRIPT: &str =
        "/Applications/It Hurts When IP.app/Contents/Resources/ithurtswhenip-rescue.command";

    const G_SOFT: &str = include_str!("../../../../recovery/tests/golden/plan-repair-soft.txt");
    const G_SERVICE: &str =
        include_str!("../../../../recovery/tests/golden/plan-repair-service.txt");
    const G_FULL: &str = include_str!("../../../../recovery/tests/golden/plan-repair-full.txt");

    const ODD_NAME: &str = "Lab \"Desk\", Jay's";
    const SLASH_NAME: &str = "USB 10/100/1000 LAN";

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn svc(name: &str) -> Tier {
        Tier::Service(name.to_string())
    }

    fn count(text: &PlanText, kind: RecordKind) -> usize {
        text.records.iter().filter(|r| r.kind == kind).count()
    }

    fn summary_num(summary: &str, key: &str) -> usize {
        summary
            .split_whitespace()
            .find_map(|kv| kv.strip_prefix(&format!("{}=", key)))
            .unwrap()
            .parse()
            .unwrap()
    }

    /// A service plan for any name, like the golden's.
    fn service_plan(name: &str) -> String {
        format!(
            "# It Hurts When IP rescue script, version 1\n\
             # mode: plan\n\
             # repair: service:{n}\n\
             # backup folder: /Users/Shared/ItHurtsWhenIP-backups/20261117T090000Z\n\
             # plan only: nothing has been changed\n\
             REPAIR\ttier\tservice\t{n}\n\
             BACKUP\tsnapshot\tnetwork\tnetwork-before.txt\n\
             BACKUP\tsnapshot\t{n}\tservice-before.txt\n\
             REPAIR\tcurrent\t{n}\tnow=dhcp/10.0.0.20/255.255.255.0/10.0.0.1 dns=- search=-\n\
             REPAIR\tsetdhcp\t{n}\twas=dhcp/10.0.0.20/255.255.255.0/10.0.0.1\n\
             REPAIR\tcleardns\t{n}\twas=-\n\
             REPAIR\tclearsearch\t{n}\twas=-\n\
             REPAIR\tdisable\t{n}\t-\n\
             REPAIR\tenable\t{n}\t-\n\
             # summary: backup=2 repair=5 leave=0\n",
            n = name
        )
    }

    fn osa(stdout: &str) -> CmdOutput {
        CmdOutput {
            code: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    // --- tier parsing and confirmation ---

    #[test]
    fn tiers_parse_and_bad_ones_are_refused() {
        assert_eq!(parse_tier("soft", None), Ok(Tier::Soft));
        assert_eq!(parse_tier("full", None), Ok(Tier::Full));
        assert_eq!(parse_tier("service", Some(ODD_NAME)), Ok(svc(ODD_NAME)));
        for (t, n) in [
            ("Soft", None),
            ("restore-backup", None),
            ("", None),
            ("service", None),
            ("service", Some("")),
            ("service", Some("Tab\tName")),
            ("service", Some("Line\nBreak")),
            ("soft", Some("Wi-Fi")),
            ("full", Some("Wi-Fi")),
        ] {
            let err = parse_tier(t, n).unwrap_err();
            assert!(err.contains("Nothing was changed"), "{t:?} {n:?}: {err}");
        }
    }

    #[test]
    fn tier_args_keep_the_service_name_whole() {
        assert_eq!(Tier::Soft.arg(), "soft");
        assert_eq!(Tier::Full.arg(), "full");
        assert_eq!(svc(SLASH_NAME).arg(), "service:USB 10/100/1000 LAN");
        assert_eq!(svc(ODD_NAME).arg(), "service:Lab \"Desk\", Jay's");
    }

    #[test]
    fn full_reset_is_refused_without_the_typed_word_before_anything_runs() {
        for typed in ["", "reset", " RESET", "RESET ", "RESET\n", "yes"] {
            let fake = FakeRunner::new();
            fake.push(Some(0), G_FULL, "")
                .push(Some(0), "IHWIP_EXIT=0\n", "");
            let err = repair_run_with(&fake, true, SCRIPT, &Tier::Full, typed).unwrap_err();
            assert!(err.contains("type RESET"), "{typed:?}: {err}");
            assert!(fake.calls().is_empty(), "{typed:?}: nothing ran");
        }
        // Soft and service do not need the word.
        assert!(check_confirmation(&Tier::Soft, "").is_ok());
        assert!(check_confirmation(&svc("Wi-Fi"), "").is_ok());
        assert!(check_confirmation(&Tier::Full, "RESET").is_ok());
    }

    // --- plan goldens ---

    #[test]
    fn repair_goldens_parse_with_counts_matching_their_summaries() {
        for (name, text) in [("soft", G_SOFT), ("service", G_SERVICE), ("full", G_FULL)] {
            let p = parse_plan(text);
            let summary = p.summary.as_deref().unwrap_or_else(|| panic!("{name}"));
            assert_eq!(count(&p, RecordKind::Other), 0, "{name}");
            assert_eq!(
                count(&p, RecordKind::Backup),
                summary_num(summary, "backup"),
                "{name}"
            );
            assert_eq!(
                p.backup_folder.as_deref(),
                Some("/Users/Shared/ItHurtsWhenIP-backups/20261115T120000Z"),
                "{name}"
            );
            let tier = repair_records(&p, "tier").next().unwrap();
            assert_eq!(tier.subject, name);
        }
        let soft = parse_plan(G_SOFT);
        let writes = soft
            .records
            .iter()
            .filter(|r| {
                r.kind == RecordKind::Repair && !["tier", "leave"].contains(&r.sub_kind.as_str())
            })
            .count();
        assert_eq!(
            writes,
            summary_num(soft.summary.as_deref().unwrap(), "repair")
        );
        assert_eq!(repair_records(&soft, "leave").count(), 2);

        let full = parse_plan(G_FULL);
        assert_eq!(count(&full, RecordKind::Reset), 4);
        assert_eq!(
            moves(&full).len(),
            summary_num(full.summary.as_deref().unwrap(), "move")
        );
    }

    #[test]
    fn golden_soft_services_and_who_can_be_reset() {
        let fake = FakeRunner::new();
        fake.push(Some(0), G_SOFT, "");
        let plan = repair_plan_with(&fake, SCRIPT, &Tier::Soft).unwrap();
        assert_eq!(
            fake.calls(),
            vec![(BASH.to_string(), s(&[SCRIPT, "--plan", "--repair", "soft"]))]
        );
        assert_eq!(plan.tier, "soft");
        assert_eq!(plan.service, None);
        assert_eq!(plan.refused, None);
        let list: Vec<(&str, &str, bool)> = plan
            .services
            .iter()
            .map(|x| (x.name.as_str(), x.action.as_str(), x.resettable))
            .collect();
        assert_eq!(
            list,
            vec![
                (SLASH_NAME, "renew", true),
                ("Dante Primary", "leave", true),
                ("Wi-Fi", "renew", true),
                ("Thunderbolt Bridge", "renew", true),
                ("Old Adapter", "leave", false),
            ]
        );
        assert_eq!(
            plan.services[0].detail,
            "device=en7 now=dhcp/192.168.1.42/255.255.255.0/192.168.1.1"
        );
        assert_eq!(plan.current, None);
        assert!(plan.moves.is_empty());
        assert!(plan.notes.is_empty());
        assert_eq!(
            plan.terminal_command,
            format!("sudo bash \"{}\" --repair soft", SCRIPT)
        );
    }

    #[test]
    fn leave_reasons_decide_what_the_service_picker_offers() {
        let text = "REPAIR\tleave\tA\tdisabled: not touched\n\
                    REPAIR\tleave\tB\tsettings could not be read: not touched\n\
                    REPAIR\tleave\tC\tneither DHCP nor Manual settings (for example a VPN): not touched\n\
                    REPAIR\tleave\tD\tno network device (for example a VPN): not renewed\n\
                    REPAIR\tleave\tE\tdevice en20 is not attached: not renewed\n\
                    REPAIR\tleave\tF\tmanual (static) settings: not touched\n\
                    # summary: backup=1 repair=2 leave=6\n";
        let offered: Vec<String> = soft_services(&parse_plan(text))
            .into_iter()
            .filter(|x| x.resettable)
            .map(|x| x.name)
            .collect();
        assert_eq!(offered, s(&["D", "E", "F"]));
    }

    #[test]
    fn golden_service_shows_what_will_be_cleared() {
        let fake = FakeRunner::new();
        fake.push(Some(0), G_SERVICE, "");
        let plan = repair_plan_with(&fake, SCRIPT, &svc("Dante Primary")).unwrap();
        assert_eq!(
            fake.calls(),
            vec![(
                BASH.to_string(),
                s(&[SCRIPT, "--plan", "--repair", "service:Dante Primary"])
            )]
        );
        assert_eq!(plan.tier, "service");
        assert_eq!(plan.service.as_deref(), Some("Dante Primary"));
        assert_eq!(
            plan.current,
            Some(ServiceSettings {
                now: "manual/192.168.10.21/255.255.255.0/-".into(),
                dns: s(&["10.0.0.1", "10.0.0.2"]),
                search: s(&["show.example"]),
            })
        );
        let kinds: Vec<&str> = plan
            .records
            .iter()
            .filter(|r| r.kind == RecordKind::Repair)
            .map(|r| r.sub_kind.as_str())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "tier",
                "current",
                "setdhcp",
                "cleardns",
                "clearsearch",
                "disable",
                "enable"
            ]
        );
        assert_eq!(
            plan.terminal_command,
            format!("sudo bash \"{}\" --repair 'service:Dante Primary'", SCRIPT)
        );
    }

    #[test]
    fn empty_dns_and_search_are_empty_lists() {
        let p = parse_plan(&service_plan("Wi-Fi"));
        let c = current_settings(&p).unwrap();
        assert_eq!(c.now, "dhcp/10.0.0.20/255.255.255.0/10.0.0.1");
        assert!(c.dns.is_empty());
        assert!(c.search.is_empty());
    }

    #[test]
    fn golden_full_lists_moves_backup_and_notes() {
        let fake = FakeRunner::new();
        fake.push(Some(0), G_FULL, "");
        let plan = repair_plan_with(&fake, SCRIPT, &Tier::Full).unwrap();
        assert_eq!(
            fake.calls(),
            vec![(BASH.to_string(), s(&[SCRIPT, "--plan", "--repair", "full"]))]
        );
        assert_eq!(
            plan.moves,
            s(&[
                "/Library/Preferences/SystemConfiguration/preferences.plist",
                "/Library/Preferences/SystemConfiguration/NetworkInterfaces.plist",
                "/Library/Preferences/SystemConfiguration/com.apple.network.identification.plist",
                "/Library/Preferences/SystemConfiguration/com.apple.airport.preferences.plist",
            ])
        );
        assert_eq!(plan.notes.len(), 3);
        assert!(plan.notes[2].contains("never restarts it"));
        assert_eq!(
            plan.backup_folder.as_deref(),
            Some("/Users/Shared/ItHurtsWhenIP-backups/20261115T120000Z")
        );
        assert!(plan.services.is_empty());
        assert_eq!(plan.current, None);
    }

    #[test]
    fn unknown_records_in_a_repair_plan_are_ignored() {
        let text = format!(
            "{}FUTURE\tthing\tx\ty\nREPAIR\tonly-three\tfields\nREPAIR\trenew\tX\td\textra\n",
            G_SOFT
        );
        let fake = FakeRunner::new();
        fake.push(Some(0), &text, "");
        let plan = repair_plan_with(&fake, SCRIPT, &Tier::Soft).unwrap();
        assert_eq!(plan.services.len(), 5, "malformed renew is not a service");
        let other: Vec<&str> = plan
            .records
            .iter()
            .filter(|r| r.kind == RecordKind::Other)
            .map(|r| r.detail.as_str())
            .collect();
        assert_eq!(
            other,
            vec![
                "FUTURE\tthing\tx\ty",
                "REPAIR\tonly-three\tfields",
                "REPAIR\trenew\tX\td\textra"
            ]
        );
    }

    #[test]
    fn refused_plan_carries_the_scripts_message() {
        let fake = FakeRunner::new();
        fake.push(
            Some(2),
            "",
            "rescue: --repair service:Old Adapter: that service is disabled; only enabled services can be reset. Nothing has been changed.\n",
        );
        let plan = repair_plan_with(&fake, SCRIPT, &svc("Old Adapter")).unwrap();
        assert!(plan.refused.unwrap().contains("that service is disabled"));
        assert!(plan.records.is_empty());
    }

    // --- run: argv, fresh-plan checks, debug ---

    fn run_ok(tier: &Tier, plan: &str, confirm: &str) -> (RepairRun, Vec<(String, Vec<String>)>) {
        let fake = FakeRunner::new();
        fake.push(Some(0), plan, "")
            .push(Some(0), "IHWIP_EXIT=0\n", "");
        let run = repair_run_with(&fake, true, SCRIPT, tier, confirm).unwrap();
        (run, fake.calls())
    }

    #[test]
    fn run_argv_for_each_tier() {
        let cases: Vec<(Tier, String, &str, &str)> = vec![
            (Tier::Soft, G_SOFT.to_string(), "", "soft"),
            (Tier::Full, G_FULL.to_string(), "RESET", "full"),
            (
                svc("Dante Primary"),
                G_SERVICE.to_string(),
                "",
                "service:Dante Primary",
            ),
            (
                svc(SLASH_NAME),
                service_plan(SLASH_NAME),
                "",
                "service:USB 10/100/1000 LAN",
            ),
            (
                svc(ODD_NAME),
                service_plan(ODD_NAME),
                "",
                "service:Lab \"Desk\", Jay's",
            ),
        ];
        for (tier, plan, confirm, arg) in cases {
            let (run, calls) = run_ok(&tier, &plan, confirm);
            let expected = s(&[
                OSASCRIPT,
                "-e",
                APPLESCRIPT,
                SCRIPT,
                "--repair",
                arg,
                "--yes",
            ]);
            assert_eq!(run.argv, expected, "{arg}");
            assert_eq!(
                calls,
                vec![
                    (BASH.to_string(), s(&[SCRIPT, "--plan", "--repair", arg])),
                    (OSASCRIPT.to_string(), expected[1..].to_vec()),
                ],
                "{arg}"
            );
            assert_eq!(run.outcome, Outcome::Ok);
            assert!(run.executed);
        }
    }

    #[test]
    fn tier_or_service_not_in_the_fresh_plan_is_rejected_without_osascript() {
        let cases: Vec<(Tier, String)> = vec![
            // Asked for one service, the plan is for another / the soft tier.
            (svc("Wi-Fi"), G_SERVICE.to_string()),
            (svc("dante primary"), G_SERVICE.to_string()),
            (svc("Dante Primary"), G_SOFT.to_string()),
            (Tier::Soft, G_FULL.to_string()),
            (Tier::Full, G_SOFT.to_string()),
            // A full plan with nothing to move.
            (
                Tier::Full,
                G_FULL
                    .lines()
                    .filter(|l| !l.starts_with("RESET"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ];
        for (tier, plan) in cases {
            let fake = FakeRunner::new();
            fake.push(Some(0), &plan, "");
            let err = repair_run_with(&fake, true, SCRIPT, &tier, "RESET").unwrap_err();
            assert!(err.contains("Nothing was changed"), "{tier:?}: {err}");
            assert_eq!(fake.calls().len(), 1, "{tier:?}: no osascript");
        }
    }

    #[test]
    fn refused_or_failed_fresh_plan_never_reaches_osascript() {
        let fake = FakeRunner::new();
        fake.push(
            Some(2),
            "",
            "rescue: --repair service:Gone: there is no network service named exactly \"Gone\" (see -listallnetworkservices). Nothing has been changed.\n",
        );
        let err = repair_run_with(&fake, true, SCRIPT, &svc("Gone"), "").unwrap_err();
        assert!(err.contains("no network service named exactly"), "{err}");
        assert_eq!(fake.calls().len(), 1);

        for code in [Some(1), None] {
            let fake = FakeRunner::new();
            fake.push(code, "", "");
            assert!(repair_run_with(&fake, true, SCRIPT, &Tier::Soft, "").is_err());
            assert_eq!(fake.calls().len(), 1);
        }
    }

    #[test]
    fn not_executing_returns_the_same_argv_and_never_runs_osascript() {
        let fake = FakeRunner::new();
        fake.push(Some(0), &service_plan(ODD_NAME), "");
        let run = repair_run_with(&fake, false, SCRIPT, &svc(ODD_NAME), "").unwrap();
        assert_eq!(run.outcome, Outcome::NotExecuted);
        assert!(!run.executed);
        assert!(!run.restart_needed);
        assert_eq!(run.argv, repair_argv(SCRIPT, &svc(ODD_NAME)));
        assert_eq!(fake.calls().len(), 1);
        assert!(fake.calls().iter().all(|(p, _)| p != OSASCRIPT));
    }

    /// The public command's path in a debug build: osascript is never run.
    #[cfg(debug_assertions)]
    #[allow(clippy::assertions_on_constants)] // guards the cfg-dependent const on purpose
    #[test]
    fn public_debug_path_never_runs_osascript() {
        assert!(!EXECUTE_OSASCRIPT);
        let fake = FakeRunner::new();
        fake.push(Some(0), G_FULL, "");
        let run = repair_for_this_build(&fake, SCRIPT, &Tier::Full, "RESET").unwrap();
        assert_eq!(run.outcome, Outcome::NotExecuted);
        assert_eq!(run.argv[0], OSASCRIPT);
        assert_eq!(fake.calls().len(), 1);
        assert_eq!(fake.calls()[0].0, BASH);
    }

    #[test]
    fn terminal_command_display_quoting() {
        assert_eq!(display_arg("soft"), "soft");
        assert_eq!(display_arg("service:Wi-Fi"), "service:Wi-Fi");
        assert_eq!(
            repair_terminal_command(SCRIPT, &svc(ODD_NAME)),
            format!(
                "sudo bash \"{}\" --repair 'service:Lab \"Desk\", Jay'\\''s'",
                SCRIPT
            )
        );
        assert_eq!(
            repair_terminal_command(SCRIPT, &svc(SLASH_NAME)),
            format!(
                "sudo bash \"{}\" --repair 'service:USB 10/100/1000 LAN'",
                SCRIPT
            )
        );
    }

    // --- result lines ---

    /// A soft repair as the script prints it (plan, steps, report), then the
    /// AppleScript's exit marker. One service got no DHCP address yet.
    const SOFT_RUN: &str = "# It Hurts When IP rescue script, version 1\n\
# mode: repair\n\
# repair: soft\n\
# backup folder: /Users/Shared/ItHurtsWhenIP-backups/20261117T090000Z\n\
# nothing has been changed yet\n\
REPAIR\ttier\tsoft\t-\n\
BACKUP\tsnapshot\tnetwork\tnetwork-before.txt\n\
REPAIR\tflushcache\tdns-cache\tdscacheutil -flushcache\n\
REPAIR\thup\tmDNSResponder\tkillall -HUP mDNSResponder\n\
REPAIR\trenew\tUSB 10/100/1000 LAN\tdevice=en7 now=dhcp/192.168.1.42/255.255.255.0/192.168.1.1\n\
REPAIR\trenew\tThunderbolt Bridge\tdevice=bridge0 now=dhcp/169.254.12.34/255.255.0.0/-\n\
REPAIR\tleave\tDante Primary\tmanual (static) settings: not touched\n\
# summary: backup=1 repair=4 leave=1\n\
OK\tBACKUP\tsnapshot\tnetwork\tnetwork-before.txt\n\
OK\tREPAIR\tflushcache\tdns-cache\tdscacheutil -flushcache\n\
OK\tREPAIR\thup\tmDNSResponder\tkillall -HUP mDNSResponder\n\
OK\tREPAIR\trenew\tUSB 10/100/1000 LAN\tdevice=en7 now=dhcp/192.168.1.42/255.255.255.0/192.168.1.1\n\
OK\tREPAIR\trenew\tThunderbolt Bridge\tdevice=bridge0 now=dhcp/169.254.12.34/255.255.0.0/-\n\
VERIFY\tok\trepair:USB 10/100/1000 LAN\tnow=dhcp/192.168.1.42/255.255.255.0/192.168.1.1\n\
VERIFY\twarn\trepair:Thunderbolt Bridge\tnow=dhcp/169.254.12.34/255.255.0.0/-; link-local only (169.254.x.x): no DHCP server answered yet; check the cable, the switch or the DHCP server\n\
# result\n\
# backup folder: /Users/Shared/ItHurtsWhenIP-backups/20261117T090000Z\n\
# log: /Users/Shared/ItHurtsWhenIP-backups/20261117T090000Z/rescue.log\n\
# failed steps: 0\n\
# warnings: 1\n\
VERIFY\twarn\trepair:Thunderbolt Bridge\tnow=dhcp/169.254.12.34/255.255.0.0/-; link-local only (169.254.x.x): no DHCP server answered yet; check the cable, the switch or the DHCP server\n\
IHWIP_EXIT=0\n";

    /// A full reset where one file could not be removed: partial, but the
    /// others moved, so a restart is still needed.
    const FULL_RUN_PARTIAL: &str = "# It Hurts When IP rescue script, version 1\n\
# mode: repair\n\
# repair: full\n\
# backup folder: /Users/Shared/ItHurtsWhenIP-backups/20261117T090000Z\n\
# nothing has been changed yet\n\
REPAIR\ttier\tfull\t-\n\
RESET\tmove\t/Library/Preferences/SystemConfiguration/preferences.plist\tsize=62\n\
RESET\tmove\t/Library/Preferences/SystemConfiguration/NetworkInterfaces.plist\tsize=35\n\
NOTE\tinfo\treset\tWi-Fi networks may need to be rejoined\n\
# summary: backup=4 move=2 absent=0 skip=0\n\
OK\tBACKUP\tsnapshot\tnetwork\tnetwork-before.txt\n\
OK\tBACKUP\tfile\t/Library/Preferences/SystemConfiguration/preferences.plist\tsystem-configuration/preferences.plist\n\
OK\tBACKUP\tfile\t/Library/Preferences/SystemConfiguration/NetworkInterfaces.plist\tsystem-configuration/NetworkInterfaces.plist\n\
OK\tBACKUP\tmanifest\tSHA256SUMS\tsystem-configuration/SHA256SUMS\n\
OK\tBACKUP\tcheck\tsystem-configuration/SHA256SUMS\t2 file(s) match\n\
OK\tRESET\tmove\t/Library/Preferences/SystemConfiguration/preferences.plist\tsize=62\n\
FAILED\tRESET\tmove\t/Library/Preferences/SystemConfiguration/NetworkInterfaces.plist\tsize=35\tcould not remove: Operation not permitted\n\
VERIFY\tok\treset:/Library/Preferences/SystemConfiguration/preferences.plist\tgone; backup copy matches SHA256SUMS\n\
VERIFY\tFAILED\treset:/Library/Preferences/SystemConfiguration/NetworkInterfaces.plist\tstill present\n\
FUTURE\tline\tfrom\ta newer script\n\
# result\n\
# backup folder: /Users/Shared/ItHurtsWhenIP-backups/20261117T090000Z\n\
# log: /Users/Shared/ItHurtsWhenIP-backups/20261117T090000Z/rescue.log\n\
# failed steps: 1; failed checks: 1\n\
FAILED\tRESET\tmove\t/Library/Preferences/SystemConfiguration/NetworkInterfaces.plist\tsize=35\tcould not remove: Operation not permitted\n\
VERIFY\tFAILED\treset:/Library/Preferences/SystemConfiguration/NetworkInterfaces.plist\tstill present\n\
# warnings: 0\n\
RESTART\tneeded\t-\tRESTART NEEDED: restart this Mac yourself to finish; until then the running network keeps its current settings\n\
IHWIP_EXIT=1\n";

    #[test]
    fn soft_result_with_a_warning() {
        let run = run_result(vec![], String::new(), &osa(SOFT_RUN));
        assert_eq!(run.outcome, Outcome::Ok);
        assert_eq!(run.exit_code, Some(0));
        assert_eq!(run.steps.len(), 7, "report repeats are not counted again");
        assert_eq!(run.warnings, 1);
        assert!(!run.restart_needed);
        assert_eq!(run.restart_text, None);
        assert_eq!(
            run.steps[3],
            StepLine {
                status: StepStatus::Ok,
                kind: "REPAIR".into(),
                sub_kind: "renew".into(),
                subject: SLASH_NAME.into(),
                detail: "device=en7 now=dhcp/192.168.1.42/255.255.255.0/192.168.1.1".into(),
                reason: String::new(),
            }
        );
        assert_eq!(run.steps[5].status, StepStatus::VerifyOk);
        assert_eq!(run.steps[6].status, StepStatus::VerifyWarn);
        assert_eq!(run.steps[6].subject, "repair:Thunderbolt Bridge");
        assert!(run.steps[6].detail.contains("link-local only"));
        assert_eq!(
            run.log_path.as_deref(),
            Some("/Users/Shared/ItHurtsWhenIP-backups/20261117T090000Z/rescue.log")
        );
        assert!(run.messages.is_empty());
    }

    #[test]
    fn full_partial_result_with_restart_needed() {
        let run = run_result(vec![], String::new(), &osa(FULL_RUN_PARTIAL));
        assert_eq!(run.outcome, Outcome::Partial);
        assert_eq!(run.exit_code, Some(1));
        assert_eq!(
            run.steps.len(),
            9,
            "unknown line ignored, repeats not counted"
        );
        assert!(run.restart_needed);
        assert!(run
            .restart_text
            .as_deref()
            .unwrap()
            .starts_with("RESTART NEEDED: restart this Mac yourself"));
        assert_eq!(run.warnings, 0);
        let failed: Vec<StepStatus> = run
            .steps
            .iter()
            .map(|s| s.status)
            .filter(|s| matches!(s, StepStatus::Failed | StepStatus::VerifyFailed))
            .collect();
        assert_eq!(failed, vec![StepStatus::Failed, StepStatus::VerifyFailed]);
        assert_eq!(run.steps[4].kind, "BACKUP");
        assert_eq!(run.steps[4].sub_kind, "check");
        assert_eq!(
            run.backup_folder.as_deref(),
            Some("/Users/Shared/ItHurtsWhenIP-backups/20261117T090000Z")
        );
    }

    #[test]
    fn restart_line_with_cr_line_endings() {
        let run = run_result(
            vec![],
            String::new(),
            &osa(&FULL_RUN_PARTIAL.replace('\n', "\r")),
        );
        assert!(run.restart_needed);
        assert_eq!(run.steps.len(), 9);
    }

    #[test]
    fn exit_status_mapping() {
        let cases = [
            ("OK\tREPAIR\thup\tmDNSResponder\tx\n# result\nIHWIP_EXIT=0\n", Outcome::Ok),
            ("FAILED\tREPAIR\tenable\tWi-Fi\t-\ttried twice\n# result\nIHWIP_EXIT=1\n", Outcome::Partial),
            ("rescue: --repair full: none of the network configuration files is in /Library/Preferences/SystemConfiguration (already reset? then restart the Mac). Nothing has been changed.\nIHWIP_EXIT=2\n", Outcome::Refused),
            ("IHWIP_OSA_ERROR=-128\nUser canceled.\n", Outcome::Cancelled),
            ("IHWIP_OSA_ERROR=-60005\nThe administrator user name or password was incorrect.\n", Outcome::Error),
            ("", Outcome::Error),
        ];
        for (stdout, outcome) in cases {
            let run = run_result(vec![], String::new(), &osa(stdout));
            assert_eq!(run.outcome, outcome, "{stdout:?}");
        }
        let refused = run_result(vec![], String::new(), &osa(cases[2].0));
        assert!(refused.messages[0].contains("already reset?"));
        let cancelled = run_result(vec![], String::new(), &osa(cases[3].0));
        assert!(cancelled.messages.is_empty());
        assert!(cancelled.steps.is_empty());
        let wrong_pw = run_result(vec![], String::new(), &osa(cases[4].0));
        assert!(wrong_pw.messages[0].contains("did not run the rescue script"));
        assert!(wrong_pw.messages[0].contains("-60005"));
    }

    #[test]
    fn restart_is_only_reported_for_a_run_that_got_to_ok_or_partial() {
        let out = "RESTART\tneeded\t-\tRESTART NEEDED: x\nIHWIP_EXIT=2\n";
        let run = run_result(vec![], String::new(), &osa(out));
        assert_eq!(run.outcome, Outcome::Refused);
        assert!(!run.restart_needed);
        let out = "RESTART\tneeded\t-\tRESTART NEEDED: x\nIHWIP_EXIT=0\n";
        assert!(run_result(vec![], String::new(), &osa(out)).restart_needed);
    }

    #[test]
    fn osascript_that_cannot_start_is_an_error() {
        let fake = FakeRunner::new();
        fake.push(Some(0), G_SOFT, ""); // osascript call then gets an io error
        let run = repair_run_with(&fake, true, SCRIPT, &Tier::Soft, "").unwrap();
        assert_eq!(run.outcome, Outcome::Error);
        assert!(run.messages[0].starts_with("Could not start osascript"));
        assert_eq!(
            run.terminal_command,
            format!("sudo bash \"{}\" --repair soft", SCRIPT)
        );
    }
}
