//! Resolve filesystem transports before Git can start a local receive-pack.
use super::{bytes_path, config_path, config_refusal, network_refusal};
use std::path::{Path, PathBuf};

pub(super) fn local_destination(value: &str, root: &Path) -> Result<Option<PathBuf>, String> {
    let value = value.trim();
    if let Some(url) = value.strip_prefix("file://") {
        let (host, path) = url.split_once('/').ok_or_else(config_refusal)?;
        // `file:///C:/…` and `file://localhost/C:/…` are the canonical forms;
        // Git for Windows also accepts `file://C:/…`, where the "host" is a drive.
        let drive = host.len() == 2 && host.ends_with(':') && host.as_bytes()[0].is_ascii_alphabetic();
        if drive {
            let text = format!("{host}/{path}");
            return config_path(&text, root, root).map(Some);
        }
        if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
            return Err(network_refusal());
        }
        let mut decoded = Vec::with_capacity(path.len() + 1);
        decoded.push(b'/');
        let mut bytes = path.bytes();
        while let Some(byte) = bytes.next() {
            if byte == b'%' {
                let high = bytes.next().and_then(|c| (c as char).to_digit(16)).ok_or_else(config_refusal)?;
                let low = bytes.next().and_then(|c| (c as char).to_digit(16)).ok_or_else(config_refusal)?;
                decoded.push((high * 16 + low) as u8);
            } else {
                decoded.push(byte);
            }
        }
        #[cfg(windows)]
        if decoded.get(1).is_some_and(u8::is_ascii_alphabetic) && decoded.get(2) == Some(&b':') {
            decoded.remove(0);
        }
        let path = bytes_path(decoded)?;
        let text = path.to_str().ok_or_else(config_refusal)?;
        return config_path(text, root, root).map(Some);
    }
    // Git for Windows distinguishes drive paths from scp-style host:path URLs.
    // Other colon-bearing values are network/helper transports, not disk paths.
    #[cfg(windows)]
    let drive = value.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) && value.as_bytes().get(1) == Some(&b':');
    #[cfg(not(windows))]
    let drive = false;
    if value.is_empty() || (value.contains(':') && !drive) {
        return Ok(None);
    }
    config_path(value, root, root).map(Some)
}
