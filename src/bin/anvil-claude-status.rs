//! Claude Code statusLine command: prints the same line as the owner's
//! statusline.mjs and, inside an ANVIL pane, leaves a status file for the tab.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anvil::claude_status::{format_line_with, git_branch, Payload, StatusRecord, MAX_PAYLOAD_BYTES};
use anvil::config::Config;

fn main() {
    if let Err(error) = anvil::secure_dll_search() {
        eprintln!("ANVIL status helper: cannot secure DLL search: {error}");
        std::process::exit(1);
    }
    let Some(payload) = read_payload(std::io::stdin()) else { return };
    // The fields chosen in ANVIL's settings; an absent or unreadable config.json
    // (never quarantined from here) means every field, as before.
    let config = Config::load_for_reload(&Config::path()).unwrap_or_default();
    anvil::strings::set_language(config.language);
    let fields = config.claude_status.line_fields;
    let branch = if fields.branch {
        payload.dir.as_deref().and_then(|d| git_branch(std::path::Path::new(d), Duration::from_millis(800)))
    } else {
        None
    };
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);

    let line = format_line_with(&payload, branch.as_deref(), now_ms, &fields);
    let mut out = std::io::stdout();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();

    if let (Some(dir), Some(pane)) = (std::env::var_os("ANVIL_STATUS_DIR"), std::env::var("ANVIL_PANE_ID").ok()) {
        let record = StatusRecord::new(&payload, branch.as_deref(), now_ms);
        let _ = record.write(&PathBuf::from(dir), &pane);
    }
}

/// Read one complete, bounded JSON payload, never a valid-looking prefix of
/// an oversized or malformed input.
fn read_payload(input: impl Read) -> Option<Payload> {
    let mut text = String::new();
    input.take(MAX_PAYLOAD_BYTES + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= MAX_PAYLOAD_BYTES).then(|| Payload::parse(&text)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_input_cannot_publish_a_valid_prefix() {
        let mut input = br#"{"model":{"display_name":"Opus"}}"#.to_vec();
        input.resize(MAX_PAYLOAD_BYTES as usize, b' ');
        assert_eq!(read_payload(input.as_slice()).unwrap().model.as_deref(), Some("Opus"));
        input.push(b'x');
        assert!(read_payload(input.as_slice()).is_none());
        input.pop();
        input.push(b' ');
        assert!(read_payload(input.as_slice()).is_none());
        assert!(read_payload(b"{".as_slice()).is_none());
    }
}
