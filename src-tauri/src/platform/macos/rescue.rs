// In-app uninstall through the bundled rescue script
// (recovery/ithurtswhenip-rescue.command). macOS only.
//
// This path never uses the privileged helper: the helper may be the broken
// part. Instead:
//  - `rescue_plan` runs the script with --plan (no privileges, read-only) and
//    parses its tab-separated records. The record contract is the comment
//    block at the top of `render_plan` in the script.
//  - `rescue_uninstall` re-runs the plan, checks the user's network choices
//    against that fresh plan, then runs the script once as root through
//    /usr/bin/osascript and the one fixed AppleScript below. Every argument
//    reaches the AppleScript as argv and is quoted there with `quoted form of`:
//    Rust never builds a shell string and never puts anything into the
//    AppleScript source.
//  - Debug builds never run osascript: they return the argv they would have
//    run (decided in Rust from cfg!(debug_assertions), never by the UI).
//
// Every program runs through a CommandRunner (argv only), so the tests use
// FakeRunner and never run a real tool.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::platform::runner::{CmdOutput, CommandRunner, SystemRunner};

/// File name of the rescue script, in the app bundle's Contents/Resources
/// (release) or the repo's recovery/ folder (debug).
const SCRIPT_FILE: &str = "ithurtswhenip-rescue.command";

const BASH: &str = "/bin/bash";
const OSASCRIPT: &str = "/usr/bin/osascript";

/// The only AppleScript the app ever runs, passed to osascript with `-e`.
///
/// It receives the rescue script path and its flags as `argv` and quotes
/// every item with `quoted form of`. Nothing else is ever added to it.
///
/// The fixed tail ` 2>&1; echo "IHWIP_EXIT=$?"` exists because `do shell
/// script` throws stdout away when the command exits non-zero, and a partial
/// run (exit 1) must still report every OK/FAILED line. The exit code comes
/// back as the last line instead. A dismissed password prompt (or any other
/// AppleScript error) comes back as `IHWIP_OSA_ERROR=<number>` plus the
/// message, so osascript itself still exits 0.
///
/// A test asserts this exact text: any change to it must show up in review.
const APPLESCRIPT: &str = r#"on run argv
    set cmd to "/bin/bash"
    repeat with a in argv
        set cmd to cmd & space & quoted form of (contents of a)
    end repeat
    set cmd to cmd & " 2>&1; echo \"IHWIP_EXIT=$?\""
    try
        return do shell script cmd with administrator privileges without altering line endings
    on error errMsg number errNum
        return "IHWIP_OSA_ERROR=" & errNum & linefeed & errMsg
    end try
end run"#;

const EXIT_MARKER: &str = "IHWIP_EXIT=";
const OSA_ERROR_MARKER: &str = "IHWIP_OSA_ERROR=";
/// AppleScript's "User canceled." (the password prompt was dismissed).
const USER_CANCELED: i32 = -128;

// --- Types the UI receives ---

/// TYPE field of a plan record. Anything unknown (or malformed) is `Other`
/// and keeps the whole line verbatim in `detail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Note,
    Backup,
    Network,
    Stop,
    Quit,
    Remove,
    Forget,
    Leftover,
    Other,
}

/// One plan record: TYPE KIND SUBJECT DETAIL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanRecord {
    pub kind: RecordKind,
    pub sub_kind: String,
    pub subject: String,
    pub detail: String,
}

/// An enabled service on Manual settings (`NETWORK manual`): the only ones
/// the UI offers to switch to DHCP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManualService {
    pub name: String,
    pub ip: String,
    pub mask: String,
    pub router: String,
}

/// A service in the helper's journal (`NETWORK journal`). `original` and
/// `current` are `<mode>/<ip>/<mask>/<router>` as the script prints them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JournalService {
    pub name: String,
    pub original: String,
    pub current: String,
    pub selected: bool,
}

/// What `rescue_plan` returns. When the script refused the choices (exit 2),
/// `refused` holds its message and the lists are empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RescuePlan {
    pub script_path: String,
    /// Display only, for the user to copy into Terminal. Never executed.
    pub terminal_command: String,
    pub refused: Option<String>,
    pub records: Vec<PlanRecord>,
    /// The `# summary:` line without its prefix.
    pub summary: Option<String>,
    pub backup_folder: Option<String>,
    /// `rescue: note:` lines from stderr.
    pub notices: Vec<String>,
    pub nothing_to_remove: bool,
    pub manual: Vec<ManualService>,
    pub journal: Vec<JournalService>,
    /// A journal was found and lists at least one service.
    pub journal_usable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Ok,
    Failed,
    VerifyOk,
    VerifyFailed,
}

/// One execution line: `OK TYPE KIND SUBJECT DETAIL`, `FAILED TYPE KIND
/// SUBJECT DETAIL REASON` or `VERIFY ok|FAILED SUBJECT [DETAIL]` (VERIFY lines
/// have kind "VERIFY" and an empty sub_kind).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepLine {
    pub status: StepStatus,
    pub kind: String,
    pub sub_kind: String,
    pub subject: String,
    pub detail: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Script exit 0.
    Ok,
    /// Script exit 1: some steps failed (listed).
    Partial,
    /// Script exit 2: it refused before changing anything.
    Refused,
    /// The macOS password prompt was dismissed: nothing ran.
    Cancelled,
    /// osascript or the script could not run, or the result was unreadable.
    Error,
    /// Debug build: osascript was not run; `argv` is what would have run.
    NotExecuted,
}

/// What `rescue_uninstall` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RescueRun {
    pub outcome: Outcome,
    pub executed: bool,
    /// Program first, then every argument, exactly as given to the runner.
    pub argv: Vec<String>,
    pub exit_code: Option<i32>,
    /// Stage A's lines, in the order the script printed them.
    pub steps: Vec<StepLine>,
    pub backup_folder: Option<String>,
    pub log_path: Option<String>,
    /// Stage B was started and waits for this app to quit.
    pub stage_b_started: bool,
    pub messages: Vec<String>,
}

impl RescueRun {
    /// The app must quit so stage B can remove the bundle: only after a run
    /// that really executed and reported starting stage B.
    pub fn quit_required(&self) -> bool {
        self.executed && self.stage_b_started
    }

    fn without_output(outcome: Outcome, executed: bool, argv: Vec<String>, msg: String) -> Self {
        RescueRun {
            outcome,
            executed,
            argv,
            exit_code: None,
            steps: vec![],
            backup_folder: None,
            log_path: None,
            stage_b_started: false,
            messages: vec![msg],
        }
    }
}

/// The user's network choices for the uninstall.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkChoices {
    pub dhcp: Vec<String>,
    pub restore_journal: bool,
}

// --- Public API (re-exported through platform::) ---

/// Runs the rescue script's `--plan` (read-only, no privileges).
pub fn rescue_plan(dhcp: Vec<String>, restore_journal: bool) -> Result<RescuePlan, String> {
    let script = script_location()?;
    let script = script_path_text(&script)?;
    rescue_plan_with(
        &SystemRunner,
        &script,
        std::process::id(),
        &NetworkChoices {
            dhcp,
            restore_journal,
        },
    )
}

/// Checks the choices against a fresh plan, then (release builds only) runs
/// the uninstall with one macOS administrator prompt.
pub fn rescue_uninstall(dhcp: Vec<String>, restore_journal: bool) -> Result<RescueRun, String> {
    let script = script_location()?;
    let script = script_path_text(&script)?;
    uninstall_for_this_build(
        &SystemRunner,
        &script,
        std::process::id(),
        &NetworkChoices {
            dhcp,
            restore_journal,
        },
    )
}

/// Release builds run osascript; debug builds never do. Decided here, in
/// Rust, from the build profile only.
const EXECUTE_OSASCRIPT: bool = !cfg!(debug_assertions);

fn uninstall_for_this_build(
    runner: &dyn CommandRunner,
    script: &str,
    pid: u32,
    choices: &NetworkChoices,
) -> Result<RescueRun, String> {
    rescue_uninstall_with(runner, EXECUTE_OSASCRIPT, script, pid, choices)
}

