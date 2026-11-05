//! Where encrypted snapshots live and how this Mac remembers which vault is
//! its own. Only ids and the folder path are stored here; passwords stay in
//! the Keychain.

use crate::vault::{self, VaultHeader};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE_SUFFIX: &str = ".vault.json";

/// The folder inside iCloud Drive. iCloud syncs it across the user's Macs;
/// the app only ever writes encrypted files there.
pub fn default_folder() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library/Mobile Documents/com~apple~CloudDocs")
            .join("TailTunnel"),
    )
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultSettings {
    /// Empty means the default iCloud Drive folder.
    pub folder: String,
    /// This config's identity and current password generation.
    pub vault_id: String,
    pub key_id: String,
    pub label: String,
    pub revision: u64,
}

impl VaultSettings {
    pub fn folder_path(&self) -> Result<PathBuf, String> {
        if !self.folder.is_empty() {
            return Ok(PathBuf::from(&self.folder));
        }
        default_folder().ok_or_else(|| "找不到 iCloud Drive 文件夹。".to_string())
    }

    pub fn is_configured(&self) -> bool {
        !self.vault_id.is_empty() && !self.key_id.is_empty()
    }
}

pub fn settings_file(app_dir: &Path) -> PathBuf {
    app_dir.join("vault-settings.json")
}

pub fn load_settings(app_dir: &Path) -> VaultSettings {
    fs::read_to_string(settings_file(app_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_settings(app_dir: &Path, settings: &VaultSettings) -> Result<(), String> {
    let text = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("写入备份设置失败：{error}"))?;
    crate::model::write_private_atomic(&settings_file(app_dir), &text)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    pub file: String,
    pub label: String,
    pub device: String,
    pub saved_at: u64,
    pub revision: u64,
    pub vault_id: String,
    pub key_id: String,
}

impl BackupEntry {
    fn from_header(file: &str, header: VaultHeader) -> Self {
        Self {
            file: file.to_string(),
            label: header.label,
            device: header.device,
            saved_at: header.saved_at,
            revision: header.revision,
            vault_id: header.vault_id,
            key_id: header.key_id,
        }
    }
}

/// Lists snapshots newest first. Files that are not readable vaults are left
/// alone and skipped, so a stray text file in the folder cannot break the list.
pub fn list_backups(folder: &Path) -> Result<Vec<BackupEntry>, String> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("读备份文件夹失败：{error}")),
    };
    let mut backups = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.ends_with(FILE_SUFFIX) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(header) = vault::parse_header(&text) else {
            continue;
        };
        backups.push(BackupEntry::from_header(name, header));
    }
    backups.sort_by(|left, right| {
        right
            .saved_at
            .cmp(&left.saved_at)
            .then(right.revision.cmp(&left.revision))
            .then(right.file.cmp(&left.file))
    });
    Ok(backups)
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|span| span.as_secs())
        .unwrap_or_default()
}

/// Writes a new snapshot under a unique name, never overwriting another one.
pub fn write_backup(folder: &Path, vault_id: &str, text: &str) -> Result<BackupEntry, String> {
    fs::create_dir_all(folder).map_err(|error| format!("创建备份文件夹失败：{error}"))?;
    let short: String = vault_id.chars().take(8).collect();
    let stamp = now_seconds();
    let mut file = format!("tailtunnel-{stamp}-{short}{FILE_SUFFIX}");
    let mut attempt = 1;
    while folder.join(&file).exists() {
        file = format!("tailtunnel-{stamp}-{short}-{attempt}{FILE_SUFFIX}");
        attempt += 1;
    }
    let path = folder.join(&file);
    crate::model::write_private_atomic(&path, text)?;
    let header = vault::parse_header(text)?;
    Ok(BackupEntry::from_header(&file, header))
}

