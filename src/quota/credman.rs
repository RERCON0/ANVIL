//! Keys the user types into ANVIL live in the Windows Credential Manager as
//! generic credentials `anvil/quota/<provider>`, never in config.json.

use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

use super::model::ProviderId;

/// Longest key accepted from the settings page.
pub const MAX_KEY_LEN: usize = 512;

/// `anvil/quota/zai` and so on.
pub fn target(id: ProviderId) -> String {
    format!("anvil/quota/{}", id.key())
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The stored secret, or `None` when there is none (or it cannot be read).
pub fn read(target: &str) -> Option<String> {
    let name = wide(target);
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: NUL-terminated target; on success the system allocates the
    // struct, which is released with CredFree below.
    if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
        return None;
    }
    // SAFETY: `credential` points to a valid CREDENTIALW until CredFree; the
    // blob pointer and size describe its secret bytes.
    let secret = unsafe {
        let c = &*credential;
        let bytes = if c.CredentialBlob.is_null() {
            &[][..]
        } else {
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
        };
        let text = String::from_utf8(bytes.to_vec()).ok();
        CredFree(credential.cast());
        text
    };
    secret.filter(|s| !s.trim().is_empty())
}

pub fn write(target: &str, secret: &str) -> Result<(), u32> {
    let mut name = wide(target);
    let mut user = wide("anvil");
    let mut blob = secret.as_bytes().to_vec();
    // SAFETY: CREDENTIALW is plain data; zero is a valid "unset" for every field.
    let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };
    credential.Type = CRED_TYPE_GENERIC;
    credential.TargetName = name.as_mut_ptr();
    credential.UserName = user.as_mut_ptr();
    credential.CredentialBlobSize = blob.len() as u32;
    credential.CredentialBlob = blob.as_mut_ptr();
    // Scope the record to this machine, not the domain profile: it must survive
    // the next logon of this same user, but must never roam to another machine
    // where it would be decrypted under a different user context. The blob
    // itself is encrypted by the credential manager with this user's DPAPI
    // master key regardless of this flag — `Persist` chooses where the record
    // is kept, not the encryption scope.
    credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
    // SAFETY: every pointer in `credential` refers to a buffer alive for the call.
    if unsafe { CredWriteW(&credential, 0) } == 0 {
        // SAFETY: reads the calling thread's last-error value.
        return Err(unsafe { GetLastError() });
    }
    Ok(())
}

/// Removing a key that is not there counts as success.
pub fn delete(target: &str) -> Result<(), u32> {
    let name = wide(target);
    // SAFETY: NUL-terminated target name.
    if unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
        // SAFETY: reads the calling thread's last-error value.
        let code = unsafe { GetLastError() };
        return if code == ERROR_NOT_FOUND { Ok(()) } else { Err(code) };
    }
    Ok(())
}

/// What the settings page accepts as a key: trimmed, non-empty, bounded, no
/// control characters (a pasted line break would corrupt the header).
pub fn clean_key(input: &str) -> Option<String> {
    let key = input.trim();
    (!key.is_empty() && key.len() <= MAX_KEY_LEN && !key.chars().any(char::is_control)).then(|| key.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_delete_round_trip() {
        let target = format!("anvil/quota-test/{}-{}", std::process::id(), crate::quota::time::now_unix());
        assert_eq!(read(&target), None);
        write(&target, "sk-test-ключ").unwrap();
        assert_eq!(read(&target).as_deref(), Some("sk-test-ключ"));
        write(&target, "sk-replaced").unwrap();
        assert_eq!(read(&target).as_deref(), Some("sk-replaced"));
        delete(&target).unwrap();
        assert_eq!(read(&target), None);
        delete(&target).unwrap();
    }

    #[test]
    fn keys_are_cleaned() {
        assert_eq!(clean_key("  abc  ").as_deref(), Some("abc"));
        assert_eq!(clean_key("   "), None);
        assert_eq!(clean_key("ab\ncd"), None);
        assert_eq!(clean_key(&"k".repeat(MAX_KEY_LEN + 1)), None);
    }
}
