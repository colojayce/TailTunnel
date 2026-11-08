//! iCloud sync: keeps this Mac's tunnels and the other Macs' in step through
//! encrypted files in iCloud Drive.
//!
//! The local `tunnels.json` stays the working copy. A sync round reads the
//! headers in the folder, merges the newest copy another Mac uploaded if this
//! Mac has not merged it yet, and uploads this Mac's config when it changed.
//! Rounds run after a local edit (debounced), when the window comes to the
//! front, every so often in the background, and when the user asks.

use crate::cloud::{self, CloudFile, CloudSettings, Stamp};
use crate::model::{self, Tunnel};
use crate::secrets::{self, KeychainStore, SecretStore};
use crate::sync::{self, SyncNote};
use crate::vault::{self, VaultKey};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use zeroize::Zeroizing;

/// Returned when a password is needed and the Keychain has none that works.
pub const NEED_PASSWORD: &str = "需要同步密码。";
const NO_ICLOUD: &str = "找不到 iCloud Drive。请在系统设置里登录 iCloud 并打开 iCloud 云盘，或换一个同步文件夹。";
const DEFAULT_LABEL: &str = "我的隧道";
const POLL: Duration = Duration::from_secs(45);
const SETTLE: Duration = Duration::from_millis(1500);

pub struct SyncState {
    cache: Mutex<SyncCache>,
    kick: Mutex<Option<Sender<()>>>,
}

struct SyncCache {
    app_dir: PathBuf,
    store: Box<dyn SecretStore>,
    device_name: String,
    settings: CloudSettings,
    key: Option<VaultKey>,
    phase: Phase,
    message: String,
    devices: Vec<DeviceView>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Ready,
    Locked,
    Failed,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    name: String,
    saved_at: u64,
    this_mac: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    folder: String,
    available: bool,
    enabled: bool,
    label: String,
    /// "off" | "ready" | "locked" | "error"
    phase: &'static str,
    message: String,
    last_sync: u64,
    devices: Vec<DeviceView>,
    min_password_len: usize,
    max_password_len: usize,
    password_symbols: &'static str,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    /// The Mac whose copy was merged, if one was.
    from: String,
    added: usize,
    updated: usize,
    removed: usize,
    notes: Vec<SyncNote>,
    uploaded: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncEvent {
    status: SyncStatus,
    report: Option<SyncReport>,
}

/// A config found in the folder that this Mac could join.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudVault {
    vault_id: String,
    label: String,
    devices: Vec<String>,
    saved_at: u64,
}

/// Gets this Mac's tunnels and the busy ones; returns the merged list, or
/// `None` when nothing changed.
type MergeFn<'a> = dyn FnMut(&[Tunnel], &HashSet<String>) -> Option<Vec<Tunnel>> + 'a;

/// This Mac's tunnels, as the sync round sees them. The app implements it on
/// top of the tunnel manager; tests use a plain list.
trait Workspace {
    fn tunnels(&self) -> Vec<Tunnel>;
    /// Calls `merge` with the current tunnels and the busy ones, under one
    /// lock, and stores the tunnels it returns, if any.
    fn update(
        &self,
        merge: &mut MergeFn,
    ) -> Result<(), String>;
}

struct AppWorkspace<'a>(&'a AppHandle);

impl Workspace for AppWorkspace<'_> {
    fn tunnels(&self) -> Vec<Tunnel> {
        self.0
            .state::<crate::AppState>()
            .0
            .lock()
            .expect("隧道状态锁已损坏")
            .tunnels()
    }

    fn update(
        &self,
        merge: &mut MergeFn,
    ) -> Result<(), String> {
        let state = self.0.state::<crate::AppState>();
        let snapshot = {
            let mut manager = state.0.lock().expect("隧道状态锁已损坏");
            let local = manager.tunnels();
            let busy = manager.busy_ids();
            match merge(&local, &busy) {
                Some(tunnels) => {
                    manager.replace_tunnels(tunnels)?;
                    Some(manager.snapshot())
                }
                None => None,
            }
        };
        if let Some(snapshot) = snapshot {
            crate::publish(self.0, &snapshot);
        }
        Ok(())
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|span| span.as_secs())
        .unwrap_or_default()
}

fn device_name() -> String {
    let from_scutil = Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|name| !name.is_empty());
    from_scutil
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|name| !name.is_empty()))
        .unwrap_or_else(|| "这台 Mac".into())
}

