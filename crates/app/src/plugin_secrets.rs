//! Plugin-private secrets protected with Windows DPAPI.
//!
//! Each value is encrypted for the current Windows user with an entropy value
//! derived from the plugin profile and secret key. The data directory contains
//! only bounded encrypted blobs and hex-encoded secret names, never plaintext
//! values. DPAPI-encrypted files cannot be read by another Windows user.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub(crate) const MAX_SECRET_KEY_BYTES: usize = 128;
pub(crate) const MAX_SECRET_VALUE_BYTES: usize = 2560;
pub(crate) const MAX_SECRETS_PER_PLUGIN: usize = 64;

const SECRET_DIR: &str = "plugin-secrets";
const SECRET_EXTENSION: &str = "dpapi";
const MAX_ENCRYPTED_SECRET_BYTES: u64 = (MAX_SECRET_VALUE_BYTES + 4096) as u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SecretStoreError {
    InvalidKey,
    ResourceExhausted(&'static str),
    #[cfg(not(target_os = "windows"))]
    Unavailable,
    HostFailure(String),
}

pub(crate) fn validate_secret_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_SECRET_KEY_BYTES
        && key.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => true,
            b'.' | b'_' | b'-' => index > 0,
            _ => false,
        })
}

pub(crate) struct SecretStore {
    directory: PathBuf,
    namespace: String,
}

impl SecretStore {
    pub(crate) fn new(data_dir: &Path, plugin_id: &str) -> Self {
        let profile = Sha256::digest(data_dir.to_string_lossy().as_bytes());
        let plugin = Sha256::digest(plugin_id.as_bytes());
        let namespace = format!(
            "{}/{}",
            encode_hex(&profile[..16]),
            encode_hex(&plugin[..16])
        );
        Self {
            directory: data_dir
                .join(SECRET_DIR)
                .join(encode_hex(plugin_id.as_bytes())),
            namespace,
        }
    }

    fn secret_path(&self, key: &str) -> Result<PathBuf, SecretStoreError> {
        if !validate_secret_key(key) {
            return Err(SecretStoreError::InvalidKey);
        }
        Ok(self
            .directory
            .join(format!("{}.{SECRET_EXTENSION}", encode_hex(key.as_bytes()))))
    }

    fn entropy(&self, key: &str) -> Vec<u8> {
        let mut digest = Sha256::new();
        digest.update(self.namespace.as_bytes());
        digest.update([0]);
        digest.update(key.as_bytes());
        digest.finalize().to_vec()
    }

    pub(crate) fn list(&self) -> Result<Vec<String>, SecretStoreError> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(SecretStoreError::HostFailure(format!(
                    "read plugin secret directory: {error}"
                )));
            }
        };
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                SecretStoreError::HostFailure(format!("read plugin secret entry: {error}"))
            })?;
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(encoded) = name.strip_suffix(&format!(".{SECRET_EXTENSION}")) else {
                continue;
            };
            let Some(key) = decode_hex_utf8(encoded) else {
                continue;
            };
            if validate_secret_key(&key) {
                keys.push(key);
            }
        }
        keys.sort();
        keys.dedup();
        keys.truncate(MAX_SECRETS_PER_PLUGIN);
        Ok(keys)
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn get(&self, key: &str) -> Result<Option<String>, SecretStoreError> {
        let path = self.secret_path(key)?;
        let encrypted = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(SecretStoreError::HostFailure(format!(
                    "read plugin secret: {error}"
                )));
            }
        };
        if encrypted.len() as u64 > MAX_ENCRYPTED_SECRET_BYTES {
            return Err(SecretStoreError::ResourceExhausted(
                "encrypted secret exceeds its storage limit",
            ));
        }
        let plaintext = unprotect(&encrypted, &self.entropy(key))?;
        if plaintext.len() > MAX_SECRET_VALUE_BYTES {
            return Err(SecretStoreError::ResourceExhausted(
                "stored secret exceeds the 2560 byte limit",
            ));
        }
        String::from_utf8(plaintext)
            .map(Some)
            .map_err(|_| SecretStoreError::HostFailure("stored secret is not valid UTF-8".into()))
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn set(&self, key: &str, value: &str) -> Result<(), SecretStoreError> {
        let path = self.secret_path(key)?;
        if value.len() > MAX_SECRET_VALUE_BYTES {
            return Err(SecretStoreError::ResourceExhausted(
                "secret value exceeds the 2560 byte limit",
            ));
        }
        if !path.exists() && self.list()?.len() >= MAX_SECRETS_PER_PLUGIN {
            return Err(SecretStoreError::ResourceExhausted(
                "plugin has reached the 64 secret limit",
            ));
        }
        let encrypted = protect(value.as_bytes(), &self.entropy(key))?;
        persist_atomic(&path, &encrypted)
    }

    pub(crate) fn delete(&self, key: &str) -> Result<bool, SecretStoreError> {
        let path = self.secret_path(key)?;
        match std::fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(SecretStoreError::HostFailure(format!(
                "delete plugin secret: {error}"
            ))),
        }
    }
}

