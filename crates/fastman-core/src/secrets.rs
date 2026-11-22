//! Secret variable references, in-memory testing store, and OS credential store backends.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex},
};
use thiserror::Error;
use uuid::Uuid;

pub const MAX_SECRET_VALUE_BYTES: usize = 2048;
const MAX_SECRET_KEY_BYTES: usize = 256;
const MAX_ENV_VAR_NAME_BYTES: usize = 128;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SecretStoreError {
    #[error("OS credential store unavailable: {0}")]
    Unavailable(String),
    #[error("Secret value exceeds maximum allowed size ({0} bytes)")]
    TooLarge(usize),
    #[error("Invalid secret reference key: {0}")]
    InvalidKey(String),
    #[error("Credential store operation failed: {0}")]
    Backend(String),
}

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, SecretStoreError>;
    fn set(&self, key: &str, value: &str) -> Result<(), SecretStoreError>;
    fn delete(&self, key: &str) -> Result<(), SecretStoreError>;
}

/// Validates a keyring/credential-store key identifier.
pub fn validate_secret_key(key: &str) -> Result<(), SecretStoreError> {
    if key.is_empty() || key.len() > MAX_SECRET_KEY_BYTES || key.contains("..") {
        return Err(SecretStoreError::InvalidKey(
            "key must be 1..=256 characters and cannot contain '..'".into(),
        ));
    }
    if !key
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'))
    {
        return Err(SecretStoreError::InvalidKey(
            "key must use ASCII letters, digits, '.', '_', ':', '/', or '-'".into(),
        ));
    }
    Ok(())
}

/// Validates a process environment variable name referenced by `SecretRef::Env`.
pub fn validate_env_var_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_ENV_VAR_NAME_BYTES {
        return false;
    }
    let first_ok = bytes[0].is_ascii_alphabetic() || bytes[0] == b'_';
    first_ok
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// Generates a unique, opaque key for storing a secret variable in the OS credential store.
pub fn make_keychain_key(project_id: &str, scope: &str) -> String {
    let clean_proj: String = project_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(64)
        .collect();
    let clean_scope: String = scope
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(64)
        .collect();
    let proj_part = if clean_proj.is_empty() {
        "project"
    } else {
        clean_proj.as_str()
    };
    let scope_part = if clean_scope.is_empty() {
        "base"
    } else {
        clean_scope.as_str()
    };
    format!("fastman:{proj_part}:{scope_part}:{}", Uuid::new_v4())
}

/// Effective plain and secret variable values resolved for an active project/environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedVariables {
    pub values: BTreeMap<String, String>,
    pub secret_names: HashSet<String>,
    pub unresolved_secrets: BTreeMap<String, String>,
}

impl ResolvedVariables {
    pub fn from_plain(values: BTreeMap<String, String>) -> Self {
        Self {
            values,
            secret_names: HashSet::new(),
            unresolved_secrets: BTreeMap::new(),
        }
    }

    pub fn secret_values(&self) -> Vec<&str> {
        self.secret_names
            .iter()
            .filter_map(|name| self.values.get(name))
            .map(String::as_str)
            .filter(|val| !val.is_empty())
            .collect()
    }
}

/// Ordered credential store updates paired with saving `project.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretChangePlan {
    pub writes: Vec<(String, String)>,
    pub deletes_after_commit: Vec<String>,
}

