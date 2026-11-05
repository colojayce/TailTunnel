use argon2::{Algorithm, Argon2, Params, Version as ArgonVersion};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, Payload},
    Key, KeyInit, XChaCha20Poly1305, XNonce,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const FORMAT: &str = "tailtunnel.vault";
pub const VERSION: u32 = 1;
pub const WRONG_PASSWORD: &str = "密码不对或文件已损坏。";
pub const KEY_CHANGED: &str = "密码代次已更新，请重新输入密码。";

const KDF_ALGO: &str = "argon2id";
const AEAD_ALGO: &str = "xchacha20-poly1305";
const KDF_MIB: u32 = 64;
const KDF_TIME: u32 = 3;
const KDF_LANES: u32 = 1;
const MIN_MIB: u32 = 8;
const MAX_MIB: u32 = 512;
pub const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MIN_PASSWORD_LEN: usize = 10;
pub const MAX_PASSWORD_LEN: usize = 32;
/// 除字母和数字外，新密码只允许这些符号：在各种键盘上都好打，
/// 也不会在 shell 或 JSON 里被转义。
pub const PASSWORD_SYMBOLS: &str = "!@#$%^&*-_+=.?";

/// Rules for a password being set. Opening an existing file never checks them,
/// so tightening the rules later cannot lock anyone out of an old backup.
pub fn check_new_password(password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("密码不能为空。".into());
    }
    if !password
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || PASSWORD_SYMBOLS.contains(c))
    {
        return Err(format!("密码只能用字母、数字和这些符号：{PASSWORD_SYMBOLS}"));
    }
    // 上面已限定为 ASCII，字节数就是字符数。
    if password.len() < MIN_PASSWORD_LEN {
        return Err(format!("密码至少要 {MIN_PASSWORD_LEN} 个字符。"));
    }
    if password.len() > MAX_PASSWORD_LEN {
        return Err(format!("密码最多 {MAX_PASSWORD_LEN} 个字符。"));
    }
    if !password.chars().any(|c| c.is_ascii_alphabetic())
        || !password.chars().any(|c| c.is_ascii_digit())
    {
        return Err("密码要同时包含字母和数字。".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Kdf {
    pub algo: String,
    pub salt: String,
    pub mib: u32,
    pub time: u32,
    pub lanes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Cipher {
    pub algo: String,
    pub nonce: String,
}

/// The cleartext part of a vault file. It carries no secrets: it only says
/// which config this is, which password generation encrypted it, and how new
/// the contents are. Every field is authenticated as associated data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VaultHeader {
    pub format: String,
    pub version: u32,
    pub vault_id: String,
    pub key_id: String,
    pub label: String,
    pub device: String,
    pub saved_at: u64,
    pub revision: u64,
    pub kdf: Kdf,
    pub aead: Cipher,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFile {
    #[serde(flatten)]
    pub header: VaultHeader,
    pub ciphertext: String,
}

/// A cached derived key. A running app keeps one of these so saving again does
/// not need the password or another Argon2 run. The 32 key bytes zero on drop.
pub struct VaultKey {
    vault_id: String,
    key_id: String,
    salt: [u8; SALT_LEN],
    mib: u32,
    time: u32,
    lanes: u32,
    key: Zeroizing<[u8; 32]>,
}

impl VaultKey {
    pub fn vault_id(&self) -> &str {
        &self.vault_id
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    fn from_header(header: &VaultHeader, key: Zeroizing<[u8; 32]>) -> Result<Self, String> {
        let salt: [u8; SALT_LEN] = decode_fixed(&header.kdf.salt, SALT_LEN)?
            .try_into()
            .map_err(|_| "配置文件已损坏。".to_string())?;
        Ok(Self {
            vault_id: header.vault_id.clone(),
            key_id: header.key_id.clone(),
            salt,
            mib: header.kdf.mib,
            time: header.kdf.time,
            lanes: header.kdf.lanes,
            key,
        })
    }

    fn kdf(&self) -> Kdf {
        Kdf {
            algo: KDF_ALGO.into(),
            salt: STANDARD.encode(self.salt),
            mib: self.mib,
            time: self.time,
            lanes: self.lanes,
        }
    }
}

impl std::fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultKey")
            .field("vault_id", &self.vault_id)
            .field("key_id", &self.key_id)
            .field("key", &"<redacted>")
            .finish()
    }
}

/// The parts of a save that may change without changing the key generation.
pub struct SealMeta {
    pub label: String,
    pub device: String,
    pub saved_at: u64,
    pub revision: u64,
}

pub struct SealedVault {
    pub text: String,
    pub key: VaultKey,
}

pub struct OpenedVault {
    pub header: VaultHeader,
    /// Plaintext config, including SSH passwords; zeroes on drop.
    pub payload: Zeroizing<Vec<u8>>,
    pub key: VaultKey,
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn new_salt() -> Result<[u8; SALT_LEN], String> {
    let mut salt = [0u8; SALT_LEN];
    getrandom::getrandom(&mut salt).map_err(|error| format!("无法生成随机数：{error}"))?;
    Ok(salt)
}

/// Encrypts with a fresh password generation. The password must pass
/// `check_new_password`. A new `key_id` and a new salt
/// are generated internally, so callers cannot pair an id with the wrong salt.
/// Pass `Some(vault_id)` only when changing the password of an existing config;
/// leave it `None` for a brand-new config.
///
/// This runs Argon2, which is deliberately CPU and memory heavy. Call it from
/// `#[tauri::command(async)]` or another worker thread, never the main thread.
pub fn seal_new_password(
    password: &str,
    vault_id: Option<&str>,
    meta: &SealMeta,
    payload: &[u8],
) -> Result<SealedVault, String> {
    check_new_password(password)?;
    let vault_id = match vault_id {
        Some(id) if canonical_uuid(id) => id.to_string(),
        Some(_) => return Err("配置文件已损坏。".into()),
        None => new_id(),
    };
    let salt = new_salt()?;
    let kdf = Kdf {
        algo: KDF_ALGO.into(),
        salt: STANDARD.encode(salt),
        mib: KDF_MIB,
        time: KDF_TIME,
        lanes: KDF_LANES,
    };
    let key = derive_key(password, &salt, &kdf)?;
    let key = VaultKey {
        vault_id,
        key_id: new_id(),
        salt,
        mib: KDF_MIB,
        time: KDF_TIME,
        lanes: KDF_LANES,
        key,
    };
    let text = seal_with_key(&key, meta, payload)?;
    Ok(SealedVault { text, key })
}

/// Re-encrypts with an already derived key. The config identity, key
/// generation, salt and KDF parameters are reused; only the metadata, payload
/// and nonce change.
pub fn seal_with_key(key: &VaultKey, meta: &SealMeta, payload: &[u8]) -> Result<String, String> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|error| format!("无法生成随机数：{error}"))?;
    let header = VaultHeader {
        format: FORMAT.into(),
        version: VERSION,
        vault_id: key.vault_id.clone(),
        key_id: key.key_id.clone(),
        label: meta.label.clone(),
        device: meta.device.clone(),
        saved_at: meta.saved_at,
        revision: meta.revision,
        kdf: key.kdf(),
        aead: Cipher {
            algo: AEAD_ALGO.into(),
            nonce: STANDARD.encode(nonce),
        },
    };
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&*key.key));
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: payload,
                aad: header_aad(&header).as_bytes(),
            },
        )
        .map_err(|_| "加密失败。".to_string())?;
    let file = VaultFile {
        header,
        ciphertext: STANDARD.encode(ciphertext),
    };
    serde_json::to_string_pretty(&file).map_err(|error| format!("生成配置失败：{error}"))
}

