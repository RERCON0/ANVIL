//! Claude Code statusLine command: prints the same line as the owner's
//! statusline.mjs and, inside an ANVIL pane, leaves a status file for the tab.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anvil::claude_status::{format_line, git_branch, Payload, StatusRecord};

fn main() {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }
    let Some(payload) = Payload::parse(&input) else { return };
    let branch = payload
        .dir
        .as_deref()
        .and_then(|d| git_branch(std::path::Path::new(d), Duration::from_millis(800)));
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);

    let line = format_line(&payload, branch.as_deref(), now_ms);
    let mut out = std::io::stdout();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();

    if let (Some(dir), Some(pane)) = (std::env::var_os("ANVIL_STATUS_DIR"), std::env::var("ANVIL_PANE_ID").ok()) {
        let record = StatusRecord::new(&payload, branch.as_deref(), now_ms);
        let _ = record.write(&PathBuf::from(dir), &pane);
    }
}
