//! Claude Code status: the statusLine text (same output as the owner's
//! statusline.mjs) and the per-pane status file ANVIL reads for its tab badge.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fsutil::atomic_write;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const MAGENTA: &str = "\x1b[35m";

/// The fields of Claude Code's statusLine payload that ANVIL uses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Payload {
    pub model: Option<String>,
    pub dir: Option<String>,
    pub context_pct: Option<f64>,
    pub five_hour_pct: Option<f64>,
    pub five_hour_resets_at: Option<f64>,
    pub seven_day_pct: Option<f64>,
    pub agent: Option<String>,
}

/// No payload Claude Code sends is anywhere near this: the helper reads stdin
/// until EOF, so the ceiling is what a misbehaving writer can make it buffer.
pub const MAX_PAYLOAD_BYTES: u64 = 1024 * 1024;

/// A name, directory or model is a label for one status line.
const MAX_TEXT_CHARS: usize = 256;

/// The text of a payload field as a single printable label. A JSON string can
/// carry any control character (ESC as `\u001b`), and a model or agent name
/// comes from settings or agent definitions that a repository can supply; it is
/// printed into the terminal, so it must not bring escape sequences of its own.
/// The length cap keeps the status file below the size the reader accepts.
fn label(text: &str) -> Option<String> {
    let text: String = text.chars().filter(|ch| printable(*ch)).take(MAX_TEXT_CHARS).collect();
    (!text.is_empty()).then_some(text)
}

