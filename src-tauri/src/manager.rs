use crate::model::{
    config_file, display_command, load_store_at, save_store_at, set_private, ssh_args,
    validate_draft, Snapshot, Tunnel, TunnelDraft, TunnelStatus, TunnelView, MAX_LOG_CHARS,
};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

pub struct Running {
    pub pid: u32,
    pub stop_requested: Arc<AtomicBool>,
    pub pw_file: PathBuf,
}

struct Live {
    status: TunnelStatus,
    message: Option<String>,
    last_line: Option<String>,
    log: String,
    running: Option<Running>,
}

pub struct StartPlan {
    pub id: String,
    pub args: Vec<String>,
    pub command: String,
    pub pw_file: PathBuf,
    pub askpass: PathBuf,
    pub stop_requested: Arc<AtomicBool>,
}

pub struct TunnelManager {
    app_dir: PathBuf,
    tunnel_file: PathBuf,
    tunnels: Vec<Tunnel>,
    live: HashMap<String, Live>,
    selected: Option<String>,
    shutting_down: bool,
}

impl TunnelManager {
    pub fn load(dir: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败：{e}"))?;
        clear_password_files(&dir);
        let tunnel_file = remembered_tunnel_file(&dir);
        let tunnels = load_store_at(&tunnel_file)?;
        let mut live = HashMap::new();
        for tunnel in &tunnels {
            live.insert(tunnel.id.clone(), Live::stopped());
        }
        write_askpass(&dir)?;
        Ok(Self {
            app_dir: dir,
            tunnel_file,
            tunnels,
            live,
            selected: None,
            shutting_down: false,
        })
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            tunnels: self
                .tunnels
                .iter()
                .map(|tunnel| self.view(tunnel))
                .collect(),
            selected_id: self.selected.clone(),
            config_path: self.tunnel_file.display().to_string(),
        }
    }

    pub fn log_of(&self, id: &str) -> String {
        self.live
            .get(id)
            .map(|live| live.log.clone())
            .unwrap_or_default()
    }

    pub fn select(&mut self, id: Option<String>) -> Result<(), String> {
        if let Some(id) = &id {
            if !self.tunnels.iter().any(|tunnel| &tunnel.id == id) {
                return Err("这条隧道已经不在列表里。".into());
            }
        }
        self.selected = id;
        Ok(())
    }

    pub fn upsert(&mut self, draft: TunnelDraft) -> Result<String, String> {
        let mut tunnel = validate_draft(&draft)?;
        if let Some(id) = draft.id.as_ref().filter(|id| !id.is_empty()) {
            let Some(existing) = self.tunnels.iter_mut().find(|item| &item.id == id) else {
                return Err("这条隧道已经不在列表里。".into());
            };
            tunnel.id = id.clone();
            *existing = tunnel;
            self.persist()?;
            Ok(id.clone())
        } else {
            tunnel.id = next_id();
            let id = tunnel.id.clone();
            self.tunnels.push(tunnel);
            self.live.insert(id.clone(), Live::stopped());
            self.selected = Some(id.clone());
            self.persist()?;
            return Ok(id);
        }
    }

    pub fn persist(&self) -> Result<(), String> {
        save_store_at(&self.tunnel_file, &self.tunnels)
    }

    pub fn set_tunnel_file(&mut self, path: PathBuf) -> Result<(), String> {
        if self
            .live
            .values()
            .any(|live| live.running.is_some())
        {
            return Err("先停掉正在跑的隧道，再更换配置文件。".into());
        }
        let path = if path.is_absolute() {
            path
        } else {
            return Err("配置文件需要一个完整路径。".into());
        };
        if path == self.tunnel_file {
            return Ok(());
        }
        if path.exists() {
            let tunnels = load_store_at(&path)?;
            self.tunnels = tunnels;
            self.live = self
                .tunnels
                .iter()
                .map(|tunnel| (tunnel.id.clone(), Live::stopped()))
                .collect();
            self.selected = None;
        } else {
            save_store_at(&path, &self.tunnels)?;
        }
        remember_tunnel_file(&self.app_dir, &path)?;
        self.tunnel_file = path;
        Ok(())
    }

    pub fn delete(&mut self, id: &str) -> Result<Option<u32>, String> {
        if !self.tunnels.iter().any(|tunnel| tunnel.id == id) {
            return Err("这条隧道已经不在列表里。".into());
        }
        self.tunnels.retain(|tunnel| tunnel.id != id);
        let pid = self.take_pid(id);
        self.live.remove(id);
        if self.selected.as_deref() == Some(id) {
            self.selected = None;
        }
        self.persist()?;
        Ok(pid)
    }

    pub fn prepare_start(&mut self, id: &str) -> Result<StartPlan, String> {
        if self.running_pid(id).is_some() {
            return Err("这条隧道已经在运行。".into());
        }
        let tunnel = self
            .tunnels
            .iter()
            .find(|tunnel| tunnel.id == id)
            .cloned()
            .ok_or("这条隧道已经不在列表里。")?;
        let pw_file = self.app_dir.join(format!("pw-{id}"));
        fs::write(&pw_file, &tunnel.password).map_err(|e| format!("无法暂存密码：{e}"))?;
        set_private(&pw_file);
        let askpass = askpass_path(&self.app_dir);
        write_askpass(&self.app_dir)?;
        let args = ssh_args(&tunnel, &self.app_dir.join("known_hosts"));
        let command = display_command(&args);
        let stop_requested = Arc::new(AtomicBool::new(false));
        if let Some(live) = self.live.get_mut(id) {
            live.log = String::new();
            live.message = None;
            live.last_line = None;
        }
        self.selected = Some(id.to_string());
        Ok(StartPlan {
            id: id.to_string(),
            args,
            command,
            pw_file,
            askpass,
            stop_requested,
        })
    }

    pub fn attach(&mut self, id: &str, running: Running) {
        let live = self.live.entry(id.to_string()).or_insert_with(Live::stopped);
        live.status = TunnelStatus::Running;
        live.message = None;
        live.running = Some(running);
    }

    pub fn fail_start(&mut self, id: &str, pw_file: &Path, message: String) {
        let _ = fs::remove_file(pw_file);
        if let Some(live) = self.live.get_mut(id) {
            live.status = TunnelStatus::Failed;
            live.message = Some(message.clone());
            live.running = None;
            push_text(&mut live.log, &message);
        }
    }

    pub fn push_log(&mut self, id: &str, line: &str) {
        if let Some(live) = self.live.get_mut(id) {
            push_text(&mut live.log, line);
            let trimmed = trim_message(line);
            live.last_line = Some(trimmed.clone());
            if live.status == TunnelStatus::Failed {
                live.message = Some(trimmed);
            }
        }
    }

    pub fn mark_exit(&mut self, id: &str, intentional: bool, code: Option<i32>) -> bool {
        let _ = fs::remove_file(self.app_dir.join(format!("pw-{id}")));
        let Some(live) = self.live.get_mut(id) else {
            return false;
        };
        live.running = None;
        if intentional {
            live.status = TunnelStatus::Stopped;
            live.message = None;
            live.last_line = None;
            push_text(&mut live.log, "已停止");
        } else {
            let fallback = match code {
                Some(code) => format!("进程已退出，退出码 {code}"),
                None => "进程已退出".into(),
            };
            let message = match live.last_line.clone() {
                Some(line) if !line.starts_with("ssh ") => line,
                _ => fallback,
            };
            live.status = TunnelStatus::Failed;
            live.message = Some(message.clone());
            if live.last_line.as_deref() != Some(message.as_str()) {
                push_text(&mut live.log, &message);
            }
        }
        true
    }

    pub fn request_stop(&mut self, id: &str) -> Option<u32> {
        let live = self.live.get_mut(id)?;
        if let Some(running) = &live.running {
            running.stop_requested.store(true, Ordering::SeqCst);
            Some(running.pid)
        } else {
            None
        }
    }

    pub fn stop_all(&mut self) -> Vec<u32> {
        self.shutting_down = true;
        self.live
            .values()
            .filter_map(|live| live.running.as_ref().map(|running| running.pid))
            .collect()
    }

    fn view(&self, tunnel: &Tunnel) -> TunnelView {
        let live = self.live.get(&tunnel.id);
        TunnelView {
            id: tunnel.id.clone(),
            host: tunnel.host.clone(),
            port: tunnel.port,
            user: tunnel.user.clone(),
            password: tunnel.password.clone(),
            forwards: tunnel.forwards.clone(),
            status: live.map(|item| item.status).unwrap_or(TunnelStatus::Stopped),
            message: live.and_then(|item| item.message.clone()),
        }
    }

    fn running_pid(&self, id: &str) -> Option<u32> {
        self.live
            .get(id)
            .and_then(|live| live.running.as_ref().map(|running| running.pid))
    }

    fn take_pid(&mut self, id: &str) -> Option<u32> {
        let live = self.live.get_mut(id)?;
        let running = live.running.take()?;
        running.stop_requested.store(true, Ordering::SeqCst);
        Some(running.pid)
    }
}

