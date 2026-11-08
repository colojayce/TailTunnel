//! The iCloud Drive folder and what this Mac remembers about its sync. Each
//! Mac writes exactly one encrypted file there, named after its own device id,
//! and only reads the others' files. Two Macs therefore never write the same
//! file, which is what keeps iCloud from making conflict copies.
//!
//! Only ids, revisions and fingerprints are stored here; passwords stay in the
//! Keychain.

use crate::sync::Prints;
use crate::vault::{self, VaultHeader};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const FILE_SUFFIX: &str = ".vault.json";
const FILE_PREFIX: &str = "tailtunnel-";

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

/// Orders saves: a higher revision wins, then the later save, then the file
/// name, so every Mac picks the same "newest" copy.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stamp {
    pub revision: u64,
    pub saved_at: u64,
    pub file: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CloudSettings {
    /// Empty means the default iCloud Drive folder.
    pub folder: String,
    /// Stable per Mac; names this Mac's file in the folder.
    pub device_id: String,
    /// The synced config's identity and current password generation. Empty
    /// when sync is off.
    pub vault_id: String,
    pub key_id: String,
    pub label: String,
    /// The highest revision this Mac has seen or written.
    pub revision: u64,
    /// Fingerprints of the last copy from iCloud that was merged here: the
    /// common ancestor for the next merge.
    pub base: Prints,
    /// Fingerprints of what this Mac last uploaded.
    pub pushed: Prints,
    pub pushed_stamp: Option<Stamp>,
    pub pulled_stamp: Option<Stamp>,
    pub last_sync: u64,
}

impl CloudSettings {
    pub fn folder_path(&self) -> Result<PathBuf, String> {
        if !self.folder.is_empty() {
            return Ok(PathBuf::from(&self.folder));
        }
        default_folder().ok_or_else(|| "找不到 iCloud Drive 文件夹。".to_string())
    }

    pub fn is_enabled(&self) -> bool {
        !self.vault_id.is_empty() && !self.key_id.is_empty()
    }

    pub fn ensure_device_id(&mut self) {
        if self.device_id.is_empty() {
            self.device_id = vault::new_id();
        }
    }

    pub fn device_file(&self) -> String {
        format!("{FILE_PREFIX}{}{FILE_SUFFIX}", self.device_id)
    }

    /// Turns sync off on this Mac. The device id and folder stay, so turning
    /// it back on reuses the same file.
    pub fn leave(&mut self) {
        *self = Self {
            folder: std::mem::take(&mut self.folder),
            device_id: std::mem::take(&mut self.device_id),
            ..Self::default()
        };
    }
}

pub fn settings_file(app_dir: &Path) -> PathBuf {
    app_dir.join("vault-settings.json")
}

pub fn load_settings(app_dir: &Path) -> CloudSettings {
    let mut settings: CloudSettings = fs::read_to_string(settings_file(app_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    settings.ensure_device_id();
    settings
}

pub fn save_settings(app_dir: &Path, settings: &CloudSettings) -> Result<(), String> {
    let text = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("写入同步设置失败：{error}"))?;
    crate::model::write_private_atomic(&settings_file(app_dir), &text)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CloudFile {
    pub file: String,
    pub label: String,
    pub device: String,
    pub saved_at: u64,
    pub revision: u64,
    pub vault_id: String,
    pub key_id: String,
}

impl CloudFile {
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

    pub fn stamp(&self) -> Stamp {
        Stamp {
            revision: self.revision,
            saved_at: self.saved_at,
            file: self.file.clone(),
        }
    }
}

/// What a folder scan found. `evicted` lists files iCloud has offloaded to
/// save space: they show up as `.name.icloud` placeholders until downloaded.
#[derive(Debug, Default)]
pub struct Scan {
    pub files: Vec<CloudFile>,
    pub evicted: Vec<PathBuf>,
}

/// Lists readable vault files, newest first. Files that are not vaults are
/// left alone and skipped, so a stray file in the folder cannot break sync.
pub fn scan(folder: &Path) -> Result<Scan, String> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Scan::default()),
        Err(error) => return Err(format!("读 iCloud 文件夹失败：{error}")),
    };
    let mut scan = Scan::default();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(real) = name
            .strip_prefix('.')
            .and_then(|rest| rest.strip_suffix(".icloud"))
            .filter(|real| real.ends_with(FILE_SUFFIX))
        {
            scan.evicted.push(folder.join(real));
            continue;
        }
        if name.starts_with('.') || !name.ends_with(FILE_SUFFIX) {
            continue;
        }
        let Ok(text) = read_file(folder, name) else {
            continue;
        };
        let Ok(header) = vault::parse_header(&text) else {
            continue;
        };
        scan.files.push(CloudFile::from_header(name, header));
    }
    scan.files.sort_by_key(|file| std::cmp::Reverse(file.stamp()));
    Ok(scan)
}

/// Writes this Mac's file, replacing its previous version atomically.
pub fn write_file(folder: &Path, file: &str, text: &str) -> Result<(), String> {
    check_name(file)?;
    fs::create_dir_all(folder).map_err(|error| format!("创建 iCloud 文件夹失败：{error}"))?;
    crate::model::write_private_atomic(&folder.join(file), text)
}