/// Same formatting-control boundary as the repository labels in the UI:
/// invisible direction overrides must not spoof a model, agent or branch.
fn printable(ch: char) -> bool {
    !ch.is_control()
        && !matches!(ch, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// The longest working directory kept. A path cut short names another
/// directory (the branch is read from it), so a longer one is dropped whole;
/// the cap also keeps the status record below the size the reader accepts.
const MAX_PATH_CHARS: usize = 8192;

/// A working directory is a lookup target, not just a label. Reject controls
/// and excessive length without deleting or truncating bytes: rewriting the
/// path could inspect a different repository.
fn path_label(text: &str) -> Option<String> {
    (!text.is_empty() && !text.chars().any(char::is_control) && text.chars().count() <= MAX_PATH_CHARS)
        .then(|| text.to_owned())
}

impl Payload {
    /// None when the input is not JSON (print nothing, like the script).
    pub fn parse(text: &str) -> Option<Payload> {
        let v: Value = serde_json::from_str(text).ok()?;
        let s = |p: &str| v.pointer(p).and_then(Value::as_str).and_then(label);
        let d = |p: &str| v.pointer(p).and_then(Value::as_str).and_then(path_label);
        let n = |p: &str| v.pointer(p).and_then(Value::as_f64);
        Some(Payload {
            model: s("/model/display_name"),
            dir: d("/workspace/current_dir").or_else(|| d("/cwd")),
            context_pct: n("/context_window/used_percentage"),
            five_hour_pct: n("/rate_limits/five_hour/used_percentage"),
            five_hour_resets_at: n("/rate_limits/five_hour/resets_at"),
            seven_day_pct: n("/rate_limits/seven_day/used_percentage"),
            agent: s("/agent/name"),
        })
    }
}

/// JavaScript `Math.round` (halves round up).
pub fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

fn heat(pct: f64) -> &'static str {
    if pct >= 85.0 {
        RED
    } else if pct >= 60.0 {
        YELLOW
    } else {
        GREEN
    }
}

fn bar(pct: f64) -> String {
    let filled = js_round(pct / 10.0).clamp(0, 10) as usize;
    format!("{}{}", "▓".repeat(filled), "░".repeat(10 - filled))
}

/// `↺ 2ч5м`, or None when the timestamp is not a plausible unix time.
/// Accepts seconds or milliseconds, like `quota::time::epoch_from_json`: the
/// same provider field feeds both, and one of them silently dropping the hint
/// while the other prints it would be a contradiction the user cannot see.
fn resets_in(resets_at_secs: f64, now_ms: i64) -> Option<String> {
    let secs = if resets_at_secs >= 1e12 { resets_at_secs / 1000.0 } else { resets_at_secs };
    if !(1_000_000_000.0..=4_000_000_000.0).contains(&secs) {
        return None;
    }
    let left = ((secs * 1000.0) as i64 - now_ms).clamp(0, 7 * 86_400_000);
    let h = left / 3_600_000;
    let m = (left % 3_600_000) / 60_000;
    Some(if h > 0 { format!("{h}ч{m}м") } else { format!("{m}м") })
}

/// Percentages outside 0-100 (a broken or hostile payload) are clamped.
/// The tab badge reads the same fields, so the clamp has to be reachable from
/// there too: otherwise one payload prints `150%` in the badge and `100%` in
/// the status line.
pub fn clamp_pct(pct: f64) -> f64 {
    pct.clamp(0.0, 100.0)
}

/// Last path component, accepting `/` and `\` and ignoring trailing separators
/// (Node's `path.basename` on Windows).
pub fn basename(dir: &str) -> &str {
    let trimmed = dir.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

use crate::config::ClaudeLineFields;

/// The statusLine text. `branch` comes from git (see `git_branch`).
pub fn format_line(p: &Payload, branch: Option<&str>, now_ms: i64) -> String {
    format_line_with(p, branch, now_ms, &ClaudeLineFields::default())
}

/// The statusLine text with only the pieces `fields` allows.
pub fn format_line_with(p: &Payload, branch: Option<&str>, now_ms: i64, fields: &ClaudeLineFields) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(model) = p.model.as_ref().filter(|_| fields.model) {
        parts.push(format!("{CYAN}[{model}]{RESET}"));
    }
    if let Some(dir) = p.dir.as_ref().filter(|_| fields.dir) {
        if let Some(dir) = label(basename(dir)) {
            parts.push(dir);
        }
    }
    if let Some(branch) = branch.filter(|_| fields.branch).and_then(label) {
        let color = if branch == "main" || branch == "master" { RED } else { GREEN };
        parts.push(format!("{color}{branch}{RESET}"));
    }
    if let Some(ctx) = p.context_pct.filter(|_| fields.context).map(clamp_pct) {
        let c = heat(ctx);
        parts.push(format!("{c}{}{RESET} {c}{}%{RESET}", bar(ctx), js_round(ctx)));
    }
    if let Some(five) = p.five_hour_pct.filter(|_| fields.five_hour).map(clamp_pct) {
        let c = heat(five);
        let mut text = format!("5h: {c}{}%{RESET}", js_round(five));
        if five >= 60.0 {
            if let Some(reset) = p.five_hour_resets_at.and_then(|resets| resets_in(resets, now_ms)) {
                text.push_str(&format!(" {DIM}↺ {reset}{RESET}"));
            }
        }
        parts.push(text);
    }
    if let Some(week) = p.seven_day_pct.filter(|_| fields.seven_day).map(clamp_pct) {
        parts.push(format!("7d: {}{}%{RESET}", heat(week), js_round(week)));
    }
    if let Some(agent) = p.agent.as_ref().filter(|_| fields.agent) {
        parts.push(format!("{MAGENTA}{agent}{RESET}"));
    }
    parts.join(&format!("{DIM} | {RESET}"))
}

/// `git rev-parse --abbrev-ref HEAD` in `dir`, killed after `timeout`.
///
/// Delegates to the guarded Git entry point so a crafted network gitfile or
/// repository path can never be followed here either; the helper keeps its
/// contract of answering with a branch name or `None`.
pub fn git_branch(dir: &Path, timeout: Duration) -> Option<String> {
    crate::git::branch_at(dir, timeout)
}

/// What ANVIL shows on the tab. Written by anvil-claude-status, read by ANVIL.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusRecord {
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub context_pct: Option<f64>,
    pub five_hour_pct: Option<f64>,
    pub five_hour_resets_at: Option<f64>,
    pub seven_day_pct: Option<f64>,
    pub agent: Option<String>,
    pub updated_at_ms: i64,
}

impl StatusRecord {
    pub fn new(p: &Payload, branch: Option<&str>, now_ms: i64) -> StatusRecord {
        StatusRecord {
            model: p.model.clone(),
            cwd: p.dir.clone(),
            branch: branch.and_then(label),
            context_pct: p.context_pct,
            five_hour_pct: p.five_hour_pct,
            five_hour_resets_at: p.five_hour_resets_at,
            seven_day_pct: p.seven_day_pct,
            agent: p.agent.clone(),
            updated_at_ms: now_ms,
        }
    }

    /// `pane_id` reaches this process through `ANVIL_PANE_ID`, which ANVIL sets
    /// to a `u64` but any program that spawns the statusLine command controls.
    /// Only the digits ANVIL itself emits are accepted: a separator or `..`
    /// would otherwise make `atomic_write` (which creates the parent) place the
    /// record outside `status_dir` and overwrite a file of the caller's
    /// choosing. Rejecting is silent — a pane without a badge is harmless, an
    /// arbitrary write is not.
    fn clean_pane_id(pane_id: &str) -> Option<&str> {
        (!pane_id.is_empty() && pane_id.len() <= 20 && pane_id.bytes().all(|byte| byte.is_ascii_digit()))
            .then_some(pane_id)
    }