// --- Where the script is ---

/// Release: next to the running executable's bundle, i.e.
/// `<bundle>/Contents/MacOS/<bin>` -> `<bundle>/Contents/Resources/<script>`.
#[cfg(not(debug_assertions))]
fn script_location() -> Result<PathBuf, String> {
    let exe =
        std::env::current_exe().map_err(|e| format!("Could not find the running app: {}", e))?;
    bundled_script_path(&exe).ok_or_else(|| {
        format!(
            "The app is not running from an app bundle ({}), so its rescue script cannot be found.",
            exe.display()
        )
    })
}

/// Debug: the repo's copy.
#[cfg(debug_assertions)]
fn script_location() -> Result<PathBuf, String> {
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../recovery/ithurtswhenip-rescue.command"
    ));
    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}

/// `<bundle>/Contents/MacOS/<bin>` -> `<bundle>/Contents/Resources/<script>`;
/// None if the executable is not inside a bundle.
#[cfg_attr(all(debug_assertions, not(test)), allow(dead_code))]
fn bundled_script_path(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos_dir.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    Some(contents.join("Resources").join(SCRIPT_FILE))
}

/// The script must exist as a file; its path must be UTF-8 to pass as argv.
fn script_path_text(path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!(
            "The rescue script is missing: it should be at {}.",
            path.display()
        ));
    }
    path.to_str().map(str::to_string).ok_or_else(|| {
        format!(
            "The rescue script's path is not valid text: {}",
            path.display()
        )
    })
}

/// Display only (shown for the user to copy into Terminal; never executed).
fn terminal_command(script: &str) -> String {
    if script.contains(['"', '$', '`', '\\']) {
        format!("sudo bash '{}'", script.replace('\'', r"'\''"))
    } else {
        format!("sudo bash \"{}\"", script)
    }
}

// --- Arguments ---

/// Service names travel as one argv element each; the script refuses tabs
/// and line breaks, so they are refused here first (with any control char).
fn check_choice_names(choices: &NetworkChoices) -> Result<(), String> {
    for name in &choices.dhcp {
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(format!(
                "Not a usable service name: {:?}. Nothing was changed.",
                name
            ));
        }
    }
    Ok(())
}

/// `--network` items: dhcp picks in order, then restore-journal. With
/// `leave_when_empty`, no choice becomes an explicit `--network leave`.
fn network_args(choices: &NetworkChoices, leave_when_empty: bool) -> Vec<String> {
    let mut args = vec![];
    for name in &choices.dhcp {
        args.push("--network".to_string());
        args.push(format!("dhcp:{}", name));
    }
    if choices.restore_journal {
        args.push("--network".to_string());
        args.push("restore-journal".to_string());
    }
    if args.is_empty() && leave_when_empty {
        args.push("--network".to_string());
        args.push("leave".to_string());
    }
    args
}

/// Arguments to /bin/bash for a plan. `--wait-pid` only makes the plan show
/// the two stages; `--plan` changes nothing. Never `--yes`.
fn plan_args(script: &str, pid: u32, choices: &NetworkChoices) -> Vec<String> {
    let mut args = vec![
        script.to_string(),
        "--plan".to_string(),
        "--wait-pid".to_string(),
        pid.to_string(),
    ];
    args.extend(network_args(choices, false));
    args
}

/// The one place the uninstall command is built: program first, then argv.
/// The debug path returns exactly this; the release path runs exactly this.
fn uninstall_argv(script: &str, pid: u32, choices: &NetworkChoices) -> Vec<String> {
    let mut argv = vec![
        OSASCRIPT.to_string(),
        "-e".to_string(),
        APPLESCRIPT.to_string(),
        script.to_string(),
        "--uninstall".to_string(),
        "--yes".to_string(),
        "--wait-pid".to_string(),
        pid.to_string(),
    ];
    argv.extend(network_args(choices, true));
    argv
}

// --- Parsing (pure) ---

/// Lines split on LF or CR; empty pieces are skipped by the callers.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split(['\n', '\r'])
}

/// A plan as printed by the script, before anything is derived from it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PlanText {
    records: Vec<PlanRecord>,
    summary: Option<String>,
    backup_folder: Option<String>,
}

fn record_kind(word: &str) -> RecordKind {
    match word {
        "NOTE" => RecordKind::Note,
        "BACKUP" => RecordKind::Backup,
        "NETWORK" => RecordKind::Network,
        "STOP" => RecordKind::Stop,
        "QUIT" => RecordKind::Quit,
        "REMOVE" => RecordKind::Remove,
        "FORGET" => RecordKind::Forget,
        "LEFTOVER" => RecordKind::Leftover,
        _ => RecordKind::Other,
    }
}

/// Exactly 4 tab-separated fields with a known TYPE, or `Other` verbatim.
fn parse_record(line: &str) -> PlanRecord {
    let fields: Vec<&str> = line.split('\t').collect();
    let kind = record_kind(fields[0]);
    if fields.len() != 4 || kind == RecordKind::Other {
        return PlanRecord {
            kind: RecordKind::Other,
            sub_kind: String::new(),
            subject: String::new(),
            detail: line.to_string(),
        };
    }
    PlanRecord {
        kind,
        sub_kind: fields[1].to_string(),
        subject: fields[2].to_string(),
        detail: fields[3].to_string(),
    }
}

/// `# ...` lines are the header and summary; every other non-empty line is a
/// record.
fn parse_plan(stdout: &str) -> PlanText {
    let mut out = PlanText::default();
    for line in lines(stdout) {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let rest = rest.trim_start();
            if let Some(v) = rest.strip_prefix("summary:") {
                out.summary = Some(v.trim().to_string());
            } else if let Some(v) = rest.strip_prefix("backup folder:") {
                let v = v.trim();
                out.backup_folder = v.starts_with('/').then(|| v.to_string());
            }
            continue;
        }
        out.records.push(parse_record(line));
    }
    out
}

/// Value of `key=` in a record detail such as `ip=1.2.3.4 mask=… selected=no`.
/// The key must start the detail or follow a space; the value runs to the
/// next ` <one of keys>=` or the end, so a value with spaces stays whole.
fn detail_value<'a>(detail: &'a str, key: &str, keys: &[&str]) -> Option<&'a str> {
    let needle = format!("{}=", key);
    let start = detail
        .match_indices(&needle)
        .map(|(i, _)| i)
        .find(|&i| i == 0 || detail[..i].ends_with(' '))?
        + needle.len();
    let rest = &detail[start..];
    let end = keys
        .iter()
        .filter_map(|k| rest.find(&format!(" {}=", k)))
        .min()
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

const MANUAL_KEYS: [&str; 4] = ["ip", "mask", "router", "selected"];
const JOURNAL_KEYS: [&str; 3] = ["original", "current", "selected"];

fn network_records<'a>(
    text: &'a PlanText,
    sub_kind: &'a str,
) -> impl Iterator<Item = &'a PlanRecord> + 'a {
    text.records
        .iter()
        .filter(move |r| r.kind == RecordKind::Network && r.sub_kind == sub_kind)
}

fn manual_services(text: &PlanText) -> Vec<ManualService> {
    network_records(text, "manual")
        .map(|r| {
            let v = |k| {
                detail_value(&r.detail, k, &MANUAL_KEYS)
                    .unwrap_or("-")
                    .to_string()
            };
            ManualService {
                name: r.subject.clone(),
                ip: v("ip"),
                mask: v("mask"),
                router: v("router"),
            }
        })
        .collect()
}

fn journal_services(text: &PlanText) -> Vec<JournalService> {
    network_records(text, "journal")
        .map(|r| {
            let v = |k| detail_value(&r.detail, k, &JOURNAL_KEYS).unwrap_or("");
            JournalService {
                name: r.subject.clone(),
                original: v("original").to_string(),
                current: v("current").to_string(),
                selected: v("selected") == "yes",
            }
        })
        .collect()
}