/// Reads one snapshot by file name. The name must be a plain file name, so a
/// caller cannot walk out of the backup folder.
pub fn read_backup(folder: &Path, file: &str) -> Result<String, String> {
    let name = Path::new(file);
    if name.components().count() != 1 || file.starts_with('.') || !file.ends_with(FILE_SUFFIX) {
        return Err("备份文件名无效。".into());
    }
    let path = folder.join(name);
    let meta = fs::metadata(&path).map_err(|error| format!("读不到这个备份：{error}"))?;
    if meta.len() as u64 > 16 * 1024 * 1024 {
        return Err("配置文件过大。".into());
    }
    fs::read_to_string(&path).map_err(|error| format!("读不到这个备份：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::{SealMeta, seal_new_password};

    const PASSWORD: &str = "Tunnel2026!x";
    const PAYLOAD: &[u8] = br#"{"tunnels":[]}"#;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tailtunnel-backup-{tag}-{}",
            vault::new_id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sealed(label: &str, revision: u64) -> String {
        let meta = SealMeta {
            label: label.into(),
            device: "test-mac".into(),
            saved_at: 1_700_000_000 + revision,
            revision,
        };
        seal_new_password(PASSWORD, None, &meta, PAYLOAD).unwrap().text
    }

    #[test]
    fn writes_and_lists_snapshots_newest_first() {
        let dir = temp_dir("list");
        write_backup(&dir, &vault::new_id(), &sealed("家里", 1)).unwrap();
        write_backup(&dir, &vault::new_id(), &sealed("公司", 2)).unwrap();
        let listed = list_backups(&dir).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].label, "公司");
        assert_eq!(listed[1].label, "家里");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn never_overwrites_an_existing_snapshot() {
        let dir = temp_dir("unique");
        let id = vault::new_id();
        let first = write_backup(&dir, &id, &sealed("家里", 1)).unwrap();
        let second = write_backup(&dir, &id, &sealed("家里", 1)).unwrap();
        assert_ne!(first.file, second.file);
        assert_eq!(list_backups(&dir).unwrap().len(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ignores_files_that_are_not_vaults() {
        let dir = temp_dir("skip");
        fs::write(dir.join("notes.txt"), "hello").unwrap();
        fs::write(dir.join("broken.vault.json"), "{").unwrap();
        write_backup(&dir, &vault::new_id(), &sealed("家里", 1)).unwrap();
        assert_eq!(list_backups(&dir).unwrap().len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_folder_lists_as_empty() {
        let dir = temp_dir("missing").join("nope");
        assert!(list_backups(&dir).unwrap().is_empty());
    }

    #[test]
    fn refuses_to_read_outside_the_folder() {
        let dir = temp_dir("escape");
        assert!(read_backup(&dir, "../secret.vault.json").is_err());
        assert!(read_backup(&dir, "/etc/passwd.vault.json").is_err());
        assert!(read_backup(&dir, ".hidden.vault.json").is_err());
        assert!(read_backup(&dir, "notes.txt").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = temp_dir("roundtrip");
        let entry = write_backup(&dir, &vault::new_id(), &sealed("家里", 3)).unwrap();
        let text = read_backup(&dir, &entry.file).unwrap();
        let opened = vault::open_with_password(PASSWORD, &text).unwrap();
        assert_eq!(&*opened.payload, PAYLOAD);
        assert_eq!(opened.header.revision, 3);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_remember_ids_but_no_password() {
        let dir = temp_dir("settings");
        let settings = VaultSettings {
            folder: "/tmp/backups".into(),
            vault_id: vault::new_id(),
            key_id: vault::new_id(),
            label: "家里".into(),
            revision: 4,
        };
        save_settings(&dir, &settings).unwrap();
        let text = fs::read_to_string(settings_file(&dir)).unwrap();
        assert!(!text.contains(PASSWORD));
        let loaded = load_settings(&dir);
        assert_eq!(loaded.vault_id, settings.vault_id);
        assert_eq!(loaded.revision, 4);
        fs::remove_dir_all(&dir).unwrap();
    }
}