/// Reads one file by name. The name must be a plain file name, so a caller
/// cannot walk out of the folder.
pub fn read_file(folder: &Path, file: &str) -> Result<String, String> {
    check_name(file)?;
    let path = folder.join(file);
    let meta = fs::metadata(&path).map_err(|error| format!("读不到同步文件：{error}"))?;
    if meta.len() > 16 * 1024 * 1024 {
        return Err("同步文件过大。".into());
    }
    fs::read_to_string(&path).map_err(|error| format!("读不到同步文件：{error}"))
}

fn check_name(file: &str) -> Result<(), String> {
    if Path::new(file).components().count() != 1 || file.starts_with('.') || !file.ends_with(FILE_SUFFIX)
    {
        return Err("同步文件名无效。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::{SealMeta, seal_new_password};

    const PASSWORD: &str = "Tunnel2026!x";
    const PAYLOAD: &[u8] = br#"{"tunnels":[]}"#;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tailtunnel-cloud-{tag}-{}", vault::new_id()));
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
    fn lists_files_newest_first() {
        let dir = temp_dir("list");
        write_file(&dir, "tailtunnel-a.vault.json", &sealed("家里", 1)).unwrap();
        write_file(&dir, "tailtunnel-b.vault.json", &sealed("公司", 2)).unwrap();
        let files = scan(&dir).unwrap().files;
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].label, "公司");
        assert_eq!(files[1].label, "家里");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_mac_replaces_only_its_own_file() {
        let dir = temp_dir("replace");
        let mut settings = CloudSettings::default();
        settings.ensure_device_id();
        let own = settings.device_file();
        write_file(&dir, &own, &sealed("家里", 1)).unwrap();
        write_file(&dir, &own, &sealed("家里", 2)).unwrap();
        let files = scan(&dir).unwrap().files;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].revision, 2);
        assert_eq!(files[0].file, own);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn skips_files_that_are_not_vaults_and_notes_evicted_ones() {
        let dir = temp_dir("skip");
        fs::write(dir.join("notes.txt"), "hello").unwrap();
        fs::write(dir.join("broken.vault.json"), "{").unwrap();
        fs::write(dir.join(".tailtunnel-x.vault.json.icloud"), "").unwrap();
        write_file(&dir, "tailtunnel-a.vault.json", &sealed("家里", 1)).unwrap();
        let found = scan(&dir).unwrap();
        assert_eq!(found.files.len(), 1);
        assert_eq!(found.evicted, [dir.join("tailtunnel-x.vault.json")]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_folder_scans_as_empty() {
        let dir = temp_dir("missing").join("nope");
        assert!(scan(&dir).unwrap().files.is_empty());
    }

    #[test]
    fn refuses_names_outside_the_folder() {
        let dir = temp_dir("escape");
        assert!(read_file(&dir, "../secret.vault.json").is_err());
        assert!(read_file(&dir, "/etc/passwd.vault.json").is_err());
        assert!(read_file(&dir, ".hidden.vault.json").is_err());
        assert!(read_file(&dir, "notes.txt").is_err());
        assert!(write_file(&dir, "../x.vault.json", "{}").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = temp_dir("roundtrip");
        write_file(&dir, "tailtunnel-a.vault.json", &sealed("家里", 3)).unwrap();
        let text = read_file(&dir, "tailtunnel-a.vault.json").unwrap();
        let opened = vault::open_with_password(PASSWORD, &text).unwrap();
        assert_eq!(&*opened.payload, PAYLOAD);
        assert_eq!(opened.header.revision, 3);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stamps_order_by_revision_then_time_then_name() {
        let stamp = |revision, saved_at, file: &str| Stamp {
            revision,
            saved_at,
            file: file.into(),
        };
        assert!(stamp(2, 1, "a") > stamp(1, 9, "z"));
        assert!(stamp(2, 5, "a") > stamp(2, 4, "z"));
        assert!(stamp(2, 5, "b") > stamp(2, 5, "a"));
    }

    #[test]
    fn settings_from_the_backup_era_still_load() {
        let dir = temp_dir("legacy");
        fs::write(
            settings_file(&dir),
            r#"{"folder":"","vaultId":"v","keyId":"k","label":"家里","revision":4}"#,
        )
        .unwrap();
        let loaded = load_settings(&dir);
        assert!(loaded.is_enabled());
        assert_eq!(loaded.revision, 4);
        assert!(!loaded.device_id.is_empty());
        assert!(loaded.base.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn leaving_keeps_the_device_and_folder_but_no_password_or_ids() {
        let dir = temp_dir("settings");
        let mut settings = CloudSettings {
            folder: "/tmp/sync".into(),
            vault_id: vault::new_id(),
            key_id: vault::new_id(),
            label: "家里".into(),
            revision: 4,
            ..CloudSettings::default()
        };
        settings.ensure_device_id();
        let device = settings.device_id.clone();
        save_settings(&dir, &settings).unwrap();
        let text = fs::read_to_string(settings_file(&dir)).unwrap();
        assert!(!text.contains(PASSWORD));
        settings.leave();
        assert!(!settings.is_enabled());
        assert_eq!(settings.device_id, device);
        assert_eq!(settings.folder, "/tmp/sync");
        assert_eq!(settings.revision, 0);
        fs::remove_dir_all(&dir).unwrap();
    }
}