/// iCloud Drive itself must exist; the TailTunnel folder inside it is created
/// on the first upload.
fn reachable(folder: &Path) -> bool {
    folder.exists() || folder.parent().is_some_and(Path::exists)
}

/// Asks iCloud to bring back files it offloaded; the next round reads them.
fn request_downloads(paths: &[PathBuf]) {
    for path in paths {
        let spawned = Command::new("brctl")
            .arg("download")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = spawned {
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

impl SyncCache {
    fn status(&self) -> SyncStatus {
        let folder = self.settings.folder_path().ok();
        let enabled = self.settings.is_enabled();
        SyncStatus {
            available: folder.as_deref().is_some_and(reachable),
            folder: folder
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            enabled,
            label: self.settings.label.clone(),
            phase: match (enabled, self.phase) {
                (false, _) => "off",
                (true, Phase::Ready) => "ready",
                (true, Phase::Locked) => "locked",
                (true, Phase::Failed) => "error",
            },
            message: self.message.clone(),
            last_sync: self.settings.last_sync,
            devices: self.devices.clone(),
            min_password_len: vault::MIN_PASSWORD_LEN,
            max_password_len: vault::MAX_PASSWORD_LEN,
            password_symbols: vault::PASSWORD_SYMBOLS,
        }
    }

    fn save(&self) -> Result<(), String> {
        cloud::save_settings(&self.app_dir, &self.settings)
    }

    fn settle(&mut self, result: &Result<SyncReport, String>) {
        match result {
            Ok(_) => {
                self.phase = Phase::Ready;
                self.message.clear();
                self.settings.last_sync = now();
            }
            Err(message) if message == NEED_PASSWORD => {
                self.phase = Phase::Locked;
                self.message.clear();
            }
            Err(message) => {
                self.phase = Phase::Failed;
                self.message = message.clone();
            }
        }
    }

    fn device_views(&mut self, files: &[&CloudFile]) {
        let own = self.settings.device_file();
        // 旧版本的备份文件一台 Mac 可能有好几份，按设备名只留最新的。
        let mut by_name: BTreeMap<String, DeviceView> = BTreeMap::new();
        for file in files {
            let this_mac = file.file == own;
            let name = if this_mac {
                self.device_name.clone()
            } else if file.device.is_empty() {
                "另一台 Mac".to_string()
            } else {
                file.device.clone()
            };
            let key = if this_mac { String::new() } else { name.clone() };
            let entry = by_name.entry(key).or_insert(DeviceView {
                name,
                saved_at: 0,
                this_mac,
            });
            entry.saved_at = entry.saved_at.max(file.saved_at);
        }
        let mut devices: Vec<DeviceView> = by_name.into_values().collect();
        devices.sort_by(|a, b| b.this_mac.cmp(&a.this_mac).then(b.saved_at.cmp(&a.saved_at)));
        self.devices = devices;
    }

    /// Derives the key for `key_id` from the password given or remembered,
    /// using the newest file of that password generation for its salt.
    fn unlock(
        &mut self,
        folder: &Path,
        files: &[&CloudFile],
        key_id: &str,
        given: Option<&str>,
    ) -> Result<(), String> {
        let vault_id = self.settings.vault_id.clone();
        let (password, remembered) = match given.filter(|value| !value.is_empty()) {
            Some(value) => (Zeroizing::new(value.to_string()), false),
            None => match secrets::recall(self.store.as_ref(), &vault_id, key_id).ok().flatten() {
                Some(value) => (Zeroizing::new(value), true),
                None => return Err(NEED_PASSWORD.into()),
            },
        };
        let source = files
            .iter()
            .filter(|file| file.key_id == key_id)
            .max_by_key(|file| file.stamp());
        let key = match source {
            Some(file) => {
                let text = cloud::read_file(folder, &file.file)?;
                match vault::open_with_password(&password, &text) {
                    Ok(opened) => opened.key,
                    Err(error) if remembered && error == vault::WRONG_PASSWORD => {
                        // 钥匙串里的密码已经解不开了，别每一轮都拿它重试。
                        let _ = secrets::forget(self.store.as_ref(), &vault_id, key_id);
                        return Err(NEED_PASSWORD.into());
                    }
                    Err(error) => return Err(error),
                }
            }
            // 文件夹里已经没有这一代密码加密的文件（比如被手动清空），用同一个密码开新的一代。
            None => {
                let meta = vault::SealMeta {
                    label: self.settings.label.clone(),
                    device: self.device_name.clone(),
                    saved_at: now(),
                    revision: self.settings.revision,
                };
                vault::seal_new_password(&password, Some(&vault_id), &meta, b"[]")?.key
            }
        };
        // 钥匙串不可用时不挡住同步，只是下次还要再输一次密码。
        let _ = secrets::remember(self.store.as_ref(), &vault_id, key.key_id(), &password);
        self.settings.key_id = key.key_id().to_string();
        self.key = Some(key);
        Ok(())
    }

    /// One sync round. The app's tunnel lock is only held while merging.
    fn round(&mut self, ws: &dyn Workspace, password: Option<&str>) -> Result<SyncReport, String> {
        let folder = self.settings.folder_path()?;
        if !reachable(&folder) {
            return Err(NO_ICLOUD.into());
        }
        let scan = cloud::scan(&folder)?;
        request_downloads(&scan.evicted);
        let own = self.settings.device_file();
        let vault_id = self.settings.vault_id.clone();
        let mine: Vec<&CloudFile> = scan
            .files
            .iter()
            .filter(|file| file.vault_id == vault_id)
            .collect();
        self.device_views(&mine);

        let newest = mine
            .iter()
            .filter(|file| file.file != own)
            .max_by_key(|file| file.stamp())
            .copied();
        // 比本机已知版本还旧的副本来自落后的那台 Mac，等它先合并本机的再说。
        // 版本号相同说明两台 Mac 同时上传过，也要合并；但从旧版“加密备份”升级来的
        // 设置没有这些记录，同号的文件就是本机自己当时备份或恢复过的那份。
        let ties_count =
            self.settings.pushed_stamp.is_some() || self.settings.pulled_stamp.is_some();
        let known = self.settings.revision;
        let incoming = newest.filter(|file| {
            (file.revision > known || (ties_count && file.revision == known))
                && self.settings.pulled_stamp.as_ref() != Some(&file.stamp())
        });
        // 重新开启同步时本机自己的旧文件可能版本更高，下一次上传要排在它后面。
        let top = mine.iter().map(|file| file.revision).max().unwrap_or(0);
        self.settings.revision = self.settings.revision.max(top);
        let wanted_key = incoming
            .map(|file| file.key_id.clone())
            .unwrap_or_else(|| self.settings.key_id.clone());
        if self.key.as_ref().map(VaultKey::key_id) != Some(wanted_key.as_str()) || password.is_some() {
            self.unlock(&folder, &mine, &wanted_key, password)?;
        }

        let mut report = SyncReport::default();
        if let Some(file) = incoming {
            self.pull(ws, &folder, file, &mut report)?;
        }
        let own_current = mine.iter().any(|file| {
            file.file == own && self.key.as_ref().is_some_and(|key| key.key_id() == file.key_id)
        });
        self.push(ws, &folder, &own, own_current, &mut report)?;
        Ok(report)
    }

    fn pull(
        &mut self,
        ws: &dyn Workspace,
        folder: &Path,
        file: &CloudFile,
        report: &mut SyncReport,
    ) -> Result<(), String> {
        let text = cloud::read_file(folder, &file.file)?;
        let key = self.key.take().ok_or_else(|| NEED_PASSWORD.to_string())?;
        let opened = vault::open_with_key(key, &text)?;
        self.key = Some(opened.key);
        let remote = model::parse_store(&opened.payload)?;
        let remote_is_newer = self
            .settings
            .pushed_stamp
            .as_ref()
            .is_none_or(|own| file.stamp() > *own);

        let mut merged_prints = sync::Prints::new();
        let settings = &self.settings;
        ws.update(&mut |local, busy| {
            let outcome = sync::merge(&sync::MergeInput {
                base: &settings.base,
                pushed: &settings.pushed,
                local,
                remote: &remote,
                busy,
                remote_is_newer,
            });
            report.added = outcome.added;
            report.updated = outcome.updated;
            report.removed = outcome.removed;
            report.notes = outcome.notes;
            merged_prints = sync::prints(&outcome.tunnels);
            let same_order = outcome
                .tunnels
                .iter()
                .map(|tunnel| &tunnel.id)
                .eq(local.iter().map(|tunnel| &tunnel.id));
            if same_order && merged_prints == sync::prints(local) {
                None
            } else {
                Some(outcome.tunnels)
            }
        })?;

        let remote_prints = sync::prints(&remote);
        // iCloud 里最新的那份已经和本机一致，不用再传一份一样的。
        if merged_prints == remote_prints {
            self.settings.pushed = remote_prints.clone();
        }
        self.settings.base = remote_prints;
        self.settings.revision = self.settings.revision.max(file.revision);
        self.settings.pulled_stamp = Some(file.stamp());
        if !file.label.is_empty() {
            self.settings.label = file.label.clone();
        }
        report.from = if file.device.is_empty() {
            "另一台 Mac".into()
        } else {
            file.device.clone()
        };
        Ok(())
    }

    fn push(
        &mut self,
        ws: &dyn Workspace,
        folder: &Path,
        own: &str,
        own_current: bool,
        report: &mut SyncReport,
    ) -> Result<(), String> {
        let tunnels = ws.tunnels();
        let current = sync::prints(&tunnels);
        if own_current && current == self.settings.pushed {
            return Ok(());
        }
        let key = self.key.as_ref().ok_or_else(|| NEED_PASSWORD.to_string())?;
        let revision = self.settings.revision + 1;
        let saved_at = now();
        let label = if self.settings.label.is_empty() {
            DEFAULT_LABEL.to_string()
        } else {
            self.settings.label.clone()
        };
        let payload = Zeroizing::new(model::store_text(&tunnels)?);
        let meta = vault::SealMeta {
            label: label.clone(),
            device: self.device_name.clone(),
            saved_at,
            revision,
        };
        let text = vault::seal_with_key(key, &meta, payload.as_bytes())?;
        cloud::write_file(folder, own, &text)?;
        self.settings.label = label;
        self.settings.revision = revision;
        self.settings.pushed = current;
        self.settings.pushed_stamp = Some(Stamp {
            revision,
            saved_at,
            file: own.to_string(),
        });
        report.uploaded = true;
        if let Some(device) = self.devices.iter_mut().find(|device| device.this_mac) {
            device.saved_at = saved_at;
        } else {
            self.devices.insert(
                0,
                DeviceView {
                    name: self.device_name.clone(),
                    saved_at,
                    this_mac: true,
                },
            );
        }
        Ok(())
    }
}

impl SyncCache {
    /// Starts a new synced config from this Mac's tunnels.
    fn create(&mut self, ws: &dyn Workspace, password: &str) -> Result<(), String> {
        let folder = self.settings.folder_path()?;
        if !reachable(&folder) {
            return Err(NO_ICLOUD.into());
        }
        let tunnels = ws.tunnels();
        let payload = Zeroizing::new(model::store_text(&tunnels)?);
        let saved_at = now();
        let meta = vault::SealMeta {
            label: DEFAULT_LABEL.into(),
            device: self.device_name.clone(),
            saved_at,
            revision: 1,
        };
        let sealed = vault::seal_new_password(password, None, &meta, payload.as_bytes())?;
        self.settings.leave();
        let own = self.settings.device_file();
        cloud::write_file(&folder, &own, &sealed.text)?;
        let prints = sync::prints(&tunnels);
        let settings = &mut self.settings;
        settings.vault_id = sealed.key.vault_id().to_string();
        settings.key_id = sealed.key.key_id().to_string();
        settings.label = DEFAULT_LABEL.into();
        settings.revision = 1;
        settings.base = prints.clone();
        settings.pushed = prints;
        settings.pushed_stamp = Some(Stamp {
            revision: 1,
            saved_at,
            file: own,
        });
        settings.last_sync = saved_at;
        // 钥匙串不可用时不挡住开启，只是下次还要再输一次密码。
        let _ = secrets::remember(
            self.store.as_ref(),
            sealed.key.vault_id(),
            sealed.key.key_id(),
            password,
        );
        self.key = Some(sealed.key);
        self.phase = Phase::Ready;
        self.message.clear();
        self.devices = vec![DeviceView {
            name: self.device_name.clone(),
            saved_at,
            this_mac: true,
        }];
        Ok(())
    }

    /// Joins a config another Mac started. On any failure, sync stays as it
    /// was before.
    fn join(
        &mut self,
        ws: &dyn Workspace,
        vault_id: &str,
        password: &str,
    ) -> Result<SyncReport, String> {
        let folder = self.settings.folder_path()?;
        if !reachable(&folder) {
            return Err(NO_ICLOUD.into());
        }
        let scan = cloud::scan(&folder)?;
        let newest = scan
            .files
            .iter()
            .find(|file| file.vault_id == vault_id)
            .ok_or_else(|| "iCloud 里找不到这份同步配置了。".to_string())?;
        let before = self.settings.clone();
        self.settings.leave();
        self.settings.vault_id = vault_id.to_string();
        self.settings.key_id = newest.key_id.clone();
        self.settings.label = newest.label.clone();
        self.key = None;
        let result = self.round(ws, Some(password));
        match result {
            Ok(report) => {
                self.settle(&Ok(report.clone()));
                Ok(report)
            }
            Err(error) => {
                self.settings = before;
                self.key = None;
                Err(if error == NEED_PASSWORD {
                    vault::WRONG_PASSWORD.into()
                } else {
                    error
                })
            }
        }
    }
}

fn sync_lock<'a>(state: &'a SyncState) -> MutexGuard<'a, SyncCache> {
    state.cache.lock().expect("同步状态锁已损坏")
}

fn emit(app: &AppHandle, status: SyncStatus, report: Option<SyncReport>) {
    let _ = app.emit("sync-changed", SyncEvent { status, report });
}

/// Runs a round if sync is on and tells the window how it went.
fn run_round(app: &AppHandle, password: Option<&str>) -> Result<SyncStatus, String> {
    let state = app.state::<SyncState>();
    let mut cache = sync_lock(&state);
    if !cache.settings.is_enabled() {
        return Ok(cache.status());
    }
    let result = cache.round(&AppWorkspace(app), password);
    cache.settle(&result);
    let saved = cache.save();
    let status = cache.status();
    emit(app, status.clone(), result.as_ref().ok().cloned());
    result?;
    saved?;
    Ok(status)
}

/// Asks the background worker for a round soon. Cheap; call it after any
/// change to the local config.
pub fn kick(app: &AppHandle) {
    let Some(state) = app.try_state::<SyncState>() else {
        return;
    };
    if let Some(sender) = state.kick.lock().expect("同步状态锁已损坏").as_ref() {
        let _ = sender.send(());
    }
}

pub fn manage(app: &tauri::App, app_dir: PathBuf) {
    let settings = cloud::load_settings(&app_dir);
    let _ = cloud::save_settings(&app_dir, &settings);
    app.manage(SyncState {
        cache: Mutex::new(SyncCache {
            app_dir,
            store: Box::new(KeychainStore),
            device_name: device_name(),
            settings,
            key: None,
            phase: Phase::Ready,
            message: String::new(),
            devices: Vec::new(),
        }),
        kick: Mutex::new(None),
    });
    let (sender, receiver) = mpsc::channel::<()>();
    *app.state::<SyncState>().kick.lock().expect("同步状态锁已损坏") = Some(sender);
    let handle = app.handle().clone();
    thread::spawn(move || {
        let _ = run_round(&handle, None);
        loop {
            match receiver.recv_timeout(POLL) {
                // 连续的改动合并成一轮。
                Ok(()) => while receiver.recv_timeout(SETTLE).is_ok() {},
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let _ = run_round(&handle, None);
        }
    });
}

#[tauri::command(async)]
pub fn sync_status(state: State<SyncState>) -> SyncStatus {
    sync_lock(&state).status()
}

/// Configs already in the folder, newest first, so a new Mac can join one.
#[tauri::command(async)]
pub fn sync_inspect(state: State<SyncState>) -> Result<Vec<CloudVault>, String> {
    let folder = sync_lock(&state).settings.folder_path()?;
    let scan = cloud::scan(&folder)?;
    request_downloads(&scan.evicted);
    let mut vaults: Vec<CloudVault> = Vec::new();
    for file in &scan.files {
        let device = if file.device.is_empty() {
            "另一台 Mac".to_string()
        } else {
            file.device.clone()
        };
        match vaults.iter_mut().find(|vault| vault.vault_id == file.vault_id) {
            Some(vault) => {
                if !vault.devices.contains(&device) {
                    vault.devices.push(device);
                }
            }
            // 文件按新到旧排好了，第一份就是这份配置最新的名字和时间。
            None => vaults.push(CloudVault {
                vault_id: file.vault_id.clone(),
                label: file.label.clone(),
                devices: vec![device],
                saved_at: file.saved_at,
            }),
        }
    }
    Ok(vaults)
}

/// Turns sync on. With `join`, this Mac joins that config with its password
/// and merges; without, it starts a new config under a new password.
#[tauri::command(async)]
pub fn sync_enable(
    app: AppHandle,
    state: State<SyncState>,
    password: String,
    join: Option<String>,
) -> Result<SyncStatus, String> {
    let mut cache = sync_lock(&state);
    let ws = AppWorkspace(&app);
    let report = match join {
        Some(vault_id) => Some(cache.join(&ws, &vault_id, &password)?),
        None => {
            cache.create(&ws, &password)?;
            None
        }
    };
    cache.save()?;
    let status = cache.status();
    emit(&app, status.clone(), report);
    Ok(status)
}

/// Supplies the password when this Mac's Keychain has none that works, for
/// example after the password was changed on another Mac.
#[tauri::command(async)]
pub fn sync_unlock(app: AppHandle, password: String) -> Result<SyncStatus, String> {
    run_round(&app, Some(&password)).map_err(|error| {
        if error == NEED_PASSWORD {
            vault::WRONG_PASSWORD.into()
        } else {
            error
        }
    })
}

#[tauri::command(async)]
pub fn sync_now(app: AppHandle) -> Result<SyncStatus, String> {
    run_round(&app, None)
}

/// Turns sync off on this Mac only. Local tunnels stay as they are; the files
/// in iCloud and the other Macs are not touched.
#[tauri::command(async)]
pub fn sync_disable(app: AppHandle, state: State<SyncState>) -> Result<SyncStatus, String> {
    let mut cache = sync_lock(&state);
    if cache.settings.is_enabled() {
        let _ = secrets::forget(
            &KeychainStore,
            &cache.settings.vault_id,
            &cache.settings.key_id,
        );
    }
    cache.settings.leave();
    cache.key = None;
    cache.phase = Phase::Ready;
    cache.message.clear();
    cache.devices.clear();
    cache.save()?;
    let status = cache.status();
    emit(&app, status.clone(), None);
    Ok(status)
}

#[tauri::command(async)]
pub fn sync_set_folder(
    app: AppHandle,
    state: State<SyncState>,
    folder: String,
) -> Result<SyncStatus, String> {
    let status = {
        let mut cache = sync_lock(&state);
        cache.settings.folder = folder.trim().to_string();
        cache.devices.clear();
        cache.save()?;
        cache.status()
    };
    kick(&app);
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthMethod, Forward};
    use crate::sync::NoteKind;
    use std::cell::RefCell;
    use std::collections::HashMap;

    const PASSWORD: &str = "Tunnel2026!x";

    #[derive(Default)]
    struct MemoryStore(Mutex<HashMap<String, String>>);

    impl SecretStore for MemoryStore {
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(account).cloned())
        }

        fn set(&self, account: &str, secret: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(account.into(), secret.into());
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(account);
            Ok(())
        }
    }

    #[derive(Default)]
    struct Local {
        tunnels: RefCell<Vec<Tunnel>>,
        busy: RefCell<HashSet<String>>,
    }

    impl Workspace for Local {
        fn tunnels(&self) -> Vec<Tunnel> {
            self.tunnels.borrow().clone()
        }

        fn update(
            &self,
            merge: &mut MergeFn,
        ) -> Result<(), String> {
            let local = self.tunnels();
            let busy = self.busy.borrow().clone();
            if let Some(next) = merge(&local, &busy) {
                *self.tunnels.borrow_mut() = next;
            }
            Ok(())
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tailtunnel-icloud-{tag}-{}", vault::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tunnel(id: &str, local_port: u16, password: &str) -> Tunnel {
        Tunnel {
            id: id.into(),
            note: String::new(),
            host: "10.0.0.5".into(),
            port: 22,
            user: "root".into(),
            password: password.into(),
            auth: AuthMethod::Password,
            key_file: String::new(),
            jump: None,
            forwards: vec![Forward {
                local_port,
                remote_host: "127.0.0.1".into(),
                remote_port: 5432,
            }],
            auto_reconnect: false,
        }
    }

    struct Mac {
        cache: SyncCache,
        local: Local,
    }

    impl Mac {
        fn new(name: &str, folder: &Path, tunnels: Vec<Tunnel>) -> Self {
            let mut settings = CloudSettings {
                folder: folder.display().to_string(),
                ..CloudSettings::default()
            };
            settings.ensure_device_id();
            Self {
                cache: SyncCache {
                    app_dir: temp_dir(name),
                    store: Box::new(MemoryStore::default()),
                    device_name: name.into(),
                    settings,
                    key: None,
                    phase: Phase::Ready,
                    message: String::new(),
                    devices: Vec::new(),
                },
                local: Local {
                    tunnels: RefCell::new(tunnels),
                    ..Local::default()
                },
            }
        }

        fn create(&mut self) {
            self.cache.create(&self.local, PASSWORD).unwrap();
        }

        fn join(&mut self, other: &Mac) -> SyncReport {
            let vault_id = other.cache.settings.vault_id.clone();
            self.cache.join(&self.local, &vault_id, PASSWORD).unwrap()
        }

        fn round(&mut self) -> SyncReport {
            self.cache.round(&self.local, None).unwrap()
        }

        fn ids(&self) -> Vec<String> {
            self.local.tunnels().into_iter().map(|t| t.id).collect()
        }

        fn get(&self, id: &str) -> Tunnel {
            self.local.tunnels().into_iter().find(|t| t.id == id).unwrap()
        }

        fn edit(&self, id: &str, change: impl FnOnce(&mut Tunnel)) {
            let mut tunnels = self.local.tunnels.borrow_mut();
            change(tunnels.iter_mut().find(|t| t.id == id).unwrap());
        }

        fn own_file(&self) -> PathBuf {
            PathBuf::from(&self.cache.settings.folder).join(self.cache.settings.device_file())
        }
    }

    /// A with x and y, B with z, joined and settled.
    fn pair(tag: &str) -> (Mac, Mac) {
        let folder = temp_dir(tag);
        let mut a = Mac::new("A", &folder, vec![tunnel("x", 5000, "px"), tunnel("y", 5001, "py")]);
        a.create();
        let mut b = Mac::new("B", &folder, vec![tunnel("z", 6000, "pz")]);
        let joined = b.join(&a);
        assert_eq!(joined.added, 2);
        assert!(joined.uploaded);
        assert_eq!(a.round().added, 1);
        (a, b)
    }

    fn settled(a: &mut Mac, b: &mut Mac) {
        for _ in 0..2 {
            assert!(!a.round().uploaded, "A kept uploading");
            assert!(!b.round().uploaded, "B kept uploading");
        }
    }

    #[test]
    fn a_new_mac_joins_and_both_end_up_with_everything() {
        let (mut a, mut b) = pair("join");
        assert_eq!(a.ids(), ["x", "y", "z"]);
        assert_eq!(b.ids(), ["z", "x", "y"]);
        assert_eq!(b.get("x").password, "px");
        settled(&mut a, &mut b);
        let names: Vec<_> = b.cache.devices.iter().map(|d| (d.name.as_str(), d.this_mac)).collect();
        assert_eq!(names, [("B", true), ("A", false)]);
    }

    #[test]
    fn edits_and_deletions_travel_to_the_other_mac() {
        let (mut a, mut b) = pair("travel");
        a.edit("x", |t| t.forwards[0].local_port = 7000);
        assert!(a.round().uploaded);
        let report = b.round();
        assert_eq!(report.updated, 1);
        assert_eq!(report.from, "A");
        assert_eq!(b.get("x").forwards[0].local_port, 7000);

        b.local.tunnels.borrow_mut().retain(|t| t.id != "y");
        assert!(b.round().uploaded);
        assert_eq!(a.round().removed, 1);
        assert_eq!(a.ids(), ["x", "z"]);
        settled(&mut a, &mut b);
    }

    #[test]
    fn a_tunnel_running_here_survives_a_deletion_elsewhere() {
        let (mut a, mut b) = pair("busy");
        b.local.busy.borrow_mut().insert("y".into());
        a.local.tunnels.borrow_mut().retain(|t| t.id != "y");
        a.round();
        let report = b.round();
        assert_eq!(b.ids(), ["z", "x", "y"]);
        assert_eq!(report.notes[0].kind, NoteKind::KeptDeleted);
    }

    #[test]
    fn concurrent_forward_edits_settle_on_the_later_save() {
        let (mut a, mut b) = pair("forward");
        a.edit("x", |t| t.forwards[0].local_port = 7000);
        b.edit("x", |t| t.forwards[0].local_port = 8000);
        a.round();
        b.round();
        let report = a.round();
        assert_eq!(report.notes[0].kind, NoteKind::SettingsReplaced);
        assert_eq!(a.get("x").forwards[0].local_port, 8000);
        assert_eq!(b.get("x").forwards[0].local_port, 8000);
        settled(&mut a, &mut b);
    }

    #[test]
    fn concurrent_login_edits_stay_on_their_own_macs() {
        let (mut a, mut b) = pair("login");
        a.edit("x", |t| t.password = "a2".into());
        b.edit("x", |t| t.password = "b2".into());
        a.round();
        assert_eq!(b.round().notes[0].kind, NoteKind::LoginKept);
        assert_eq!(a.round().notes[0].kind, NoteKind::LoginKept);
        settled(&mut a, &mut b);
        assert_eq!(a.get("x").password, "a2");
        assert_eq!(b.get("x").password, "b2");
    }

    #[test]
    fn uploads_with_the_same_revision_are_both_merged() {
        let (mut a, mut b) = pair("tie");
        a.edit("x", |t| t.forwards[0].local_port = 7000);
        a.round();
        // iCloud 还没把 A 的文件送到 B 那边，B 也在同一个版本号上传了。
        let parked = a.own_file().with_extension("parked");
        std::fs::rename(a.own_file(), &parked).unwrap();
        b.edit("y", |t| t.forwards[0].local_port = 7001);
        b.round();
        std::fs::rename(&parked, a.own_file()).unwrap();

        a.round();
        b.round();
        for mac in [&a, &b] {
            assert_eq!(mac.get("x").forwards[0].local_port, 7000);
            assert_eq!(mac.get("y").forwards[0].local_port, 7001);
        }
        settled(&mut a, &mut b);
    }

    #[test]
    fn a_wrong_password_leaves_sync_off() {
        let folder = temp_dir("wrong");
        let mut a = Mac::new("A", &folder, vec![tunnel("x", 5000, "px")]);
        a.create();
        let mut b = Mac::new("B", &folder, Vec::new());
        let vault_id = a.cache.settings.vault_id.clone();
        let error = b.cache.join(&b.local, &vault_id, "Wrong2026!x").err().unwrap();
        assert_eq!(error, vault::WRONG_PASSWORD);
        assert!(!b.cache.settings.is_enabled());
        assert!(b.ids().is_empty());
    }

    #[test]
    fn a_mac_without_the_password_in_its_keychain_is_locked_not_broken() {
        let (_a, mut b) = pair("locked");
        b.cache.key = None;
        b.cache.store = Box::new(MemoryStore::default());
        let result = b.cache.round(&b.local, None);
        assert_eq!(result.err().as_deref(), Some(NEED_PASSWORD));
        assert!(b.cache.round(&b.local, Some(PASSWORD)).is_ok());
    }

    #[test]
    fn upgrading_from_backups_does_not_roll_back_newer_local_edits() {
        let folder = temp_dir("legacy");
        let mut a = Mac::new("A", &folder, vec![tunnel("x", 5000, "px")]);
        let meta = vault::SealMeta {
            label: "家里".into(),
            device: "A".into(),
            saved_at: 1_700_000_000,
            revision: 3,
        };
        let payload = model::store_text(&a.local.tunnels()).unwrap();
        let sealed = vault::seal_new_password(PASSWORD, None, &meta, payload.as_bytes()).unwrap();
        cloud::write_file(&folder, "tailtunnel-1700000000-abcd1234.vault.json", &sealed.text).unwrap();
        let settings = &mut a.cache.settings;
        settings.vault_id = sealed.key.vault_id().into();
        settings.key_id = sealed.key.key_id().into();
        settings.revision = 3;
        secrets::remember(a.cache.store.as_ref(), sealed.key.vault_id(), sealed.key.key_id(), PASSWORD)
            .unwrap();
        // 最后一次备份之后，本机又改过这条隧道。
        a.edit("x", |t| t.forwards[0].local_port = 7000);

        let report = a.round();
        assert!(report.uploaded);
        assert_eq!(a.get("x").forwards[0].local_port, 7000);
        assert_eq!(a.cache.settings.revision, 4);
    }
}