/// Decrypts and returns the payload together with the derived key, so the
/// caller can keep saving without asking for the password again.
///
/// This runs Argon2, which is deliberately CPU and memory heavy. Call it from
/// `#[tauri::command(async)]` or another worker thread, never the main thread.
pub fn open_with_password(password: &str, text: &str) -> Result<OpenedVault, String> {
    let file = parse_file(text)?;
    validate(&file.header)?;
    let salt = decode_fixed(&file.header.kdf.salt, SALT_LEN)?;
    let key = derive_key(password, &salt, &file.header.kdf)?;
    let payload = decrypt_payload(&file.header, &file.ciphertext, &key)?;
    let key = VaultKey::from_header(&file.header, key)?;
    Ok(OpenedVault {
        header: file.header,
        payload,
        key,
    })
}

/// Decrypts with a cached key. A file from another password generation is
/// rejected before decryption, so the caller can ask for the new password.
pub fn open_with_key(key: VaultKey, text: &str) -> Result<OpenedVault, String> {
    let file = parse_file(text)?;
    validate(&file.header)?;
    if file.header.vault_id != key.vault_id
        || file.header.key_id != key.key_id
        || file.header.kdf.salt != STANDARD.encode(key.salt)
        || file.header.kdf.mib != key.mib
        || file.header.kdf.time != key.time
        || file.header.kdf.lanes != key.lanes
    {
        return Err(KEY_CHANGED.into());
    }
    let payload = decrypt_payload(&file.header, &file.ciphertext, &key.key)?;
    Ok(OpenedVault {
        header: file.header,
        payload,
        key,
    })
}