pub(crate) fn remove_plugin_secrets(
    data_dir: &Path,
    plugin_id: &str,
) -> Result<bool, SecretStoreError> {
    let store = SecretStore::new(data_dir, plugin_id);
    match std::fs::remove_dir_all(&store.directory) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(SecretStoreError::HostFailure(format!(
            "remove plugin secret directory: {error}"
        ))),
    }
}

#[cfg(not(target_os = "windows"))]
impl SecretStore {
    pub(crate) fn get(&self, key: &str) -> Result<Option<String>, SecretStoreError> {
        if !validate_secret_key(key) {
            Err(SecretStoreError::InvalidKey)
        } else {
            Err(SecretStoreError::Unavailable)
        }
    }

    pub(crate) fn set(&self, key: &str, _value: &str) -> Result<(), SecretStoreError> {
        if !validate_secret_key(key) {
            Err(SecretStoreError::InvalidKey)
        } else {
            Err(SecretStoreError::Unavailable)
        }
    }
}

fn persist_atomic(path: &Path, bytes: &[u8]) -> Result<(), SecretStoreError> {
    let Some(parent) = path.parent() else {
        return Err(SecretStoreError::HostFailure(
            "plugin secret has no parent directory".into(),
        ));
    };
    std::fs::create_dir_all(parent).map_err(|error| {
        SecretStoreError::HostFailure(format!("create plugin secret directory: {error}"))
    })?;
    let temporary = path.with_extension(format!("{SECRET_EXTENSION}.tmp"));
    std::fs::write(&temporary, bytes)
        .map_err(|error| SecretStoreError::HostFailure(format!("write plugin secret: {error}")))?;
    if path.exists() {
        std::fs::remove_file(path).map_err(|error| {
            SecretStoreError::HostFailure(format!("replace plugin secret: {error}"))
        })?;
    }
    std::fs::rename(&temporary, path)
        .map_err(|error| SecretStoreError::HostFailure(format!("commit plugin secret: {error}")))
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

fn decode_hex_utf8(encoded: &str) -> Option<String> {
    if !encoded.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..encoded.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(encoded.get(index..index + 2)?, 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

#[cfg(all(target_os = "windows", not(test)))]
fn protect(plaintext: &[u8], entropy: &[u8]) -> Result<Vec<u8>, SecretStoreError> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: plaintext.len() as u32,
        pbData: plaintext.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    }
    .map_err(|error| SecretStoreError::HostFailure(format!("protect plugin secret: {error}")))?;
    let encrypted = copy_and_free(output)?;
    Ok(encrypted)
}

#[cfg(all(target_os = "windows", not(test)))]
fn unprotect(encrypted: &[u8], entropy: &[u8]) -> Result<Vec<u8>, SecretStoreError> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: encrypted.len() as u32,
        pbData: encrypted.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    }
    .map_err(|error| SecretStoreError::HostFailure(format!("decrypt plugin secret: {error}")))?;
    copy_and_free(output)
}