/// The journal NOTE says "found: …" and at least one service is listed.
fn journal_usable(text: &PlanText) -> bool {
    let found = text.records.iter().any(|r| {
        r.kind == RecordKind::Note && r.subject == "journal" && r.detail.starts_with("found:")
    });
    found && network_records(text, "journal").next().is_some()
}

fn nothing_to_remove(text: &PlanText) -> bool {
    text.records
        .iter()
        .any(|r| r.kind == RecordKind::Note && r.subject == "-" && r.detail == "nothing to remove")
}

/// Splits the script's stderr into `rescue: note:` notices and other
/// `rescue:` messages (refusals). Usage text and blank lines are dropped.
fn script_messages(stderr: &str) -> (Vec<String>, Vec<String>) {
    let mut notices = vec![];
    let mut messages = vec![];
    for line in lines(stderr) {
        if let Some(n) = line.strip_prefix("rescue: note: ") {
            notices.push(n.trim().to_string());
        } else if let Some(m) = line.strip_prefix("rescue: ") {
            messages.push(m.trim().to_string());
        }
    }
    (notices, messages)
}

/// Stage A's output as returned by the AppleScript.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RunText {
    steps: Vec<StepLine>,
    backup_folder: Option<String>,
    log_path: Option<String>,
    stage_b_started: bool,
    messages: Vec<String>,
    exit: Option<i32>,
    osa_error: Option<(i32, String)>,
}

fn parse_step(line: &str) -> Option<StepLine> {
    let f: Vec<&str> = line.split('\t').collect();
    let get = |i: usize| f.get(i).copied().unwrap_or("").to_string();
    let step = |status, reason| StepLine {
        status,
        kind: get(1),
        sub_kind: get(2),
        subject: get(3),
        detail: get(4),
        reason,
    };
    match f[0] {
        "OK" => Some(step(StepStatus::Ok, String::new())),
        "FAILED" => Some(step(
            StepStatus::Failed,
            f.get(5..).map(|r| r.join("\t")).unwrap_or_default(),
        )),
        "VERIFY" => {
            let status = match f.get(1).copied() {
                Some("ok") => StepStatus::VerifyOk,
                Some("FAILED") => StepStatus::VerifyFailed,
                _ => return None,
            };
            Some(StepLine {
                status,
                kind: "VERIFY".to_string(),
                sub_kind: String::new(),
                subject: get(2),
                detail: f.get(3..).map(|r| r.join("\t")).unwrap_or_default(),
                reason: String::new(),
            })
        }
        _ => None,
    }
}

/// Steps are the OK/FAILED/VERIFY lines before the first `# result` line
/// (the final report repeats failures; those are not counted twice). The
/// backup folder and log come from the report; `# stage B process:` means
/// stage B was started. Plan records printed before the run are ignored.
fn parse_run_output(stdout: &str) -> RunText {
    let mut out = RunText::default();
    let mut in_report = false;
    let mut osa_lines: Option<Vec<String>> = None;
    for line in lines(stdout) {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(code) = line.strip_prefix(OSA_ERROR_MARKER) {
            out.osa_error = Some((code.trim().parse().unwrap_or(0), String::new()));
            osa_lines = Some(vec![]);
            continue;
        }
        if let Some(text) = osa_lines.as_mut() {
            text.push(line.trim().to_string());
            continue;
        }
        if let Some(code) = line.strip_prefix(EXIT_MARKER) {
            out.exit = code.trim().parse().ok();
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let rest = rest.trim_start();
            if rest == "result" || rest.starts_with("result:") {
                in_report = true;
            } else if let Some(v) = rest.strip_prefix("backup folder:") {
                if in_report {
                    let v = v.trim();
                    out.backup_folder = v.starts_with('/').then(|| v.to_string());
                }
            } else if let Some(v) = rest.strip_prefix("log:") {
                out.log_path = Some(v.trim().to_string());
            } else if rest.starts_with("stage B process:") {
                out.stage_b_started = true;
            } else if rest.starts_with("stopped before changing anything") {
                out.messages.push(rest.to_string());
            }
            continue;
        }
        if let Some(m) = line.strip_prefix("rescue: ") {
            out.messages.push(m.trim().to_string());
            continue;
        }
        if !in_report {
            if let Some(step) = parse_step(line) {
                out.steps.push(step);
            }
        }
    }
    if let (Some((code, _)), Some(text)) = (out.osa_error.clone(), osa_lines) {
        out.osa_error = Some((code, text.join(" ")));
    }
    out
}

/// Exit status mapping. An AppleScript error wins (-128 = cancelled), then
/// the script's own exit code, then osascript's.
fn map_outcome(out: &CmdOutput, text: &RunText) -> (Outcome, Option<String>) {
    if let Some((code, msg)) = &text.osa_error {
        if *code == USER_CANCELED {
            return (Outcome::Cancelled, None);
        }
        return (
            Outcome::Error,
            Some(format!(
                "macOS did not run the uninstall (AppleScript error {}: {}).",
                code, msg
            )),
        );
    }
    match text.exit {
        Some(0) => (Outcome::Ok, None),
        Some(1) => (Outcome::Partial, None),
        Some(2) => (Outcome::Refused, None),
        Some(n) => (
            Outcome::Error,
            Some(format!("The rescue script stopped with exit code {}.", n)),
        ),
        None => {
            let detail = [out.stderr.trim(), out.stdout.trim()]
                .iter()
                .find(|s| !s.is_empty())
                .map(|s| format!(": {}", s))
                .unwrap_or_default();
            let code = out
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            (
                Outcome::Error,
                Some(format!(
                    "No result came back from the rescue script (osascript exit {}){}",
                    code, detail
                )),
            )
        }
    }
}

fn run_result(argv: Vec<String>, out: &CmdOutput) -> RescueRun {
    let text = parse_run_output(&out.stdout);
    let (outcome, extra) = map_outcome(out, &text);
    let mut messages = text.messages;
    if let Some(m) = extra {
        messages.push(m);
    }
    if outcome == Outcome::Refused && messages.is_empty() {
        messages.push("The rescue script refused to run. Nothing was changed.".to_string());
    }
    RescueRun {
        outcome,
        executed: true,
        argv,
        exit_code: text.exit,
        steps: text.steps,
        backup_folder: text.backup_folder,
        log_path: text.log_path,
        // Only a run the script reports as ok/partial can have started stage B.
        stage_b_started: text.stage_b_started && matches!(outcome, Outcome::Ok | Outcome::Partial),
        messages,
    }
}

// --- Plan and uninstall, testable with a FakeRunner ---

enum PlanRun {
    Ready(PlanText, Vec<String>),
    Refused(String, Vec<String>),
}

fn run_plan(
    runner: &dyn CommandRunner,
    script: &str,
    pid: u32,
    choices: &NetworkChoices,
) -> Result<PlanRun, String> {
    let args = plan_args(script, pid, choices);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = runner
        .run(BASH, &argv)
        .map_err(|e| format!("Could not run the rescue script: {}", e))?;
    let (notices, messages) = script_messages(&out.stderr);
    match out.code {
        Some(0) => {
            let text = parse_plan(&out.stdout);
            if text.summary.is_none() {
                return Err("The rescue script's plan was not in the expected format.".to_string());
            }
            Ok(PlanRun::Ready(text, notices))
        }
        Some(2) => {
            let msg = if messages.is_empty() {
                "The rescue script refused these choices. Nothing was changed.".to_string()
            } else {
                messages.join(" ")
            };
            Ok(PlanRun::Refused(msg, notices))
        }
        Some(n) => Err(format!(
            "The rescue script could not make a plan (exit {}). {}",
            n,
            messages.join(" ")
        )
        .trim_end()
        .to_string()),
        None => Err("The rescue script was stopped before it finished its plan.".to_string()),
    }
}