fn decrypt_payload(
    header: &VaultHeader,
    ciphertext: &str,
    key: &[u8; 32],
) -> Result<Zeroizing<Vec<u8>>, String> {
    let nonce = decode_fixed(&header.aead.nonce, NONCE_LEN)?;
    let ciphertext = STANDARD
        .decode(ciphertext)
        .map_err(|_| "配置文件已损坏。".to_string())?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: header_aad(header).as_bytes(),
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| WRONG_PASSWORD.to_string())
}

/// Reads the cleartext header without asking for the password. Used for
/// listing a backup folder and deciding which remembered password applies.
///
/// Its fields are unauthenticated until a successful open: compare `revision`
/// or `saved_at` only against the header returned by `open_with_password` or
/// `open_with_key`, never against this one.
pub fn parse_header(text: &str) -> Result<VaultHeader, String> {
    let file = parse_file(text)?;
    validate(&file.header)?;
    Ok(file.header)
}

fn parse_file(text: &str) -> Result<VaultFile, String> {
    if text.len() > MAX_FILE_BYTES {
        return Err("配置文件过大。".into());
    }
    serde_json::from_str(text).map_err(|error| format!("配置文件无法解析：{error}"))
}

/// Only canonical lowercase hyphenated UUIDs are accepted, so the same id has
/// exactly one spelling.
fn canonical_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .map(|id| id.hyphenated().to_string() == value)
        .unwrap_or(false)
}

fn validate(header: &VaultHeader) -> Result<(), String> {
    if header.format != FORMAT {
        return Err("这不是 TailTunnel 的加密配置。".into());
    }
    if header.version != VERSION {
        return Err(format!("不支持的配置版本：{}。", header.version));
    }
    // Keychain accounts are `vault_id:key_id`; non-canonical UUIDs let two
    // different joins collide.
    if !canonical_uuid(&header.vault_id) || !canonical_uuid(&header.key_id) {
        return Err("配置文件已损坏。".into());
    }
    if header.kdf.algo != KDF_ALGO || header.aead.algo != AEAD_ALGO {
        return Err("不支持的加密算法。".into());
    }
    if !(MIN_MIB..=MAX_MIB).contains(&header.kdf.mib)
        || !(1..=10).contains(&header.kdf.time)
        || !(1..=8).contains(&header.kdf.lanes)
    {
        return Err("加密参数超出范围。".into());
    }
    decode_fixed(&header.kdf.salt, SALT_LEN)?;
    decode_fixed(&header.aead.nonce, NONCE_LEN)?;
    Ok(())
}

/// The header bytes are authenticated as AAD, so nobody can swap the config
/// identity, downgrade the KDF, or relabel a file without breaking the tag.
fn header_aad(header: &VaultHeader) -> String {
    let mut out = String::with_capacity(256);
    push_field(&mut out, &header.format);
    push_field(&mut out, &header.version.to_string());
    push_field(&mut out, &header.vault_id);
    push_field(&mut out, &header.key_id);
    push_field(&mut out, &header.label);
    push_field(&mut out, &header.device);
    push_field(&mut out, &header.saved_at.to_string());
    push_field(&mut out, &header.revision.to_string());
    push_field(&mut out, &header.kdf.algo);
    push_field(&mut out, &header.kdf.salt);
    push_field(&mut out, &header.kdf.mib.to_string());
    push_field(&mut out, &header.kdf.time.to_string());
    push_field(&mut out, &header.kdf.lanes.to_string());
    push_field(&mut out, &header.aead.algo);
    push_field(&mut out, &header.aead.nonce);
    out
}

fn push_field(out: &mut String, value: &str) {
    out.push_str(&value.len().to_string());
    out.push(':');
    out.push_str(value);
    out.push(';');
}

