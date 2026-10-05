//! The payload of a JWT, decoded without verification: ANVIL only reads a
//! token's expiry and account claims to decide whether to use it, never to
//! trust anything it says.

pub fn payload(jwt: &str) -> Option<serde_json::Value> {
    let part = jwt.split('.').nth(1)?;
    let mut bytes = Vec::with_capacity(part.len() * 3 / 4);
    let mut value = 0u32;
    let mut bits = 0u32;
    for byte in part.bytes() {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            // Padding ends the segment; anything else is not base64url and
            // must not be skipped, or junk would decode into a payload the
            // token never carried.
            b'=' => break,
            _ => return None,
        };
        value = value << 6 | u32::from(digit);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((value >> bits) as u8);
        }
    }
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
pub(crate) fn encode_for_test(claims: &serde_json::Value) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = serde_json::to_vec(claims).unwrap();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | u32::from(*b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    format!("eyJhbGciOiJub25lIn0.{out}.sig")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_payload_segment() {
        let claims =
            serde_json::json!({"exp": 1_791_210_000, "https://api.openai.com/auth": {"chatgpt_plan_type": "pro"}});
        assert_eq!(payload(&encode_for_test(&claims)), Some(claims));
        assert_eq!(payload("not-a-jwt"), None);
        assert_eq!(payload("a.%%%.c"), None);
    }
}