#[cfg(all(target_os = "windows", not(test)))]
fn copy_and_free(output: CRYPT_INTEGER_BLOB) -> Result<Vec<u8>, SecretStoreError> {
    let bytes = if output.cbData == 0 {
        Vec::new()
    } else if output.pbData.is_null() {
        return Err(SecretStoreError::HostFailure(
            "DPAPI returned an empty output pointer".into(),
        ));
    } else {
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() }
    };
    unsafe {
        LocalFree(Some(HLOCAL(output.pbData.cast())));
    }
    Ok(bytes)
}

#[cfg(all(target_os = "windows", not(test)))]
use windows::core::PCWSTR;
#[cfg(all(target_os = "windows", not(test)))]
use windows::Win32::Foundation::{LocalFree, HLOCAL};
#[cfg(all(target_os = "windows", not(test)))]
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

// The test runner cannot open its Windows logon profile for DPAPI. Tests
// exercise storage semantics with an authenticated local cipher; release
// builds compile only the DPAPI implementation above.
#[cfg(all(target_os = "windows", test))]
fn protect(plaintext: &[u8], entropy: &[u8]) -> Result<Vec<u8>, SecretStoreError> {
    const TEST_HEADER: &[u8] = b"STEWARD-TEST-DPAPI\x01";
    let digest = test_digest(entropy, plaintext);
    let mask = test_mask(entropy);
    let mut output = Vec::with_capacity(TEST_HEADER.len() + digest.len() + plaintext.len());
    output.extend_from_slice(TEST_HEADER);
    output.extend_from_slice(&digest);
    output.extend(
        plaintext
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % mask.len()]),
    );
    Ok(output)
}

#[cfg(all(target_os = "windows", test))]
fn unprotect(encrypted: &[u8], entropy: &[u8]) -> Result<Vec<u8>, SecretStoreError> {
    const TEST_HEADER: &[u8] = b"STEWARD-TEST-DPAPI\x01";
    if encrypted.len() < TEST_HEADER.len() + 32 || !encrypted.starts_with(TEST_HEADER) {
        return Err(SecretStoreError::HostFailure(
            "decrypt plugin secret: invalid ciphertext".into(),
        ));
    }
    let digest = &encrypted[TEST_HEADER.len()..TEST_HEADER.len() + 32];
    let mask = test_mask(entropy);
    let plaintext = encrypted[TEST_HEADER.len() + 32..]
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ mask[index % mask.len()])
        .collect::<Vec<_>>();
    if digest != test_digest(entropy, &plaintext) {
        return Err(SecretStoreError::HostFailure(
            "decrypt plugin secret: ciphertext authentication failed".into(),
        ));
    }
    Ok(plaintext)
}

#[cfg(all(target_os = "windows", test))]
fn test_digest(entropy: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(b"steward-test-secret-digest\0");
    digest.update(entropy);
    digest.update([0]);
    digest.update(plaintext);
    digest.finalize().to_vec()
}