impl SecretChangePlan {
    pub fn is_empty(&self) -> bool {
        self.writes.is_empty() && self.deletes_after_commit.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemorySecretStore {
    entries: Arc<Mutex<HashMap<String, String>>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretStoreError> {
        validate_secret_key(key)?;
        let guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        Ok(guard.get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretStoreError> {
        validate_secret_key(key)?;
        if value.len() > MAX_SECRET_VALUE_BYTES {
            return Err(SecretStoreError::TooLarge(MAX_SECRET_VALUE_BYTES));
        }
        let mut guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        validate_secret_key(key)?;
        let mut guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(key);
        Ok(())
    }
}

/// Cross-platform OS credential store backed by Win32 Credential Manager,
/// macOS Keychain (`/usr/bin/security`), or Linux Secret Service (`secret-tool`).
#[derive(Debug, Clone)]
pub struct OsSecretStore {
    service: String,
}

impl Default for OsSecretStore {
    fn default() -> Self {
        Self {
            service: "fastman".to_owned(),
        }
    }
}

impl OsSecretStore {
    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn validate_inputs(key: &str, service: &str) -> Result<(), SecretStoreError> {
        validate_secret_key(key)?;
        validate_secret_key(service)?;
        Ok(())
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn base64_url_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = if i + 1 < bytes.len() {
            bytes[i + 1] as u32
        } else {
            0
        };
        let b2 = if i + 2 < bytes.len() {
            bytes[i + 2] as u32
        } else {
            0
        };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        }
        if i + 2 < bytes.len() {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        }
        i += 3;
    }
    out
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a' + 26) as u32),
            b'0'..=b'9' => Some((b - b'0' + 52) as u32),
            b'-' | b'+' => Some(62),
            b'_' | b'/' => Some(63),
            _ => None,
        }
    }
    let clean = input.trim().trim_end_matches('=').as_bytes();
    if clean.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity((clean.len() * 3) / 4);
    for chunk in clean.chunks(4) {
        let c0 = val(chunk[0])?;
        let c1 = val(chunk[1])?;
        match chunk.len() {
            2 => {
                let n = (c0 << 18) | (c1 << 12);
                out.push((n >> 16) as u8);
            }
            3 => {
                let c2 = val(chunk[2])?;
                let n = (c0 << 18) | (c1 << 12) | (c2 << 6);
                out.push((n >> 16) as u8);
                out.push((n >> 8) as u8);
            }
            4 => {
                let c2 = val(chunk[2])?;
                let c3 = val(chunk[3])?;
                let n = (c0 << 18) | (c1 << 12) | (c2 << 6) | c3;
                out.push((n >> 16) as u8);
                out.push((n >> 8) as u8);
                out.push(n as u8);
            }
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(windows)]
mod platform {
    use super::{MAX_SECRET_VALUE_BYTES, SecretStoreError};
    use std::{ffi::c_void, io, ptr};

    const CRED_TYPE_GENERIC: u32 = 1;
    const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;
    const ERROR_NOT_FOUND: i32 = 1168;

    #[repr(C)]
    struct CredentialW {
        flags: u32,
        cred_type: u32,
        target_name: *mut u16,
        comment: *mut u16,
        last_written: [u32; 2],
        credential_blob_size: u32,
        credential_blob: *mut u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut c_void,
        target_alias: *mut u16,
        user_name: *mut u16,
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
        fn CredReadW(
            target_name: *const u16,
            cred_type: u32,
            flags: u32,
            credential: *mut *mut CredentialW,
        ) -> i32;
        fn CredDeleteW(target_name: *const u16, cred_type: u32, flags: u32) -> i32;
        fn CredFree(buffer: *mut c_void);
    }

    struct CredGuard(*mut CredentialW);

    impl Drop for CredGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CredFree(self.0.cast()) };
            }
        }
    }

    fn encode_target(service: &str, key: &str) -> Vec<u16> {
        format!("{service}/{key}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    pub(super) fn get(service: &str, key: &str) -> Result<Option<String>, SecretStoreError> {
        let target = encode_target(service, key);
        let mut raw_ptr: *mut CredentialW = ptr::null_mut();
        let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut raw_ptr) };
        let _guard = CredGuard(raw_ptr);
        if ok == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NOT_FOUND) {
                return Ok(None);
            }
            return Err(SecretStoreError::Backend(format!(
                "Win32 CredReadW failed ({err})"
            )));
        }
        if raw_ptr.is_null() {
            return Ok(None);
        }
        let cred = unsafe { &*raw_ptr };
        if cred.credential_blob.is_null() || cred.credential_blob_size == 0 {
            return Ok(Some(String::new()));
        }
        let len = cred.credential_blob_size as usize;
        if len > MAX_SECRET_VALUE_BYTES {
            return Err(SecretStoreError::TooLarge(MAX_SECRET_VALUE_BYTES));
        }
        let bytes = unsafe { std::slice::from_raw_parts(cred.credential_blob, len) }.to_vec();
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| SecretStoreError::Backend("Stored credential is not valid UTF-8".into()))
    }

    pub(super) fn set(service: &str, key: &str, value: &str) -> Result<(), SecretStoreError> {
        if value.len() > MAX_SECRET_VALUE_BYTES {
            return Err(SecretStoreError::TooLarge(MAX_SECRET_VALUE_BYTES));
        }
        let mut target = encode_target(service, key);
        let mut blob = value.as_bytes().to_vec();
        let blob_size = u32::try_from(blob.len())
            .map_err(|_| SecretStoreError::TooLarge(MAX_SECRET_VALUE_BYTES))?;
        let cred = CredentialW {
            flags: 0,
            cred_type: CRED_TYPE_GENERIC,
            target_name: target.as_mut_ptr(),
            comment: ptr::null_mut(),
            last_written: [0, 0],
            credential_blob_size: blob_size,
            credential_blob: if blob.is_empty() {
                ptr::null_mut()
            } else {
                blob.as_mut_ptr()
            },
            persist: CRED_PERSIST_LOCAL_MACHINE,
            attribute_count: 0,
            attributes: ptr::null_mut(),
            target_alias: ptr::null_mut(),
            user_name: ptr::null_mut(),
        };
        let ok = unsafe { CredWriteW(&cred, 0) };
        if ok == 0 {
            let err = io::Error::last_os_error();
            return Err(SecretStoreError::Backend(format!(
                "Win32 CredWriteW failed ({err})"
            )));
        }
        Ok(())
    }

    pub(super) fn delete(service: &str, key: &str) -> Result<(), SecretStoreError> {
        let target = encode_target(service, key);
        let ok = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NOT_FOUND) {
                return Ok(());
            }
            return Err(SecretStoreError::Backend(format!(
                "Win32 CredDeleteW failed ({err})"
            )));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{MAX_SECRET_VALUE_BYTES, SecretStoreError, base64_url_decode, base64_url_encode};
    use std::{
        io::{ErrorKind, Write},
        process::{Command, Stdio},
    };

    const ITEM_NOT_FOUND_EXIT: i32 = 44;

    pub(super) fn get(service: &str, key: &str) -> Result<Option<String>, SecretStoreError> {
        let out = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", service, "-a", key, "-w"])
            .output()
            .map_err(|err| {
                if err.kind() == ErrorKind::NotFound {
                    SecretStoreError::Unavailable("/usr/bin/security not found".into())
                } else {
                    SecretStoreError::Backend(format!("Run /usr/bin/security: {err}"))
                }
            })?;
        if !out.status.success() {
            if out.status.code() == Some(ITEM_NOT_FOUND_EXIT) {
                return Ok(None);
            }
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(SecretStoreError::Backend(format!(
                "Keychain lookup failed: {}",
                stderr.trim()
            )));
        }
        let raw = String::from_utf8_lossy(&out.stdout);
        let trimmed = raw.trim_end_matches(['\r', '\n']);
        if let Some(encoded) = trimmed.strip_prefix("fm1:") {
            let decoded = base64_url_decode(encoded).ok_or_else(|| {
                SecretStoreError::Backend("Invalid base64 payload in keychain item".into())
            })?;
            return String::from_utf8(decoded)
                .map(Some)
                .map_err(|_| SecretStoreError::Backend("Keychain item is not valid UTF-8".into()));
        }
        Ok(Some(trimmed.to_owned()))
    }

    pub(super) fn set(service: &str, key: &str, value: &str) -> Result<(), SecretStoreError> {
        if value.len() > MAX_SECRET_VALUE_BYTES {
            return Err(SecretStoreError::TooLarge(MAX_SECRET_VALUE_BYTES));
        }
        let encoded = format!("fm1:{}", base64_url_encode(value.as_bytes()));
        let mut child = Command::new("/usr/bin/security")
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                if err.kind() == ErrorKind::NotFound {
                    SecretStoreError::Unavailable("/usr/bin/security not found".into())
                } else {
                    SecretStoreError::Backend(format!("Spawn /usr/bin/security: {err}"))
                }
            })?;
        if let Some(mut stdin) = child.stdin.take() {
            writeln!(
                stdin,
                "add-generic-password -U -s {service} -a {key} -w {encoded}"
            )
            .map_err(|err| SecretStoreError::Backend(format!("Write to keychain stdin: {err}")))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|err| SecretStoreError::Backend(format!("Wait for keychain: {err}")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(SecretStoreError::Backend(format!(
                "Keychain write failed: {}",
                stderr.trim()
            )));
        }
        Ok(())
    }

    pub(super) fn delete(service: &str, key: &str) -> Result<(), SecretStoreError> {
        let out = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", service, "-a", key])
            .output()
            .map_err(|err| {
                if err.kind() == ErrorKind::NotFound {
                    SecretStoreError::Unavailable("/usr/bin/security not found".into())
                } else {
                    SecretStoreError::Backend(format!("Run /usr/bin/security: {err}"))
                }
            })?;
        if !out.status.success() && out.status.code() != Some(ITEM_NOT_FOUND_EXIT) {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(SecretStoreError::Backend(format!(
                "Keychain delete failed: {}",
                stderr.trim()
            )));
        }
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use super::{MAX_SECRET_VALUE_BYTES, SecretStoreError};
    use std::{
        io::{ErrorKind, Write},
        process::{Command, Stdio},
    };

    fn map_io_error(err: std::io::Error) -> SecretStoreError {
        if err.kind() == ErrorKind::NotFound {
            SecretStoreError::Unavailable("secret-tool (libsecret) is not installed".into())
        } else {
            SecretStoreError::Backend(format!("Run secret-tool: {err}"))
        }
    }

    pub(super) fn get(service: &str, key: &str) -> Result<Option<String>, SecretStoreError> {
        let out = Command::new("secret-tool")
            .args(["lookup", "service", service, "account", key])
            .output()
            .map_err(map_io_error)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let trimmed = stderr.trim();
            if trimmed.is_empty() && out.status.code() == Some(1) {
                return Ok(None);
            }
            if trimmed.contains("D-Bus") || trimmed.contains("dbus") {
                return Err(SecretStoreError::Unavailable(trimmed.to_owned()));
            }
            return Err(SecretStoreError::Backend(format!(
                "secret-tool lookup failed: {trimmed}"
            )));
        }
        String::from_utf8(out.stdout)
            .map(Some)
            .map_err(|_| SecretStoreError::Backend("Stored secret is not valid UTF-8".into()))
    }

    pub(super) fn set(service: &str, key: &str, value: &str) -> Result<(), SecretStoreError> {
        if value.len() > MAX_SECRET_VALUE_BYTES {
            return Err(SecretStoreError::TooLarge(MAX_SECRET_VALUE_BYTES));
        }
        let mut child = Command::new("secret-tool")
            .args([
                "store",
                "--label=Fastman",
                "service",
                service,
                "account",
                key,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(map_io_error)?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(value.as_bytes())
                .map_err(|err| SecretStoreError::Backend(format!("Write secret stdin: {err}")))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|err| SecretStoreError::Backend(format!("Wait secret-tool: {err}")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let trimmed = stderr.trim();
            if trimmed.contains("D-Bus") || trimmed.contains("dbus") {
                return Err(SecretStoreError::Unavailable(trimmed.to_owned()));
            }
            return Err(SecretStoreError::Backend(format!(
                "secret-tool store failed: {trimmed}"
            )));
        }
        Ok(())
    }

    pub(super) fn delete(service: &str, key: &str) -> Result<(), SecretStoreError> {
        let out = Command::new("secret-tool")
            .args(["clear", "service", service, "account", key])
            .output()
            .map_err(map_io_error)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let trimmed = stderr.trim();
            if trimmed.contains("D-Bus") || trimmed.contains("dbus") {
                return Err(SecretStoreError::Unavailable(trimmed.to_owned()));
            }
            return Err(SecretStoreError::Backend(format!(
                "secret-tool clear failed: {trimmed}"
            )));
        }
        Ok(())
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    use super::SecretStoreError;

    pub(super) fn get(_service: &str, _key: &str) -> Result<Option<String>, SecretStoreError> {
        Err(SecretStoreError::Unavailable(
            "OS credential store not supported on this platform".into(),
        ))
    }

    pub(super) fn set(_service: &str, _key: &str, _value: &str) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Unavailable(
            "OS credential store not supported on this platform".into(),
        ))
    }

    pub(super) fn delete(_service: &str, _key: &str) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Unavailable(
            "OS credential store not supported on this platform".into(),
        ))
    }
}