fn derive_key(password: &str, salt: &[u8], kdf: &Kdf) -> Result<Zeroizing<[u8; 32]>, String> {
    if password.is_empty() {
        return Err("密码不能为空。".into());
    }
    let params = Params::new(kdf.mib * 1024, kdf.time, kdf.lanes, Some(32))
        .map_err(|error| format!("加密参数无效：{error}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, ArgonVersion::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut *key)
        .map_err(|error| format!("生成密钥失败：{error}"))?;
    Ok(key)
}

fn decode_fixed(value: &str, len: usize) -> Result<Vec<u8>, String> {
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| "配置文件已损坏。".to_string())?;
    if bytes.len() != len {
        return Err("配置文件已损坏。".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "Tunnel2026!x";
    const PAYLOAD: &[u8] =
        r#"{"tunnels":[{"id":"a","host":"nas","note":"家里的 NAS"}]}"#.as_bytes();

    fn meta(revision: u64) -> SealMeta {
        SealMeta {
            label: "工作隧道".into(),
            device: "test-mac".into(),
            saved_at: 1_762_348_212,
            revision,
        }
    }

    fn sealed() -> SealedVault {
        seal_new_password(PASSWORD, None, &meta(3), PAYLOAD).unwrap()
    }

    #[test]
    fn round_trips_payload_and_header() {
        let sealed = sealed();
        let vault_id = sealed.key.vault_id().to_string();
        let key_id = sealed.key.key_id().to_string();
        let opened = open_with_password(PASSWORD, &sealed.text).unwrap();
        assert_eq!(&*opened.payload, PAYLOAD);
        assert_eq!(opened.header.vault_id, vault_id);
        assert_eq!(opened.header.key_id, key_id);
        assert_eq!(opened.header.label, "工作隧道");
        assert_eq!(opened.header.revision, 3);
    }

    #[test]
    fn wrong_password_is_rejected() {
        let text = sealed().text;
        assert_eq!(
            open_with_password("wrong password", &text).err().unwrap(),
            WRONG_PASSWORD
        );
    }

    #[test]
    fn header_is_readable_without_password() {
        let sealed = sealed();
        let header = parse_header(&sealed.text).unwrap();
        assert_eq!(header.vault_id, sealed.key.vault_id());
        assert_eq!(header.key_id, sealed.key.key_id());
        assert_eq!(header.label, "工作隧道");
        assert_eq!(header.revision, 3);
    }

    #[test]
    fn tampered_metadata_is_rejected() {
        let text = sealed().text;
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["label"] = serde_json::json!("被改过的名字");
        let tampered = serde_json::to_string(&value).unwrap();
        assert_eq!(
            open_with_password(PASSWORD, &tampered).err().unwrap(),
            WRONG_PASSWORD
        );
    }

    #[test]
    fn rejects_kdf_parameters_outside_bounds() {
        let text = sealed().text;
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["kdf"]["mib"] = serde_json::json!(1);
        let bad = serde_json::to_string(&value).unwrap();
        let error = parse_header(&bad).unwrap_err();
        assert!(error.contains("加密参数"), "{error}");
        assert!(open_with_password(PASSWORD, &bad).is_err());
    }

    #[test]
    fn resaving_keeps_identity_but_changes_nonce() {
        let first = sealed();
        let before = parse_header(&first.text).unwrap();
        let text = seal_with_key(&first.key, &meta(4), PAYLOAD).unwrap();
        let after = parse_header(&text).unwrap();
        assert_eq!(before.vault_id, after.vault_id);
        assert_eq!(before.key_id, after.key_id);
        assert_eq!(before.kdf.salt, after.kdf.salt);
        assert_ne!(before.aead.nonce, after.aead.nonce);
        assert_eq!(after.revision, 4);
        let opened = open_with_password(PASSWORD, &text).unwrap();
        assert_eq!(&*opened.payload, PAYLOAD);
    }

    #[test]
    fn new_password_changes_key_id_and_salt() {
        let first = sealed();
        let vault_id = first.key.vault_id().to_string();
        let second = seal_new_password("Rotate2026-key", Some(&vault_id), &meta(4), PAYLOAD).unwrap();
        assert_eq!(second.key.vault_id(), vault_id);
        assert_ne!(second.key.key_id(), first.key.key_id());
        let before = parse_header(&first.text).unwrap();
        let after = parse_header(&second.text).unwrap();
        assert_ne!(before.kdf.salt, after.kdf.salt);
        assert_eq!(
            open_with_key(first.key, &second.text).err().unwrap(),
            KEY_CHANGED
        );
        assert_eq!(
            open_with_password(PASSWORD, &second.text).err().unwrap(),
            WRONG_PASSWORD
        );
        assert!(open_with_password("Rotate2026-key", &second.text).is_ok());
    }

    #[test]
    fn opens_with_a_cached_key() {
        let SealedVault { text, key } = sealed();
        let opened = open_with_key(key, &text).unwrap();
        assert_eq!(&*opened.payload, PAYLOAD);
        assert_eq!(opened.header.revision, 3);
    }

    #[test]
    fn rejects_ids_that_are_not_canonical_uuids() {
        for (field, bad_id) in [
            ("vaultId", "a:b"),
            ("keyId", "not-a-uuid"),
            ("keyId", "A987FBC9-4BED-3078-CF07-9141BA07C9F3"),
        ] {
            let text = sealed().text;
            let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
            value[field] = serde_json::json!(bad_id);
            let bad = serde_json::to_string(&value).unwrap();
            assert_eq!(parse_header(&bad).unwrap_err(), "配置文件已损坏。");
        }
        assert!(seal_new_password(PASSWORD, Some("vault-1"), &meta(3), PAYLOAD).is_err());
    }

    fn new_password_error(password: &str) -> String {
        seal_new_password(password, None, &meta(3), PAYLOAD)
            .err()
            .unwrap()
    }

    #[test]
    fn new_password_needs_at_least_ten_characters() {
        assert_eq!(new_password_error("Abc12345!"), "密码至少要 10 个字符。");
        assert!(seal_new_password("Abc123456!", None, &meta(3), PAYLOAD).is_ok());
    }

    #[test]
    fn new_password_has_a_length_cap() {
        assert!(check_new_password(&"a1".repeat(16)).is_ok());
        let too_long = format!("{}x", "a1".repeat(16));
        assert_eq!(new_password_error(&too_long), "密码最多 32 个字符。");
    }

    #[test]
    fn new_password_needs_a_letter_and_a_digit() {
        for password in ["abcdefghij", "1234567890", "!@#$%^&*-_"] {
            assert_eq!(new_password_error(password), "密码要同时包含字母和数字。");
        }
    }

    #[test]
    fn new_password_allows_only_letters_digits_and_listed_symbols() {
        let expected = format!("密码只能用字母、数字和这些符号：{PASSWORD_SYMBOLS}");
        for password in [
            "Tunnel 2026x",
            "Tunnel2026\"x",
            "Tunnel2026\\x",
            "Tunnel2026~x",
            "Tunnel2026中文",
            "Tunnel2026é",
        ] {
            assert_eq!(new_password_error(password), expected, "{password}");
        }
        for symbol in PASSWORD_SYMBOLS.chars() {
            let password = format!("Tunnel2026{symbol}");
            assert!(check_new_password(&password).is_ok(), "{password}");
        }
    }

    #[test]
    fn opening_does_not_apply_the_new_password_rules() {
        // 规则只管设新密码；以后改规则也不能把旧备份锁在外面。
        let salt = new_salt().unwrap();
        let kdf = Kdf {
            algo: KDF_ALGO.into(),
            salt: STANDARD.encode(salt),
            mib: KDF_MIB,
            time: KDF_TIME,
            lanes: KDF_LANES,
        };
        let key = VaultKey {
            vault_id: new_id(),
            key_id: new_id(),
            salt,
            mib: KDF_MIB,
            time: KDF_TIME,
            lanes: KDF_LANES,
            key: derive_key("short", &salt, &kdf).unwrap(),
        };
        let text = seal_with_key(&key, &meta(3), PAYLOAD).unwrap();
        let opened = open_with_password("short", &text).unwrap();
        assert_eq!(&*opened.payload, PAYLOAD);
    }

    #[test]
    fn empty_password_is_rejected() {
        assert_eq!(
            seal_new_password("", None, &meta(3), PAYLOAD).err().unwrap(),
            "密码不能为空。"
        );
    }

    #[test]
    fn plain_json_is_not_a_vault() {
        let error = open_with_password(PASSWORD, r#"{"tunnels":[]}"#).err().unwrap();
        assert!(error.contains("配置"), "{error}");
    }
}