fn rescue_plan_with(
    runner: &dyn CommandRunner,
    script: &str,
    pid: u32,
    choices: &NetworkChoices,
) -> Result<RescuePlan, String> {
    check_choice_names(choices)?;
    let mut plan = RescuePlan {
        script_path: script.to_string(),
        terminal_command: terminal_command(script),
        refused: None,
        records: vec![],
        summary: None,
        backup_folder: None,
        notices: vec![],
        nothing_to_remove: false,
        manual: vec![],
        journal: vec![],
        journal_usable: false,
    };
    match run_plan(runner, script, pid, choices)? {
        PlanRun::Refused(msg, notices) => {
            plan.refused = Some(msg);
            plan.notices = notices;
        }
        PlanRun::Ready(text, notices) => {
            plan.manual = manual_services(&text);
            plan.journal = journal_services(&text);
            plan.journal_usable = journal_usable(&text);
            plan.nothing_to_remove = nothing_to_remove(&text);
            plan.summary = text.summary;
            plan.backup_folder = text.backup_folder;
            plan.records = text.records;
            plan.notices = notices;
        }
    }
    Ok(plan)
}

/// DHCP picks must be enabled Manual services (`NETWORK manual`) or journal
/// services that still exist (`NETWORK journal`, current not "missing") in a
/// fresh plan; restore needs a usable journal.
fn check_choices_against(fresh: &PlanText, choices: &NetworkChoices) -> Result<(), String> {
    let manual = manual_services(fresh);
    let journal = journal_services(fresh);
    for name in &choices.dhcp {
        let is_manual = manual.iter().any(|m| &m.name == name);
        let is_journal = journal
            .iter()
            .any(|j| &j.name == name && !j.current.starts_with("missing"));
        if !is_manual && !is_journal {
            return Err(format!(
                "\"{}\" is not a service this uninstall can switch to DHCP (a fresh plan does not list it as an enabled service on Manual settings). Nothing was changed.",
                name
            ));
        }
    }
    if choices.restore_journal && !journal_usable(fresh) {
        return Err(
            "There is no usable record of the original network settings to restore. Nothing was changed."
                .to_string(),
        );
    }
    Ok(())
}

fn rescue_uninstall_with(
    runner: &dyn CommandRunner,
    execute: bool,
    script: &str,
    pid: u32,
    choices: &NetworkChoices,
) -> Result<RescueRun, String> {
    check_choice_names(choices)?;
    let fresh = match run_plan(runner, script, pid, &NetworkChoices::default())? {
        PlanRun::Ready(text, _) => text,
        PlanRun::Refused(msg, _) => {
            return Err(format!(
                "The rescue script would not make a fresh plan, so nothing was run: {}",
                msg
            ))
        }
    };
    check_choices_against(&fresh, choices)?;

    let argv = uninstall_argv(script, pid, choices);
    if !execute {
        return Ok(RescueRun::without_output(
            Outcome::NotExecuted,
            false,
            argv,
            "Debug build: not executed.".to_string(),
        ));
    }
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    match runner.run(&argv[0], &args) {
        Ok(out) => Ok(run_result(argv, &out)),
        Err(e) => Ok(RescueRun::without_output(
            Outcome::Error,
            true,
            argv,
            format!("Could not start osascript: {}", e),
        )),
    }
}

#[cfg(test)]
mod tests {
    // FakeRunner only: no test runs bash, osascript or the rescue script.
    use super::*;
    use crate::platform::runner::FakeRunner;

    const SCRIPT: &str =
        "/Applications/It Hurts When IP.app/Contents/Resources/ithurtswhenip-rescue.command";
    const PID: u32 = 4242;

    const G_EMPTY: &str = include_str!("../../../../recovery/tests/golden/plan-empty.txt");
    const G_FULL: &str = include_str!("../../../../recovery/tests/golden/plan-full.txt");
    const G_JOURNAL: &str = include_str!("../../../../recovery/tests/golden/plan-journal.txt");
    const G_JOURNAL_RESTORE: &str =
        include_str!("../../../../recovery/tests/golden/plan-journal-restore.txt");
    const G_LEGACY: &str = include_str!("../../../../recovery/tests/golden/plan-legacy-only.txt");
    const G_MIXED: &str = include_str!("../../../../recovery/tests/golden/plan-mixed.txt");
    const G_NO_JOURNAL: &str =
        include_str!("../../../../recovery/tests/golden/plan-no-journal.txt");

    const ALL_GOLDEN: [(&str, &str); 7] = [
        ("plan-empty", G_EMPTY),
        ("plan-full", G_FULL),
        ("plan-journal", G_JOURNAL),
        ("plan-journal-restore", G_JOURNAL_RESTORE),
        ("plan-legacy-only", G_LEGACY),
        ("plan-mixed", G_MIXED),
        ("plan-no-journal", G_NO_JOURNAL),
    ];

    const ODD_NAME: &str = "Lab \"Desk\", Jay's";

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
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

    fn plan_call(extra: &[&str]) -> (String, Vec<String>) {
        let mut args = s(&[SCRIPT, "--plan", "--wait-pid", "4242"]);
        args.extend(s(extra));
        (BASH.to_string(), args)
    }

    /// A minimal plan with one Manual service whose name has a space, a
    /// comma and both quote kinds.
    fn odd_name_plan() -> String {
        format!(
            "# It Hurts When IP rescue script, version 1\n\
             # mode: plan\n\
             # backup folder: /Users/Shared/ItHurtsWhenIP-backups/20261116T101500Z\n\
             # plan only: nothing has been changed\n\
             BACKUP\tsnapshot\tnetwork\tnetwork-before.txt\n\
             NETWORK\tmanual\t{}\tip=10.0.0.20 mask=255.255.255.0 router=- selected=no\n\
             NETWORK\tchoice\t-\tleave\n\
             REMOVE\tapp\t/Applications/It Hurts When IP.app\t-\n\
             # summary: backup=1 network-manual=1 stop=0 quit=0 remove=1 forget=0 skipped=0 leftover=0\n",
            ODD_NAME
        )
    }

    // --- the AppleScript ---

    #[test]
    fn applescript_is_exactly_the_reviewed_text() {
        let expected = "on run argv\n    set cmd to \"/bin/bash\"\n    repeat with a in argv\n        set cmd to cmd & space & quoted form of (contents of a)\n    end repeat\n    set cmd to cmd & \" 2>&1; echo \\\"IHWIP_EXIT=$?\\\"\"\n    try\n        return do shell script cmd with administrator privileges without altering line endings\n    on error errMsg number errNum\n        return \"IHWIP_OSA_ERROR=\" & errNum & linefeed & errMsg\n    end try\nend run";
        assert_eq!(APPLESCRIPT, expected);
    }

    // --- script location ---

    #[test]
    fn bundled_script_is_in_contents_resources() {
        let exe = Path::new("/Applications/It Hurts When IP.app/Contents/MacOS/it-hurts-when-ip");
        assert_eq!(
            bundled_script_path(exe),
            Some(PathBuf::from(SCRIPT)),
            "resolved from the running bundle"
        );
        let moved = Path::new("/Users/x/Desktop/Copy.app/Contents/MacOS/it-hurts-when-ip");
        assert_eq!(
            bundled_script_path(moved),
            Some(PathBuf::from(
                "/Users/x/Desktop/Copy.app/Contents/Resources/ithurtswhenip-rescue.command"
            ))
        );
        assert_eq!(
            bundled_script_path(Path::new("/usr/local/bin/it-hurts-when-ip")),
            None
        );
        assert_eq!(
            bundled_script_path(Path::new("/x/Other/MacOS/it-hurts-when-ip")),
            None
        );
    }