    pub fn file_path(status_dir: &Path, pane_id: &str) -> std::path::PathBuf {
        status_dir.join(format!("{}.json", Self::clean_pane_id(pane_id).unwrap_or("invalid")))
    }

    pub fn write(&self, status_dir: &Path, pane_id: &str) -> std::io::Result<()> {
        let Some(pane_id) = Self::clean_pane_id(pane_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "pane id is not numeric"));
        };
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        atomic_write(&Self::file_path(status_dir, pane_id), text.as_bytes())
    }

    pub fn read(path: &Path) -> Option<StatusRecord> {
        // Bounded: this file is re-read whenever its mtime moves, so a
        // substituted or enormous one must cost a rejected read.
        let bytes = crate::fsutil::read_limited(path, MAX_STATUS_BYTES).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// No status line is anywhere near this; the ceiling only stops a substituted
/// file from being read into memory whole.
const MAX_STATUS_BYTES: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    fn line(json: &str) -> String {
        format_line(&Payload::parse(json).unwrap(), None, 0)
    }

    /// The quota line accepts `resets_at` in seconds or milliseconds, so the
    /// status line has to agree: the same value must produce the same hint.
    #[test]
    fn resets_hint_accepts_seconds_and_milliseconds_alike() {
        let now_ms = 1_755_000_000_000;
        assert_eq!(resets_in(1_755_000_000.0, now_ms).as_deref(), Some("0м"));
        assert_eq!(resets_in(1_755_000_000_000.0, now_ms).as_deref(), Some("0м"));
        assert_eq!(resets_in(1_755_000_720.0, now_ms).as_deref(), Some("12м"));
        assert_eq!(resets_in(1_755_000_720_000.0, now_ms).as_deref(), Some("12м"));
        assert_eq!(resets_in(1.0, now_ms), None, "not a plausible unix time either way");
        assert_eq!(resets_in(f64::NAN, now_ms), None);
    }

    // Expected strings were produced by running the owner's statusline.mjs
    // under Node on the same payloads (directories do not exist, so no branch).
    #[test]
    fn matches_statusline_mjs_readme_payload() {
        let json = r#"{"model":{"display_name":"Opus 5"},"workspace":{"current_dir":"C:/anvil-nonexistent/seller"},
            "context_window":{"used_percentage":37.4},
            "rate_limits":{"five_hour":{"used_percentage":17.2,"resets_at":1755500000},"seven_day":{"used_percentage":63.8}}}"#;
        assert_eq!(
            line(json),
            "\x1b[36m[Opus 5]\x1b[0m\x1b[2m | \x1b[0mseller\x1b[2m | \x1b[0m\x1b[32m▓▓▓▓░░░░░░\x1b[0m \x1b[32m37%\x1b[0m\x1b[2m | \x1b[0m5h: \x1b[32m17%\x1b[0m\x1b[2m | \x1b[0m7d: \x1b[33m64%\x1b[0m"
        );
    }

    #[test]
    fn matches_statusline_mjs_agent_without_limits() {
        let json = r#"{"model":{"display_name":"Fable 5"},"cwd":"C:\\anvil-nonexistent\\proj\\",
            "context_window":{"used_percentage":85},"agent":{"name":"reviewer"}}"#;
        assert_eq!(
            line(json),
            "\x1b[36m[Fable 5]\x1b[0m\x1b[2m | \x1b[0mproj\x1b[2m | \x1b[0m\x1b[31m▓▓▓▓▓▓▓▓▓░\x1b[0m \x1b[31m85%\x1b[0m\x1b[2m | \x1b[0m\x1b[35mreviewer\x1b[0m"
        );
    }

    #[test]
    fn matches_statusline_mjs_rounding_edges() {
        let json = r#"{"model":{"display_name":"Sonnet 5"},"context_window":{"used_percentage":84.9},
            "rate_limits":{"five_hour":{"used_percentage":59.5},"seven_day":{"used_percentage":5}}}"#;
        assert_eq!(
            line(json),
            "\x1b[36m[Sonnet 5]\x1b[0m\x1b[2m | \x1b[0m\x1b[33m▓▓▓▓▓▓▓▓░░\x1b[0m \x1b[33m85%\x1b[0m\x1b[2m | \x1b[0m5h: \x1b[32m60%\x1b[0m\x1b[2m | \x1b[0m7d: \x1b[32m5%\x1b[0m"
        );
        let json = r#"{"context_window":{"used_percentage":0.4},"rate_limits":{"five_hour":{"used_percentage":60}}}"#;
        assert_eq!(line(json), "\x1b[32m░░░░░░░░░░\x1b[0m \x1b[32m0%\x1b[0m\x1b[2m | \x1b[0m5h: \x1b[33m60%\x1b[0m");
    }

    /// The branch is read from this path, so a long one must reach it whole.
    #[test]
    fn a_working_directory_is_kept_whole_or_dropped_never_cut() {
        let long = format!("C:/{}", "deep/".repeat(80));
        assert!(long.chars().count() > MAX_TEXT_CHARS);
        let parse = |dir: &str| {
            Payload::parse(&serde_json::json!({"workspace": {"current_dir": dir}}).to_string()).unwrap().dir
        };
        assert_eq!(parse(&long), Some(long.clone()));
        assert_eq!(parse("C:/pro\u{1b}j"), None, "a lookup target must not become a different path");
        assert_eq!(parse(&"d".repeat(MAX_PATH_CHARS + 1)), None);
    }

    /// A model or agent name can come from a repository's settings or agent
    /// definitions. It is printed into the terminal and written to the status
    /// file, which the reader refuses above 64 KiB, so it is a bounded plain label.
    #[test]
    fn payload_text_is_a_printable_bounded_label() {
        let hostile = format!("Opus\u{1b}]52;c;AAAA\u{7}\u{9b}2J{}", "x".repeat(200_000));
        let json = serde_json::json!({
            "model": { "display_name": hostile },
            "agent": { "name": "\u{1b}[2J\n" },
            "cwd": "C:/x/proj\r\n"
        });
        let payload = Payload::parse(&json.to_string()).unwrap();
        let model = payload.model.as_deref().unwrap();
        assert!(model.starts_with("Opus]52;c;AAAA2Jxxx"), "{model}");
        assert_eq!(model.chars().count(), 256);
        assert_eq!(payload.agent.as_deref(), Some("[2J"));
        assert_eq!(payload.dir, None, "a path containing controls is rejected whole");
        // Only ANVIL's own colour sequences remain in the printed line.
        let printed = format_line(&payload, None, 0);
        assert_eq!(printed.matches('\u{1b}').count(), printed.matches("\u{1b}[").count(), "{printed:?}");
        assert!(printed.chars().all(|ch| !ch.is_control() || ch == '\u{1b}'), "{printed:?}");

        let dir = tempfile::tempdir().unwrap();
        let record = StatusRecord::new(&payload, None, 1);
        record.write(dir.path(), "3").unwrap();
        assert_eq!(StatusRecord::read(&StatusRecord::file_path(dir.path(), "3")), Some(record));
    }

    #[test]
    fn status_labels_cannot_override_text_direction() {
        let dir = "C:/project\u{202e}gpj.exe";
        let payload = Payload::parse(
            &serde_json::json!({
                "model": {"display_name": "Opus\u{202e}spoof"},
                "agent": {"name": "agent\u{2066}spoof"},
                "cwd": dir,
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(payload.dir.as_deref(), Some(dir), "the actual lookup path stays exact");
        let printed = format_line(&payload, Some("branch\u{202e}spoof"), 0);
        assert!(!printed.contains('\u{202e}') && !printed.contains('\u{2066}'), "{printed:?}");
        assert!(printed.contains("projectgpj.exe") && printed.contains("branchspoof"), "{printed:?}");
        let record = StatusRecord::new(&payload, Some("branch\u{202e}spoof"), 0);
        assert_eq!(record.branch.as_deref(), Some("branchspoof"));
    }

    #[test]
    fn empty_and_invalid_input_print_nothing() {
        assert_eq!(line("{}"), "");
        assert!(Payload::parse("не json").is_none());
    }

    /// `ANVIL_PANE_ID` is set by ANVIL, but anything that spawns the statusLine
    /// command controls it. `atomic_write` creates the parent directory, so an
    /// unchecked id would place the record wherever the string pointed.
    #[test]
    fn a_pane_id_cannot_choose_the_path() {
        let base = tempfile::tempdir().unwrap();
        let status_dir = base.path().join("run");
        let record = StatusRecord::new(&Payload { model: Some("Opus".into()), ..Payload::default() }, None, 1);
        for hostile in ["..\\..\\escaped", "../../escaped", "..", "1/../../x", "a\\b", "with space", "", "-1", "1.json"]
        {
            assert!(record.write(&status_dir, hostile).is_err(), "{hostile:?} must be refused");
        }
        assert!(record.write(&status_dir, "7").is_ok());
        assert!(status_dir.join("7.json").is_file());
        assert!(!base.path().join("escaped.json").exists(), "nothing was written outside the status directory");
        // A hostile id never reaches the reader either: it has no file to read.
        assert!(StatusRecord::file_path(&status_dir, "..\\..\\escaped").starts_with(&status_dir));
    }

    #[test]
    fn reset_time_from_sixty_percent() {
        let now_ms: i64 = 1_759_500_000_000;
        let resets = (now_ms / 1000 + 2 * 3600 + 5 * 60) as f64;
        let p = Payload { five_hour_pct: Some(75.0), five_hour_resets_at: Some(resets), ..Payload::default() };
        assert_eq!(format_line(&p, None, now_ms), "5h: \x1b[33m75%\x1b[0m \x1b[2m↺ 2ч5м\x1b[0m");
        let p = Payload { five_hour_resets_at: Some((now_ms / 1000 + 45 * 60) as f64), ..p };
        assert_eq!(format_line(&p, None, now_ms), "5h: \x1b[33m75%\x1b[0m \x1b[2m↺ 45м\x1b[0m");
        let p = Payload { five_hour_pct: Some(59.0), ..p };
        assert_eq!(format_line(&p, None, now_ms), "5h: \x1b[32m59%\x1b[0m", "no reset below 60%");
    }

    #[test]
    fn branch_colours() {
        let p = Payload { dir: Some("C:/x/proj".into()), ..Payload::default() };
        assert_eq!(format_line(&p, Some("main"), 0), "proj\x1b[2m | \x1b[0m\x1b[31mmain\x1b[0m");
        assert_eq!(format_line(&p, Some("dev"), 0), "proj\x1b[2m | \x1b[0m\x1b[32mdev\x1b[0m");
    }

    #[test]
    fn fields_switch_pieces_off_and_keep_the_separators() {
        use crate::config::ClaudeLineFields;
        let json = r#"{"model":{"display_name":"Opus 5"},"workspace":{"current_dir":"C:/anvil-nonexistent/seller"},
            "context_window":{"used_percentage":37.4},
            "rate_limits":{"five_hour":{"used_percentage":17.2,"resets_at":1755500000},"seven_day":{"used_percentage":63.8}}}"#;
        let p = Payload::parse(json).unwrap();
        assert_eq!(format_line_with(&p, None, 0, &ClaudeLineFields::default()), format_line(&p, None, 0));
        let fields = ClaudeLineFields { dir: false, five_hour: false, ..ClaudeLineFields::default() };
        assert_eq!(
            format_line_with(&p, None, 0, &fields),
            "\x1b[36m[Opus 5]\x1b[0m\x1b[2m | \x1b[0m\x1b[32m▓▓▓▓░░░░░░\x1b[0m \x1b[32m37%\x1b[0m\x1b[2m | \x1b[0m7d: \x1b[33m64%\x1b[0m"
        );
        let none = ClaudeLineFields {
            model: false,
            dir: false,
            branch: false,
            context: false,
            five_hour: false,
            seven_day: false,
            agent: false,
        };
        assert_eq!(format_line_with(&p, Some("main"), 0, &none), "");
    }

    #[test]
    fn branch_field_hides_the_branch() {
        use crate::config::ClaudeLineFields;
        let p = Payload { dir: Some("C:/x/proj".into()), ..Payload::default() };
        let fields = ClaudeLineFields { branch: false, ..ClaudeLineFields::default() };
        assert_eq!(format_line_with(&p, Some("main"), 0, &fields), "proj");
    }

    #[test]
    fn js_round_matches_math_round() {
        assert_eq!(js_round(59.5), 60);
        assert_eq!(js_round(84.9), 85);
        assert_eq!(js_round(8.5), 9);
        assert_eq!(js_round(0.4), 0);
    }

    #[test]
    fn status_record_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = Payload { model: Some("Opus 5".into()), context_pct: Some(37.4), ..Payload::default() };
        let rec = StatusRecord::new(&p, Some("dev"), 42);
        rec.write(dir.path(), "7").unwrap();
        assert_eq!(StatusRecord::read(&StatusRecord::file_path(dir.path(), "7")), Some(rec));
    }
}