impl Live {
    fn stopped() -> Self {
        Self {
            status: TunnelStatus::Stopped,
            message: None,
            last_line: None,
            log: String::new(),
            running: None,
        }
    }
}

fn push_text(log: &mut String, line: &str) {
    if !log.is_empty() {
        log.push('\n');
    }
    log.push_str(line);
    if log.len() > MAX_LOG_CHARS {
        let extra = log.len() - MAX_LOG_CHARS;
        let mut cut = extra;
        while !log.is_char_boundary(cut) {
            cut += 1;
        }
        log.replace_range(..cut, "");
    }
}

fn trim_message(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.chars().count() > 180 {
        let short: String = trimmed.chars().take(180).collect();
        format!("{short}…")
    } else {
        trimmed.to_string()
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    tunnel_file: String,
}

fn settings_file(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

fn remembered_tunnel_file(dir: &Path) -> PathBuf {
    let fallback = config_file(dir);
    let Ok(text) = fs::read_to_string(settings_file(dir)) else {
        return fallback;
    };
    let Ok(settings) = serde_json::from_str::<AppSettings>(&text) else {
        return fallback;
    };
    let path = PathBuf::from(settings.tunnel_file);
    if path.as_os_str().is_empty() {
        fallback
    } else {
        path
    }
}

fn remember_tunnel_file(dir: &Path, path: &Path) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&AppSettings {
        tunnel_file: path.display().to_string(),
    })
    .map_err(|e| format!("写入设置失败：{e}"))?;
    let file = settings_file(dir);
    fs::write(&file, text).map_err(|e| format!("写入设置失败：{e}"))?;
    set_private(&file);
    Ok(())
}

fn next_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("{millis:x}{n:x}")
}

pub fn askpass_path(dir: &Path) -> PathBuf {
    dir.join("askpass.sh")
}

pub fn write_askpass(dir: &Path) -> Result<(), String> {
    let path = askpass_path(dir);
    let script = "#!/bin/sh\n/bin/cat \"$TAILTUNNEL_PW_FILE\"\n";
    fs::write(&path, script).map_err(|e| format!("无法写入密码助手：{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("无法设置密码助手权限：{e}"))?;
    }
    Ok(())
}

fn clear_password_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("pw-") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub fn signal_pid(pid: u32, sig: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, sig);
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, sig);
    }
}