    #[test]
    fn missing_script_is_a_clear_error() {
        let err = script_path_text(Path::new("/nonexistent/ihwip/ithurtswhenip-rescue.command"))
            .unwrap_err();
        assert!(err.contains("rescue script is missing"), "{err}");
        assert!(err.contains("/nonexistent/ihwip/"), "{err}");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_script_location_is_the_repo_copy() {
        let path = script_location().unwrap();
        assert!(
            path.ends_with("recovery/ithurtswhenip-rescue.command"),
            "{path:?}"
        );
        assert!(script_path_text(&path).is_ok(), "repo copy exists");
    }

    #[test]
    fn terminal_command_is_display_text() {
        assert_eq!(
            terminal_command(SCRIPT),
            format!("sudo bash \"{}\"", SCRIPT)
        );
        assert_eq!(
            terminal_command("/tmp/a \"b\" c's/x.command"),
            "sudo bash '/tmp/a \"b\" c'\\''s/x.command'"
        );
    }

    // --- plan parsing against every golden file ---

    #[test]
    fn golden_record_counts_match_their_summary_lines() {
        for (name, text) in ALL_GOLDEN {
            let plan = parse_plan(text);
            let summary = plan.summary.as_deref().unwrap_or_else(|| panic!("{name}"));
            assert_eq!(count(&plan, RecordKind::Other), 0, "{name}");
            assert_eq!(
                count(&plan, RecordKind::Backup),
                summary_num(summary, "backup"),
                "{name}"
            );
            assert_eq!(
                manual_services(&plan).len(),
                summary_num(summary, "network-manual"),
                "{name}"
            );
            assert_eq!(
                count(&plan, RecordKind::Stop),
                summary_num(summary, "stop"),
                "{name}"
            );
            assert_eq!(
                count(&plan, RecordKind::Quit),
                summary_num(summary, "quit"),
                "{name}"
            );
            assert_eq!(
                count(&plan, RecordKind::Remove),
                summary_num(summary, "remove"),
                "{name}"
            );
            assert_eq!(
                count(&plan, RecordKind::Forget),
                summary_num(summary, "forget"),
                "{name}"
            );
            assert_eq!(
                count(&plan, RecordKind::Leftover),
                summary_num(summary, "leftover"),
                "{name}"
            );
        }
    }

    #[test]
    fn golden_total_record_counts() {
        let totals: Vec<(&str, usize)> = ALL_GOLDEN
            .iter()
            .map(|(n, t)| (*n, parse_plan(t).records.len()))
            .collect();
        assert_eq!(
            totals,
            vec![
                ("plan-empty", 3),
                ("plan-full", 38),
                ("plan-journal", 43),
                ("plan-journal-restore", 48),
                ("plan-legacy-only", 20),
                ("plan-mixed", 54),
                ("plan-no-journal", 38),
            ]
        );
    }

    #[test]
    fn golden_empty_is_nothing_to_remove() {
        let p = parse_plan(G_EMPTY);
        assert!(nothing_to_remove(&p));
        assert_eq!(p.backup_folder, None);
        assert_eq!(
            p.summary.as_deref(),
            Some("backup=0 network-manual=1 stop=0 quit=0 remove=0 forget=0 skipped=0 leftover=0")
        );
        assert_eq!(
            manual_services(&p),
            vec![ManualService {
                name: "Dante Primary".into(),
                ip: "192.168.10.21".into(),
                mask: "255.255.255.0".into(),
                router: "-".into(),
            }]
        );
        assert!(!journal_usable(&p));
    }

    #[test]
    fn golden_full_fields() {
        let p = parse_plan(G_FULL);
        assert!(!nothing_to_remove(&p));
        assert_eq!(
            p.backup_folder.as_deref(),
            Some("/Users/Shared/ItHurtsWhenIP-backups/20261115T120000Z")
        );
        assert_eq!(
            p.records[0],
            PlanRecord {
                kind: RecordKind::Backup,
                sub_kind: "snapshot".into(),
                subject: "network".into(),
                detail: "network-before.txt".into(),
            }
        );
        assert!(p.records.contains(&PlanRecord {
            kind: RecordKind::Remove,
            sub_kind: "app".into(),
            subject: "/Applications/It Hurts When IP.app".into(),
            detail: "-".into(),
        }));
        assert!(p.records.contains(&PlanRecord {
            kind: RecordKind::Stop,
            sub_kind: "launchd".into(),
            subject: "com.ithurtswhenip.helper".into(),
            detail: "loaded".into(),
        }));
        // A journal with 0 services is not usable for a restore.
        assert!(journal_services(&p).is_empty());
        assert!(!journal_usable(&p));
    }

    #[test]
    fn golden_journal_fields_incl_quoted_service_name() {
        let p = parse_plan(G_JOURNAL);
        assert!(journal_usable(&p));
        let j = journal_services(&p);
        assert_eq!(j.len(), 5);
        assert_eq!(
            j[1],
            JournalService {
                name: "Show \"A\", B".into(),
                original: "manual/10.0.0.5/255.255.255.0/10.0.0.1".into(),
                current: "dhcp/192.168.50.7/255.255.255.0/192.168.50.1".into(),
                selected: false,
            }
        );
        assert_eq!(j[4].name, "Gone Adapter");
        assert_eq!(j[4].current, "missing/-/-/-");
    }

    #[test]
    fn golden_journal_restore_planned_writes() {
        let p = parse_plan(G_JOURNAL_RESTORE);
        assert!(journal_services(&p).iter().all(|j| j.selected));
        let writes: Vec<(&str, &str, &str)> = p
            .records
            .iter()
            .filter(|r| {
                r.kind == RecordKind::Network
                    && ["setdhcp", "setmanual", "unchanged", "skip"].contains(&r.sub_kind.as_str())
            })
            .map(|r| (r.sub_kind.as_str(), r.subject.as_str(), r.detail.as_str()))
            .collect();
        assert_eq!(
            writes,
            vec![
                ("setdhcp", "Dante Primary", "source=journal"),
                (
                    "setmanual",
                    "Show \"A\", B",
                    "ip=10.0.0.5 mask=255.255.255.0 router=10.0.0.1 source=journal"
                ),
                (
                    "unchanged",
                    "Wi-Fi",
                    "already matches the recorded original"
                ),
                (
                    "setmanual",
                    "Thunderbolt Bridge",
                    "ip=169.254.1.1 mask=255.255.0.0 router=- source=journal"
                ),
                (
                    "skip",
                    "Gone Adapter",
                    "service no longer exists: not restored"
                ),
            ]
        );
        assert!(p.records.contains(&PlanRecord {
            kind: RecordKind::Network,
            sub_kind: "choice".into(),
            subject: "-".into(),
            detail: "restore-journal".into(),
        }));
    }

    #[test]
    fn golden_legacy_only_fields() {
        let p = parse_plan(G_LEGACY);
        assert!(!journal_usable(&p));
        let leftovers: Vec<&str> = p
            .records
            .iter()
            .filter(|r| r.kind == RecordKind::Leftover)
            .map(|r| r.subject.as_str())
            .collect();
        assert_eq!(leftovers, vec!["/Applications/IP Switcher.app"]);
        assert_eq!(count(&p, RecordKind::Forget), 2);
    }

    #[test]
    fn golden_mixed_stops_both_helpers() {
        let p = parse_plan(G_MIXED);
        let stops: Vec<&str> = p
            .records
            .iter()
            .filter(|r| r.kind == RecordKind::Stop)
            .map(|r| r.subject.as_str())
            .collect();
        assert_eq!(
            stops,
            vec!["com.ithurtswhenip.helper", "com.ipswitcher.helper"]
        );
    }

    #[test]
    fn golden_no_journal_manual_services() {
        let p = parse_plan(G_NO_JOURNAL);
        assert!(!journal_usable(&p));
        assert_eq!(
            manual_services(&p),
            vec![
                ManualService {
                    name: "USB 10/100/1000 LAN".into(),
                    ip: "10.1.2.3".into(),
                    mask: "255.255.0.0".into(),
                    router: "10.1.0.1".into(),
                },
                ManualService {
                    name: "Dante Primary".into(),
                    ip: "192.168.10.21".into(),
                    mask: "255.255.255.0".into(),
                    router: "-".into(),
                },
            ]
        );
    }

    #[test]
    fn unknown_and_malformed_records_are_kept_verbatim_as_other() {
        let text = "# mode: plan\r\n\
                    FUTURE\tthing\tx\ty\r\n\
                    NOTE\tonly-three\tfields\n\
                    REMOVE\tfile\t/a\t-\textra\n\
                    just text\n\
                    \n\
                    REMOVE\tfile\t/Library/x\t-\r\n\
                    # summary: backup=0\n";
        let p = parse_plan(text);
        let other: Vec<&str> = p
            .records
            .iter()
            .filter(|r| r.kind == RecordKind::Other)
            .map(|r| r.detail.as_str())
            .collect();
        assert_eq!(
            other,
            vec![
                "FUTURE\tthing\tx\ty",
                "NOTE\tonly-three\tfields",
                "REMOVE\tfile\t/a\t-\textra",
                "just text"
            ]
        );
        assert_eq!(count(&p, RecordKind::Remove), 1);
        assert_eq!(p.records.last().unwrap().subject, "/Library/x");
        assert_eq!(p.summary.as_deref(), Some("backup=0"));
    }

    #[test]
    fn detail_values_keep_spaces_and_tolerate_missing_keys() {
        let keys = MANUAL_KEYS;
        let d = "ip=10.0.0.1 mask=255.0.0.0 router=- selected=no";
        assert_eq!(detail_value(d, "ip", &keys), Some("10.0.0.1"));
        assert_eq!(detail_value(d, "router", &keys), Some("-"));
        assert_eq!(detail_value(d, "selected", &keys), Some("no"));
        let odd = "ip=not an ip mask=x selected=yes";
        assert_eq!(detail_value(odd, "ip", &keys), Some("not an ip"));
        assert_eq!(detail_value(odd, "router", &keys), None);
        // "router=" inside another value does not count as the key.
        assert_eq!(detail_value("ip=xrouter=1", "router", &keys), None);
    }

    // --- rescue_plan ---

    #[test]
    fn plan_runs_bash_with_plan_flags_and_choices_only() {
        let fake = FakeRunner::new();
        fake.push(Some(0), G_JOURNAL_RESTORE, "");
        let plan = rescue_plan_with(
            &fake,
            SCRIPT,
            PID,
            &NetworkChoices {
                dhcp: vec!["Wi-Fi".into()],
                restore_journal: true,
            },
        )
        .unwrap();
        assert_eq!(
            fake.calls(),
            vec![plan_call(&[
                "--network",
                "dhcp:Wi-Fi",
                "--network",
                "restore-journal"
            ])]
        );
        assert!(!fake.calls()[0].1.contains(&"--yes".to_string()));
        assert!(!fake.calls()[0].1.contains(&"--uninstall".to_string()));
        assert_eq!(plan.refused, None);
        assert_eq!(plan.script_path, SCRIPT);
        assert_eq!(plan.terminal_command, format!("sudo bash \"{}\"", SCRIPT));
        assert!(plan.journal_usable);
        assert_eq!(plan.journal.len(), 5);
        assert_eq!(plan.manual.len(), 1);
        assert_eq!(plan.records.len(), 48);
    }

    #[test]
    fn plan_without_choices_has_no_network_args() {
        let fake = FakeRunner::new();
        fake.push(Some(0), G_FULL, "");
        rescue_plan_with(&fake, SCRIPT, PID, &NetworkChoices::default()).unwrap();
        assert_eq!(fake.calls(), vec![plan_call(&[])]);
    }

    #[test]
    fn plan_exit_2_is_refused_with_the_scripts_own_message() {
        let fake = FakeRunner::new();
        fake.push(
            Some(2),
            "",
            "rescue: note: IHWIP_ROOT is not set: ignoring IHWIP_* test overrides\n\
             rescue: --network dhcp:Gone: there is no network service named exactly \"Gone\" (see the NETWORK lines of --plan). Nothing has been changed.\n",
        );
        let plan = rescue_plan_with(
            &fake,
            SCRIPT,
            PID,
            &NetworkChoices {
                dhcp: vec!["Gone".into()],
                restore_journal: false,
            },
        )
        .unwrap();
        assert_eq!(
            plan.refused.as_deref(),
            Some("--network dhcp:Gone: there is no network service named exactly \"Gone\" (see the NETWORK lines of --plan). Nothing has been changed.")
        );
        assert_eq!(
            plan.notices,
            vec!["IHWIP_ROOT is not set: ignoring IHWIP_* test overrides"]
        );
        assert!(plan.records.is_empty());
    }

    #[test]
    fn plan_failures_are_errors() {
        let fake = FakeRunner::new();
        fake.push(Some(1), "", "boom\n");
        assert!(rescue_plan_with(&fake, SCRIPT, PID, &NetworkChoices::default()).is_err());

        let fake = FakeRunner::new();
        fake.push(Some(0), "not a plan\n", "");
        let err = rescue_plan_with(&fake, SCRIPT, PID, &NetworkChoices::default()).unwrap_err();
        assert!(err.contains("expected format"), "{err}");

        let fake = FakeRunner::new(); // empty queue -> io error
        let err = rescue_plan_with(&fake, SCRIPT, PID, &NetworkChoices::default()).unwrap_err();
        assert!(err.starts_with("Could not run the rescue script"), "{err}");
    }

    #[test]
    fn bad_service_names_are_refused_before_anything_runs() {
        for bad in ["", "Tab\tName", "Line\nBreak", "CR\rName"] {
            let fake = FakeRunner::new();
            let choices = NetworkChoices {
                dhcp: vec![bad.to_string()],
                restore_journal: false,
            };
            assert!(
                rescue_plan_with(&fake, SCRIPT, PID, &choices).is_err(),
                "{bad:?}"
            );
            assert!(rescue_uninstall_with(&fake, true, SCRIPT, PID, &choices).is_err());
            assert!(fake.calls().is_empty(), "{bad:?}");
        }
    }

    // --- rescue_uninstall: argv, validation, debug ---

    #[test]
    fn uninstall_argv_without_network_choices_says_leave() {
        let fake = FakeRunner::new();
        fake.push(Some(0), G_FULL, "")
            .push(Some(0), "IHWIP_EXIT=0\n", "");
        let run =
            rescue_uninstall_with(&fake, true, SCRIPT, PID, &NetworkChoices::default()).unwrap();
        let expected = s(&[
            OSASCRIPT,
            "-e",
            APPLESCRIPT,
            SCRIPT,
            "--uninstall",
            "--yes",
            "--wait-pid",
            "4242",
            "--network",
            "leave",
        ]);
        assert_eq!(run.argv, expected);
        assert_eq!(
            fake.calls(),
            vec![
                plan_call(&[]),
                (OSASCRIPT.to_string(), expected[1..].to_vec())
            ]
        );
        assert_eq!(run.outcome, Outcome::Ok);
        assert!(run.executed);
    }

    #[test]
    fn uninstall_dhcp_pick_with_space_comma_and_quotes_is_one_argv_element() {
        let fake = FakeRunner::new();
        fake.push(Some(0), &odd_name_plan(), "")
            .push(Some(0), "IHWIP_EXIT=0\n", "");
        let choices = NetworkChoices {
            dhcp: vec![ODD_NAME.to_string()],
            restore_journal: false,
        };
        let run = rescue_uninstall_with(&fake, true, SCRIPT, PID, &choices).unwrap();
        let calls = fake.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], plan_call(&[]), "fresh plan has no choices");
        assert_eq!(calls[1].0, OSASCRIPT);
        assert_eq!(
            calls[1].1,
            s(&[
                "-e",
                APPLESCRIPT,
                SCRIPT,
                "--uninstall",
                "--yes",
                "--wait-pid",
                "4242",
                "--network",
                "dhcp:Lab \"Desk\", Jay's",
            ])
        );
        assert_eq!(run.argv[1..], calls[1].1[..]);
    }

    #[test]
    fn uninstall_journal_restore_argv() {
        let fake = FakeRunner::new();
        fake.push(Some(0), G_JOURNAL, "")
            .push(Some(0), "IHWIP_EXIT=0\n", "");
        let choices = NetworkChoices {
            dhcp: vec!["Dante Primary".into()],
            restore_journal: true,
        };
        rescue_uninstall_with(&fake, true, SCRIPT, PID, &choices).unwrap();
        assert_eq!(
            fake.calls()[1].1,
            s(&[
                "-e",
                APPLESCRIPT,
                SCRIPT,
                "--uninstall",
                "--yes",
                "--wait-pid",
                "4242",
                "--network",
                "dhcp:Dante Primary",
                "--network",
                "restore-journal",
            ])
        );

        let fake = FakeRunner::new();
        fake.push(Some(0), G_JOURNAL, "")
            .push(Some(0), "IHWIP_EXIT=0\n", "");
        let choices = NetworkChoices {
            dhcp: vec![],
            restore_journal: true,
        };
        let run = rescue_uninstall_with(&fake, true, SCRIPT, PID, &choices).unwrap();
        assert_eq!(
            run.argv[8..],
            s(&["--network", "restore-journal"])[..],
            "only restore-journal, no leave"
        );
    }

    #[test]
    fn service_not_in_the_fresh_plan_is_rejected_without_osascript() {
        // Not listed at all; journal service that no longer exists; wrong case.
        for name in ["Ethernet", "Gone Adapter", "dante primary"] {
            let fake = FakeRunner::new();
            fake.push(Some(0), G_JOURNAL, "");
            let choices = NetworkChoices {
                dhcp: vec![name.to_string()],
                restore_journal: false,
            };
            let err = rescue_uninstall_with(&fake, true, SCRIPT, PID, &choices).unwrap_err();
            assert!(err.contains("Nothing was changed"), "{err}");
            assert_eq!(fake.calls(), vec![plan_call(&[])], "{name}: no osascript");
        }
        // A journal service that still exists may be picked.
        let fake = FakeRunner::new();
        fake.push(Some(0), G_JOURNAL, "")
            .push(Some(0), "IHWIP_EXIT=0\n", "");
        let choices = NetworkChoices {
            dhcp: vec!["Show \"A\", B".into()],
            restore_journal: false,
        };
        assert!(rescue_uninstall_with(&fake, true, SCRIPT, PID, &choices).is_ok());
    }

    #[test]
    fn restore_without_a_usable_journal_is_rejected_without_osascript() {
        for plan in [G_NO_JOURNAL, G_FULL, G_LEGACY] {
            let fake = FakeRunner::new();
            fake.push(Some(0), plan, "");
            let choices = NetworkChoices {
                dhcp: vec![],
                restore_journal: true,
            };
            let err = rescue_uninstall_with(&fake, true, SCRIPT, PID, &choices).unwrap_err();
            assert!(err.contains("no usable record"), "{err}");
            assert_eq!(fake.calls(), vec![plan_call(&[])]);
        }
    }

    #[test]
    fn refused_or_failed_fresh_plan_never_reaches_osascript() {
        let fake = FakeRunner::new();
        fake.push(Some(2), "", "rescue: IHWIP_ROOT must not be /\n");
        let err = rescue_uninstall_with(&fake, true, SCRIPT, PID, &NetworkChoices::default())
            .unwrap_err();
        assert!(err.contains("IHWIP_ROOT must not be /"), "{err}");
        assert_eq!(fake.calls().len(), 1);

        let fake = FakeRunner::new();
        fake.push(Some(1), "", "");
        assert!(
            rescue_uninstall_with(&fake, true, SCRIPT, PID, &NetworkChoices::default()).is_err()
        );
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn not_executing_returns_the_same_argv_and_never_runs_osascript() {
        let fake = FakeRunner::new();
        fake.push(Some(0), &odd_name_plan(), "");
        let choices = NetworkChoices {
            dhcp: vec![ODD_NAME.to_string()],
            restore_journal: false,
        };
        let run = rescue_uninstall_with(&fake, false, SCRIPT, PID, &choices).unwrap();
        assert_eq!(run.outcome, Outcome::NotExecuted);
        assert!(!run.executed);
        assert!(!run.quit_required());
        assert_eq!(run.argv, uninstall_argv(SCRIPT, PID, &choices));
        assert_eq!(fake.calls(), vec![plan_call(&[])]);
        assert!(fake.calls().iter().all(|(p, _)| p != OSASCRIPT));
    }

    /// The public command's path in a debug build: osascript is never run.
    #[cfg(debug_assertions)]
    #[test]
    fn public_debug_path_never_runs_osascript() {
        assert!(!EXECUTE_OSASCRIPT);
        let fake = FakeRunner::new();
        fake.push(Some(0), G_FULL, "");
        let run = uninstall_for_this_build(&fake, SCRIPT, PID, &NetworkChoices::default()).unwrap();
        assert_eq!(run.outcome, Outcome::NotExecuted);
        assert_eq!(run.argv[0], OSASCRIPT);
        assert_eq!(fake.calls(), vec![plan_call(&[])]);
    }

    // --- result lines and exit mapping ---

    /// Stage A output as the script prints it (plan, steps, report, stage B
    /// lines), then the AppleScript's exit marker. One removal failed.
    const STAGE_A_PARTIAL: &str = "# It Hurts When IP rescue script, version 1\n\
# mode: uninstall\n\
# backup folder: /Users/Shared/ItHurtsWhenIP-backups/20261116T101500Z\n\
# nothing has been changed yet\n\
BACKUP\tsnapshot\tnetwork\tnetwork-before.txt\n\
BACKUP\tfile\t/Library/LaunchDaemons/com.ithurtswhenip.helper.plist\tsystem/Library/LaunchDaemons/com.ithurtswhenip.helper.plist\n\
NETWORK\tmanual\tLab \"Desk\", Jay's\tip=10.0.0.20 mask=255.255.255.0 router=- selected=yes\n\
NETWORK\tsetdhcp\tLab \"Desk\", Jay's\tsource=pick\n\
NETWORK\tchoice\t-\tdhcp\n\
NOTE\tinfo\twait-pid\ttwo stages: stage B waits for process 4242 to exit (up to 300 s), then removes the app bundle and per-user data\n\
STOP\tlaunchd\tcom.ithurtswhenip.helper\tloaded\n\
REMOVE\tfile\t/Library/LaunchDaemons/com.ithurtswhenip.helper.plist\t-\n\
REMOVE\tfile\t/var/log/com.ithurtswhenip.helper.log\t-\n\
REMOVE\tapp\t/Applications/It Hurts When IP.app\t-\n\
FORGET\treceipt\tcom.ithurtswhenip.pkg\t-\n\
# summary: backup=2 network-manual=1 stop=1 quit=0 remove=3 forget=1 skipped=0 leftover=0\n\
OK\tBACKUP\tsnapshot\tnetwork\tnetwork-before.txt\n\
OK\tBACKUP\tfile\t/Library/LaunchDaemons/com.ithurtswhenip.helper.plist\tsystem/Library/LaunchDaemons/com.ithurtswhenip.helper.plist\n\
OK\tNETWORK\tsetdhcp\tLab \"Desk\", Jay's\tsource=pick\n\
VERIFY\tok\tnetwork:Lab \"Desk\", Jay's\tnow=dhcp/10.0.0.20/255.255.255.0/10.0.0.1\n\
OK\tSTOP\tlaunchd\tcom.ithurtswhenip.helper\tstopped (bootout)\n\
OK\tREMOVE\tfile\t/Library/LaunchDaemons/com.ithurtswhenip.helper.plist\t-\n\
FAILED\tREMOVE\tfile\t/var/log/com.ithurtswhenip.helper.log\t-\tcould not remove: Operation not permitted\n\
OK\tFORGET\treceipt\tcom.ithurtswhenip.pkg\t-\n\
VERIFY\tok\t/Library/LaunchDaemons/com.ithurtswhenip.helper.plist\n\
VERIFY\tFAILED\t/var/log/com.ithurtswhenip.helper.log\n\
VERIFY\tok\tlaunchd:com.ithurtswhenip.helper\n\
VERIFY\tok\treceipt:com.ithurtswhenip.pkg\n\
# result: stage A\n\
# backup folder: /Users/Shared/ItHurtsWhenIP-backups/20261116T101500Z-2\n\
# log: /Users/Shared/ItHurtsWhenIP-backups/20261116T101500Z-2/rescue.log\n\
# failed steps: 1; still present after verify: 1\n\
FAILED\tREMOVE\tfile\t/var/log/com.ithurtswhenip.helper.log\t-\tcould not remove: Operation not permitted\n\
VERIFY\tFAILED\t/var/log/com.ithurtswhenip.helper.log\n\
# stage B: waits for process 4242 to exit (up to 300 s), then removes the app bundle and per-user data; its result is appended to /Users/Shared/ItHurtsWhenIP-backups/20261116T101500Z-2/rescue.log\n\
# stage B process: 4243\n\
IHWIP_EXIT=1\n";

    fn osa(stdout: &str) -> CmdOutput {
        CmdOutput {
            code: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    #[test]
    fn stage_a_result_lines_are_parsed_once() {
        let t = parse_run_output(STAGE_A_PARTIAL);
        assert_eq!(t.exit, Some(1));
        assert_eq!(t.osa_error, None);
        assert!(t.stage_b_started);
        assert_eq!(
            t.backup_folder.as_deref(),
            Some("/Users/Shared/ItHurtsWhenIP-backups/20261116T101500Z-2"),
            "the report's folder, not the plan's guess"
        );
        assert_eq!(
            t.log_path.as_deref(),
            Some("/Users/Shared/ItHurtsWhenIP-backups/20261116T101500Z-2/rescue.log")
        );
        assert_eq!(t.steps.len(), 12, "report repeats are not counted again");
        let failed: Vec<&StepLine> = t
            .steps
            .iter()
            .filter(|l| matches!(l.status, StepStatus::Failed | StepStatus::VerifyFailed))
            .collect();
        assert_eq!(failed.len(), 2);
        assert_eq!(
            *failed[0],
            StepLine {
                status: StepStatus::Failed,
                kind: "REMOVE".into(),
                sub_kind: "file".into(),
                subject: "/var/log/com.ithurtswhenip.helper.log".into(),
                detail: "-".into(),
                reason: "could not remove: Operation not permitted".into(),
            }
        );
        assert_eq!(
            t.steps[2],
            StepLine {
                status: StepStatus::Ok,
                kind: "NETWORK".into(),
                sub_kind: "setdhcp".into(),
                subject: "Lab \"Desk\", Jay's".into(),
                detail: "source=pick".into(),
                reason: String::new(),
            }
        );
        assert_eq!(
            t.steps[3],
            StepLine {
                status: StepStatus::VerifyOk,
                kind: "VERIFY".into(),
                sub_kind: String::new(),
                subject: "network:Lab \"Desk\", Jay's".into(),
                detail: "now=dhcp/10.0.0.20/255.255.255.0/10.0.0.1".into(),
                reason: String::new(),
            }
        );
        assert_eq!(t.steps[9].status, StepStatus::VerifyFailed);
        assert_eq!(t.steps[9].detail, "");
    }

    #[test]
    fn partial_run_with_stage_b_requires_quit() {
        let run = run_result(vec![], &osa(STAGE_A_PARTIAL));
        assert_eq!(run.outcome, Outcome::Partial);
        assert_eq!(run.exit_code, Some(1));
        assert!(run.stage_b_started);
        assert!(run.quit_required());
    }

    #[test]
    fn exit_status_mapping() {
        let cases = [
            ("OK\tSTOP\tlaunchd\tx\tstopped\n# result: stage A\n# stage B process: 9\nIHWIP_EXIT=0\n", Outcome::Ok, true),
            ("FAILED\tSTOP\tlaunchd\tx\tloaded\tstill loaded\n# result: stage A\n# stage B process: 9\nIHWIP_EXIT=1\n", Outcome::Partial, true),
            ("rescue: --network dhcp:X: that service is disabled; only enabled services can be switched. Nothing has been changed.\nIHWIP_EXIT=2\n", Outcome::Refused, false),
            ("IHWIP_OSA_ERROR=-128\nUser canceled.\n", Outcome::Cancelled, false),
            ("IHWIP_OSA_ERROR=-60005\nThe administrator user name or password was incorrect.\n", Outcome::Error, false),
            ("/bin/bash: x: No such file or directory\nIHWIP_EXIT=127\n", Outcome::Error, false),
            ("", Outcome::Error, false),
        ];
        for (stdout, outcome, quit) in cases {
            let run = run_result(vec![], &osa(stdout));
            assert_eq!(run.outcome, outcome, "{stdout:?}");
            assert_eq!(run.quit_required(), quit, "{stdout:?}");
        }

        let refused = run_result(vec![], &osa(cases[2].0));
        assert_eq!(
            refused.messages,
            vec!["--network dhcp:X: that service is disabled; only enabled services can be switched. Nothing has been changed."]
        );
        let cancelled = run_result(vec![], &osa(cases[3].0));
        assert!(cancelled.messages.is_empty());
        assert!(cancelled.steps.is_empty());
        let wrong_pw = run_result(vec![], &osa(cases[4].0));
        assert!(
            wrong_pw.messages[0].contains("-60005"),
            "{:?}",
            wrong_pw.messages
        );
        assert!(wrong_pw.messages[0].contains("password was incorrect"));
    }

    #[test]
    fn stage_b_line_never_forces_a_quit_unless_ok_or_partial() {
        // Even if a stage B line showed up, refused / error never quits the app.
        for code in ["2", "3"] {
            let out = format!("# stage B process: 9\nIHWIP_EXIT={}\n", code);
            let run = run_result(vec![], &osa(&out));
            assert!(!run.stage_b_started, "exit {code}");
            assert!(!run.quit_required());
        }
    }

    #[test]
    fn osascript_failure_without_marker_is_an_error() {
        let out = CmdOutput {
            code: Some(1),
            stdout: String::new(),
            stderr: "0:5: syntax error: A unknown token can't go here. (-2740)\n".into(),
        };
        let run = run_result(vec![], &out);
        assert_eq!(run.outcome, Outcome::Error);
        assert!(
            run.messages[0].contains("osascript exit 1"),
            "{:?}",
            run.messages
        );
        assert!(run.messages[0].contains("syntax error"));

        let fake = FakeRunner::new();
        fake.push(Some(0), G_FULL, ""); // osascript call then gets an io error
        let run =
            rescue_uninstall_with(&fake, true, SCRIPT, PID, &NetworkChoices::default()).unwrap();
        assert_eq!(run.outcome, Outcome::Error);
        assert!(run.messages[0].starts_with("Could not start osascript"));
        assert!(!run.quit_required());
    }

    #[test]
    fn stopped_before_changing_anything_is_a_message_and_no_stage_b() {
        let out =
            "FAILED\tBACKUP\tdir\t/Users/Shared/ItHurtsWhenIP-backups/T\t-\tcould not create it\n\
                   # stopped before changing anything: no backup folder\n\
                   IHWIP_EXIT=1\n";
        let run = run_result(vec![], &osa(out));
        assert_eq!(run.outcome, Outcome::Partial);
        assert!(!run.stage_b_started);
        assert_eq!(run.steps.len(), 1);
        assert_eq!(
            run.messages,
            vec!["stopped before changing anything: no backup folder"]
        );
        assert_eq!(run.backup_folder, None);
    }

    #[test]
    fn run_output_with_cr_line_endings_still_parses() {
        let out = STAGE_A_PARTIAL.replace('\n', "\r");
        let t = parse_run_output(&out);
        assert_eq!(t.exit, Some(1));
        assert_eq!(t.steps.len(), 12);
        assert!(t.stage_b_started);
    }
}
