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

impl Payload {
    /// None when the input is not JSON (print nothing, like the script).
    pub fn parse(text: &str) -> Option<Payload> {
        let v: Value = serde_json::from_str(text).ok()?;
        let s = |p: &str| v.pointer(p).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
        let n = |p: &str| v.pointer(p).and_then(Value::as_f64);
        Some(Payload {
            model: s("/model/display_name"),
            dir: s("/workspace/current_dir").or_else(|| s("/cwd")),
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
fn resets_in(resets_at_secs: f64, now_ms: i64) -> Option<String> {
    if !(1_000_000_000.0..=4_000_000_000.0).contains(&resets_at_secs) {
        return None;
    }
    let left = ((resets_at_secs * 1000.0) as i64 - now_ms).clamp(0, 7 * 86_400_000);
    let h = left / 3_600_000;
    let m = (left % 3_600_000) / 60_000;
    Some(if h > 0 { format!("{h}ч{m}м") } else { format!("{m}м") })
}

/// Percentages outside 0-100 (a broken or hostile payload) are clamped.
fn clamp_pct(pct: f64) -> f64 {
    pct.clamp(0.0, 100.0)
}

/// Last path component, accepting `/` and `\` and ignoring trailing separators
/// (Node's `path.basename` on Windows).
pub fn basename(dir: &str) -> &str {
    let trimmed = dir.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

/// The statusLine text. `branch` comes from git (see `git_branch`).
pub fn format_line(p: &Payload, branch: Option<&str>, now_ms: i64) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(model) = &p.model {
        parts.push(format!("{CYAN}[{model}]{RESET}"));
    }
    if let Some(dir) = &p.dir {
        parts.push(basename(dir).to_owned());
    }
    if let Some(branch) = branch.filter(|b| !b.is_empty()) {
        let color = if branch == "main" || branch == "master" { RED } else { GREEN };
        parts.push(format!("{color}{branch}{RESET}"));
    }
    if let Some(ctx) = p.context_pct.map(clamp_pct) {
        let c = heat(ctx);
        parts.push(format!("{c}{}{RESET} {c}{}%{RESET}", bar(ctx), js_round(ctx)));
    }
    if let Some(five) = p.five_hour_pct.map(clamp_pct) {
        let c = heat(five);
        let mut text = format!("5h: {c}{}%{RESET}", js_round(five));
        if five >= 60.0 {
            if let Some(reset) = p.five_hour_resets_at.and_then(|resets| resets_in(resets, now_ms)) {
                text.push_str(&format!(" {DIM}↺ {reset}{RESET}"));
            }
        }
        parts.push(text);
    }
    if let Some(week) = p.seven_day_pct.map(clamp_pct) {
        parts.push(format!("7d: {}{}%{RESET}", heat(week), js_round(week)));
    }
    if let Some(agent) = &p.agent {
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
            branch: branch.map(str::to_owned),
            context_pct: p.context_pct,
            five_hour_pct: p.five_hour_pct,
            five_hour_resets_at: p.five_hour_resets_at,
            seven_day_pct: p.seven_day_pct,
            agent: p.agent.clone(),
            updated_at_ms: now_ms,
        }
    }

    pub fn file_path(status_dir: &Path, pane_id: &str) -> std::path::PathBuf {
        status_dir.join(format!("{pane_id}.json"))
    }

    pub fn write(&self, status_dir: &Path, pane_id: &str) -> std::io::Result<()> {
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        atomic_write(&Self::file_path(status_dir, pane_id), text.as_bytes())
    }

    pub fn read(path: &Path) -> Option<StatusRecord> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(json: &str) -> String {
        format_line(&Payload::parse(json).unwrap(), None, 0)
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
        assert_eq!(
            line(json),
            "\x1b[32m░░░░░░░░░░\x1b[0m \x1b[32m0%\x1b[0m\x1b[2m | \x1b[0m5h: \x1b[33m60%\x1b[0m"
        );
    }

    #[test]
    fn empty_and_invalid_input_print_nothing() {
        assert_eq!(line("{}"), "");
        assert!(Payload::parse("не json").is_none());
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