#[cfg(all(target_os = "windows", test))]
fn test_mask(entropy: &[u8]) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(b"steward-test-secret-mask\0");
    digest.update(entropy);
    digest.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    fn test_store(label: &str) -> (SecretStore, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!(
            "steward-secrets-{label}-{}-{unique}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let id = format!("com.example.pdx5.{label}");
        (SecretStore::new(&data_dir, &id), data_dir)
    }

    fn cleanup(store: &SecretStore, data_dir: &Path) {
        for key in store.list().unwrap_or_default() {
            let _ = store.delete(&key);
        }
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn keys_are_bounded_and_roundtrip_through_hex_filenames() {
        assert!(validate_secret_key("github-token"));
        assert!(validate_secret_key("API.v2_token"));
        assert!(!validate_secret_key(""));
        assert!(!validate_secret_key(".hidden"));
        assert!(!validate_secret_key("slash/name"));
        assert!(!validate_secret_key("wild*card"));
        assert!(!validate_secret_key(&"a".repeat(MAX_SECRET_KEY_BYTES + 1)));
        let encoded = encode_hex("API.v2_token".as_bytes());
        assert_eq!(decode_hex_utf8(&encoded).as_deref(), Some("API.v2_token"));
    }

    #[test]
    fn plugin_and_profile_paths_are_isolated() {
        let (first, first_dir) = test_store("profile-one");
        let (second, second_dir) = test_store("profile-two");
        assert_ne!(first.directory, second.directory);
        let other_plugin = SecretStore::new(&first_dir, "com.example.pdx5.other");
        assert_ne!(first.directory, other_plugin.directory);
        let _ = std::fs::remove_dir_all(first_dir);
        let _ = std::fs::remove_dir_all(second_dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn dpapi_roundtrip_replaces_lists_and_deletes_without_plaintext_on_disk() {
        let (store, data_dir) = test_store("roundtrip");
        let other = SecretStore::new(&data_dir, "com.example.pdx5.other");

        assert_eq!(store.get("github-token").unwrap(), None);
        store.set("github-token", "first-secret-value").unwrap();
        assert_eq!(
            store.get("github-token").unwrap().as_deref(),
            Some("first-secret-value")
        );
        assert_eq!(store.list().unwrap(), vec!["github-token"]);
        let encrypted_path = store.secret_path("github-token").unwrap();
        let encrypted = std::fs::read(&encrypted_path).unwrap();
        assert!(!encrypted
            .windows(b"first-secret-value".len())
            .any(|window| window == b"first-secret-value"));

        store.set("github-token", "second-secret-value").unwrap();
        assert_eq!(
            store.get("github-token").unwrap().as_deref(),
            Some("second-secret-value")
        );
        assert_eq!(store.get("missing").unwrap(), None);
        assert_eq!(other.get("github-token").unwrap(), None);
        assert!(other.list().unwrap().is_empty());

        assert!(store.delete("github-token").unwrap());
        assert!(!store.delete("github-token").unwrap());
        assert_eq!(store.get("github-token").unwrap(), None);
        cleanup(&store, &data_dir);
        cleanup(&other, &data_dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn secret_size_and_count_limits_fail_before_unbounded_growth() {
        let (store, data_dir) = test_store("limits");
        let oversized = "x".repeat(MAX_SECRET_VALUE_BYTES + 1);
        assert!(matches!(
            store.set("too-large", &oversized),
            Err(SecretStoreError::ResourceExhausted(_))
        ));

        for index in 0..MAX_SECRETS_PER_PLUGIN {
            store.set(&format!("secret-{index}"), "value").unwrap();
        }
        assert_eq!(store.list().unwrap().len(), MAX_SECRETS_PER_PLUGIN);
        assert!(matches!(
            store.set("one-too-many", "value"),
            Err(SecretStoreError::ResourceExhausted(_))
        ));
        store.set("secret-0", "replacement").unwrap();
        cleanup(&store, &data_dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn tampered_or_copied_ciphertext_fails_closed() {
        let (store, data_dir) = test_store("tamper");
        store.set("source", "source-value").unwrap();
        store.set("target", "target-value").unwrap();
        let source = std::fs::read(store.secret_path("source").unwrap()).unwrap();
        let target_path = store.secret_path("target").unwrap();
        std::fs::write(&target_path, source).unwrap();
        assert!(matches!(
            store.get("target"),
            Err(SecretStoreError::HostFailure(_))
        ));
        cleanup(&store, &data_dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn plugin_removal_deletes_the_whole_secret_namespace() {
        let (store, data_dir) = test_store("remove");
        store.set("source", "value").unwrap();
        assert!(remove_plugin_secrets(&data_dir, "com.example.pdx5.remove").unwrap());
        assert!(!store.directory.exists());
        assert!(!remove_plugin_secrets(&data_dir, "com.example.pdx5.remove").unwrap());
        let _ = std::fs::remove_dir_all(data_dir);
    }
}