impl SecretStore for OsSecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretStoreError> {
        Self::validate_inputs(key, &self.service)?;
        platform::get(&self.service, key)
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretStoreError> {
        Self::validate_inputs(key, &self.service)?;
        platform::set(&self.service, key, value)
    }

    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        Self::validate_inputs(key, &self.service)?;
        platform::delete(&self.service, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_secret_keys_and_env_var_names() {
        assert!(validate_secret_key("fastman:proj-1:base:uuid").is_ok());
        assert!(validate_secret_key("").is_err());
        assert!(validate_secret_key("bad/../key").is_err());
        assert!(validate_secret_key("bad key").is_err());
        assert!(validate_secret_key("bad\"key").is_err());
        assert!(validate_secret_key(&"a".repeat(257)).is_err());

        assert!(validate_env_var_name("API_KEY"));
        assert!(validate_env_var_name("_PRIVATE_2"));
        assert!(!validate_env_var_name(""));
        assert!(!validate_env_var_name("2FAST"));
        assert!(!validate_env_var_name("BAD-NAME"));
        assert!(!validate_env_var_name(&"A".repeat(129)));
    }

    #[test]
    fn memory_secret_store_roundtrip_and_limits() {
        let store = MemorySecretStore::new();
        let key = make_keychain_key("proj-1", "base");
        assert!(validate_secret_key(&key).is_ok());
        assert_eq!(store.get(&key).unwrap(), None);

        store.set(&key, "secret-value-1").unwrap();
        assert_eq!(store.get(&key).unwrap().as_deref(), Some("secret-value-1"));

        store.set(&key, "updated-value").unwrap();
        assert_eq!(store.get(&key).unwrap().as_deref(), Some("updated-value"));

        let oversized = "x".repeat(MAX_SECRET_VALUE_BYTES + 1);
        assert!(matches!(
            store.set(&key, &oversized),
            Err(SecretStoreError::TooLarge(_))
        ));
        assert_eq!(store.get(&key).unwrap().as_deref(), Some("updated-value"));

        store.delete(&key).unwrap();
        assert_eq!(store.get(&key).unwrap(), None);
        store.delete(&key).unwrap();
    }

    #[test]
    fn base64_url_roundtrips_arbitrary_utf8_and_binary_lengths() {
        for raw in ["", "a", "ab", "abc", "hello\nworld\"quote'!", "café 🚀"] {
            let encoded = base64_url_encode(raw.as_bytes());
            assert!(!encoded.contains(['+', '/', '=']));
            let decoded = base64_url_decode(&encoded).unwrap();
            assert_eq!(decoded, raw.as_bytes());
        }
        assert!(base64_url_decode("a").is_none());
        assert!(base64_url_decode("!!").is_none());
    }

    #[test]
    #[ignore = "touches OS credential store"]
    fn os_secret_store_roundtrip() {
        struct Cleanup<'a>(&'a OsSecretStore, String);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = self.0.delete(&self.1);
            }
        }

        let store = OsSecretStore::with_service("fastman-test");
        let key = format!("test-roundtrip-{}", Uuid::new_v4());
        let _cleanup = Cleanup(&store, key.clone());

        assert_eq!(store.get(&key).unwrap(), None);
        store.set(&key, "os-canary-\"line1\nline2\"-café").unwrap();
        assert_eq!(
            store.get(&key).unwrap().as_deref(),
            Some("os-canary-\"line1\nline2\"-café")
        );
        store.delete(&key).unwrap();
        assert_eq!(store.get(&key).unwrap(), None);
    }
}
