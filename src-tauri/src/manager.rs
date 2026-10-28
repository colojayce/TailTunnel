use crate::model::{
    LogEntry, LogPage, MAX_LOG_CHARS, Snapshot, Tunnel, TunnelDraft, TunnelStatus, TunnelView,
    config_file, display_command, jump_prompt, load_store_at, save_store_at, set_private, ssh_args,
    tunnel_export, validate_draft,
};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub struct Running {
    pub pid: u32,
    pub stop_requested: Arc<AtomicBool>,
    pub pw_file: PathBuf,
}

struct Live {
    status: TunnelStatus,
    message: Option<String>,
    last_line: Option<String>,
    lines: Vec<LogEntry>,
    next_seq: u64,
    floor: u64,
    connected_at: Option<u64>,
    running: Option<Running>,
    starting: bool,
    /// First log line of the current connection attempt.
    attempt_floor: u64,
    /// Whether the attempt that just ended had been connected.
    was_connected: bool,
    /// Token of the scheduled reconnect; clearing it cancels the reconnect.
    retry: Option<u64>,
    /// Reconnects since the connection was last stable, for the back-off.
    attempts: u32,
    /// Inside a run of reconnects that has already been announced.
    series: bool,
}

/// A system notification to show, if the window is not in front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

pub struct LogUpdate {
    pub seq: u64,
    pub established: bool,
    pub notice: Option<Notice>,
}

pub struct RetryPlan {
    pub delay_secs: u64,
    pub token: u64,
    pub seq: u64,
    pub line: String,
}

#[derive(Default)]
pub struct AfterExit {
    pub retry: Option<RetryPlan>,
    pub notice: Option<Notice>,
}

pub struct StartPlan {
    pub id: String,
    pub args: Vec<String>,
    pub command: String,
    /// Log line announcing a reconnect, written before the command.
    pub banner: Option<String>,
    pub pw_file: PathBuf,
    pub jump_pw_file: Option<PathBuf>,
    pub jump_prompt: Option<String>,
    pub askpass: PathBuf,
    pub stop_requested: Arc<AtomicBool>,
}

/// Seconds to wait before each reconnect; the last value repeats.
pub const RETRY_DELAYS: [u64; 5] = [2, 5, 10, 30, 60];
/// A connection that stayed up this long starts the back-off over.
const STABLE_MS: u64 = 60_000;
/// Error from `prepare_retry` when the reconnect was cancelled meanwhile.
pub const RETRY_CANCELLED: &str = "重连已取消。";

pub struct TunnelManager {
    app_dir: PathBuf,
    tunnel_file: PathBuf,
    tunnels: Vec<Tunnel>,
    live: HashMap<String, Live>,
    selected: Option<String>,
    shutting_down: bool,
    notify: bool,
    retry_counter: u64,
}

impl TunnelManager {
    pub fn load(dir: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败：{e}"))?;
        clear_password_files(&dir);
        let settings = read_settings(&dir);
        let tunnel_file = if settings.tunnel_file.is_empty() {
            config_file(&dir)
        } else {
            PathBuf::from(&settings.tunnel_file)
        };
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
            notify: settings.notify,
            retry_counter: 0,
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
            notify: self.notify,
        }
    }

    pub fn set_notify(&mut self, on: bool) -> Result<(), String> {
        write_settings(
            &self.app_dir,
            &AppSettings {
                tunnel_file: self.tunnel_file.display().to_string(),
                notify: on,
            },
        )?;
        self.notify = on;
        Ok(())
    }

    pub fn log_of(&self, id: &str) -> LogPage {
        self.live
            .get(id)
            .map(|live| LogPage {
                floor: live.floor,
                lines: live.lines.clone(),
            })
            .unwrap_or(LogPage {
                floor: 0,
                lines: Vec::new(),
            })
    }

    pub fn export_text(&self, id: &str) -> Result<String, String> {
        let tunnel = self
            .tunnels
            .iter()
            .find(|tunnel| tunnel.id == id)
            .ok_or("这条隧道已经不在列表里。")?;
        tunnel_export(tunnel)
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
        if self.live.values().any(|live| live.running.is_some()) {
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
        write_settings(
            &self.app_dir,
            &AppSettings {
                tunnel_file: path.display().to_string(),
                notify: self.notify,
            },
        )?;
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
        self.prepare(id, None)
    }

    /// Starts the reconnect scheduled under `token`, unless it was cancelled.
    pub fn prepare_retry(&mut self, id: &str, token: u64) -> Result<StartPlan, String> {
        self.prepare(id, Some(token))
    }

    pub fn retry_pending(&self, id: &str, token: u64) -> bool {
        !self.shutting_down && self.live.get(id).and_then(|live| live.retry) == Some(token)
    }

    fn prepare(&mut self, id: &str, retry: Option<u64>) -> Result<StartPlan, String> {
        if let Some(token) = retry {
            if !self.retry_pending(id, token) {
                return Err(RETRY_CANCELLED.into());
            }
        }
        if self.live.get(id).is_some_and(|live| live.starting) {
            return Err("这条隧道正在连接。".into());
        }
        if self.running_pid(id).is_some() {
            let connecting = self
                .live
                .get(id)
                .is_some_and(|live| live.status == TunnelStatus::Connecting);
            return Err(if connecting {
                "这条隧道正在连接。".into()
            } else {
                "这条隧道已经在运行。".into()
            });
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
        let (jump_pw_file, jump_prompt) = match &tunnel.jump {
            Some(jump) => {
                let path = jump_sibling(&pw_file);
                fs::write(&path, &jump.password).map_err(|e| {
                    let _ = fs::remove_file(&pw_file);
                    format!("无法暂存密码：{e}")
                })?;
                set_private(&path);
                (Some(path), Some(jump_prompt(jump)))
            }
            None => (None, None),
        };
        let askpass = askpass_path(&self.app_dir);
        write_askpass(&self.app_dir)?;
        let args = ssh_args(&tunnel, &self.app_dir.join("known_hosts"));
        let command = display_command(&args);
        let stop_requested = Arc::new(AtomicBool::new(false));
        let mut banner = None;
        if let Some(live) = self.live.get_mut(id) {
            if retry.is_some() {
                // Keep the log of the drop so the reconnects read as one story.
                banner = Some(format!("第 {} 次重连", live.attempts));
            } else {
                live.lines.clear();
                live.floor = live.next_seq;
                live.attempts = 0;
                live.series = false;
            }
            live.retry = None;
            live.attempt_floor = live.next_seq;
            live.message = None;
            live.last_line = None;
            live.connected_at = None;
            live.status = TunnelStatus::Connecting;
            live.starting = true;
        }
        self.selected = Some(id.to_string());
        Ok(StartPlan {
            id: id.to_string(),
            args,
            command,
            banner,
            pw_file,
            jump_pw_file,
            jump_prompt,
            askpass,
            stop_requested,
        })
    }

    pub fn probe_plan(&self, id: &str) -> Result<(Tunnel, PathBuf), String> {
        let tunnel = self
            .tunnels
            .iter()
            .find(|tunnel| tunnel.id == id)
            .cloned()
            .ok_or("这条隧道已经不在列表里。")?;
        Ok((tunnel, self.app_dir.clone()))
    }

    pub fn attach(&mut self, id: &str, running: Running) {
        let live = self
            .live
            .entry(id.to_string())
            .or_insert_with(Live::stopped);
        live.status = TunnelStatus::Connecting;
        live.message = None;
        live.connected_at = None;
        live.starting = false;
        live.running = Some(running);
    }

    pub fn fail_start(&mut self, id: &str, pw_file: &Path, message: String) -> Option<u64> {
        let _ = fs::remove_file(pw_file);
        let _ = fs::remove_file(jump_sibling(pw_file));
        let live = self.live.get_mut(id)?;
        live.status = TunnelStatus::Failed;
        live.message = Some(message.clone());
        live.connected_at = None;
        live.running = None;
        live.starting = false;
        live.retry = None;
        live.series = false;
        Some(append_line(live, &message))
    }

    pub fn push_log(&mut self, id: &str, line: &str) -> Option<LogUpdate> {
        let live = self.live.get_mut(id)?;
        let seq = append_line(live, line);
        let trimmed = trim_message(line);
        if !is_noise_line(line) {
            live.last_line = Some(trimmed.clone());
            if live.status == TunnelStatus::Failed {
                live.message = Some(trimmed);
            }
        }
        let mut established = false;
        let mut notice = None;
        if live.status == TunnelStatus::Connecting && session_ready(line) {
            live.status = TunnelStatus::Running;
            live.connected_at = Some(now_ms());
            live.message = None;
            established = true;
            if live.series {
                live.series = false;
                if self.notify {
                    notice = self.tunnels.iter().find(|tunnel| tunnel.id == id).map(|tunnel| Notice {
                        title: tunnel.display_name(),
                        body: "已重新连上。".into(),
                    });
                }
            }
        }
        Some(LogUpdate {
            seq,
            established,
            notice,
        })
    }

    pub fn mark_exit(&mut self, id: &str, intentional: bool, code: Option<i32>) -> bool {
        let pw_file = self.app_dir.join(format!("pw-{id}"));
        let _ = fs::remove_file(jump_sibling(&pw_file));
        let _ = fs::remove_file(pw_file);
        let Some(live) = self.live.get_mut(id) else {
            return false;
        };
        live.was_connected = live.connected_at.is_some();
        if live
            .connected_at
            .is_some_and(|at| now_ms().saturating_sub(at) >= STABLE_MS)
        {
            live.attempts = 0;
        }
        live.running = None;
        live.connected_at = None;
        live.starting = false;
        if intentional {
            live.status = TunnelStatus::Stopped;
            live.message = None;
            live.last_line = None;
            live.retry = None;
            live.attempts = 0;
            live.series = false;
            append_line(live, "已停止");
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
                append_line(live, &message);
            }
        }
        true
    }

    pub fn address_in_use_ports(&self, id: &str) -> Vec<u16> {
        let Some(live) = self.live.get(id) else {
            return Vec::new();
        };
        if live.status != TunnelStatus::Failed {
            return Vec::new();
        }
        let hit = live
            .lines
            .iter()
            .any(|entry| mentions_address_in_use(&entry.line))
            || live.message.as_deref().is_some_and(mentions_address_in_use);
        if !hit {
            return Vec::new();
        }
        let Some(tunnel) = self.tunnels.iter().find(|tunnel| tunnel.id == id) else {
            return Vec::new();
        };
        let mut ports: Vec<u16> = tunnel
            .forwards
            .iter()
            .map(|forward| forward.local_port)
            .collect();
        ports.sort_unstable();
        ports.dedup();
        let mut named = Vec::new();
        for line in live
            .lines
            .iter()
            .map(|entry| entry.line.as_str())
            .chain(live.message.as_deref())
        {
            if !mentions_address_in_use(line) {
                continue;
            }
            for port in &ports {
                if line_mentions_port(line, *port) && !named.contains(port) {
                    named.push(*port);
                }
            }
        }
        if named.is_empty() { ports } else { named }
    }

    pub fn log_token(&self, id: &str) -> Option<u64> {
        self.live.get(id).map(|live| live.next_seq)
    }

    pub fn set_failure_message(&mut self, id: &str, message: String, token: u64) -> bool {
        let Some(live) = self.live.get_mut(id) else {
            return false;
        };
        if live.status != TunnelStatus::Failed || live.next_seq != token {
            return false;
        }
        live.message = Some(message);
        true
    }

    /// Decides what follows an unexpected exit: a scheduled reconnect when the
    /// tunnel asks for one and the failure is worth retrying, and whether to
    /// tell the user. `token` is `log_token` taken right after `mark_exit`.
    pub fn after_exit(&mut self, id: &str, token: u64) -> AfterExit {
        let mut outcome = AfterExit::default();
        if self.shutting_down {
            return outcome;
        }
        let Some(tunnel) = self.tunnels.iter().find(|tunnel| tunnel.id == id) else {
            return outcome;
        };
        let name = tunnel.display_name();
        let auto = tunnel.auto_reconnect;
        let retry_token = self.retry_counter + 1;
        let Some(live) = self.live.get_mut(id) else {
            return outcome;
        };
        if live.status != TunnelStatus::Failed || live.next_seq != token || live.running.is_some()
        {
            return outcome;
        }
        let reason = live.message.clone().unwrap_or_default();
        let body = if auto && !attempt_hit_fatal(live) {
            self.retry_counter = retry_token;
            live.attempts = live.attempts.saturating_add(1);
            let delay_secs = RETRY_DELAYS[(live.attempts as usize - 1).min(RETRY_DELAYS.len() - 1)];
            let line = format!("{delay_secs} 秒后第 {} 次重连", live.attempts);
            live.status = TunnelStatus::Waiting;
            live.retry = Some(retry_token);
            live.message = Some(if reason.is_empty() {
                line.clone()
            } else {
                format!("{line} · {reason}")
            });
            let seq = append_line(live, &line);
            outcome.retry = Some(RetryPlan {
                delay_secs,
                token: retry_token,
                seq,
                line,
            });
            if live.series {
                None
            } else {
                live.series = true;
                Some(if live.was_connected {
                    format!("连接断开，正在自动重连。{reason}")
                } else {
                    format!("没有连上，正在自动重连。{reason}")
                })
            }
        } else if live.series {
            live.series = false;
            Some(format!("自动重连失败：{reason}"))
        } else if live.was_connected {
            Some(format!("连接断开：{reason}"))
        } else {
            Some(format!("连接失败：{reason}"))
        };
        if self.notify {
            outcome.notice = body.map(|body| Notice { title: name, body });
        }
        outcome
    }

    /// Marks a start that was refused before ssh ran (a busy port). For a
    /// reconnect, does nothing once the reconnect has been cancelled.
    pub fn refuse_start(
        &mut self,
        id: &str,
        message: &str,
        retry: Option<u64>,
    ) -> Option<(u64, Option<Notice>)> {
        if let Some(token) = retry {
            if !self.retry_pending(id, token) {
                return None;
            }
        }
        let name = self
            .tunnels
            .iter()
            .find(|tunnel| tunnel.id == id)?
            .display_name();
        let live = self.live.get_mut(id)?;
        if live.running.is_some() || live.starting {
            return None;
        }
        let body = if live.series {
            format!("自动重连失败：{message}")
        } else {
            format!("没有启动：{message}")
        };
        live.status = TunnelStatus::Failed;
        live.message = Some(message.to_string());
        live.connected_at = None;
        live.retry = None;
        live.series = false;
        let seq = append_line(live, message);
        let notice = self.notify.then(|| Notice { title: name, body });
        Some((seq, notice))
    }

    /// Local ports this tunnel forwards, without repeats.
    pub fn local_ports(&self, id: &str) -> Vec<u16> {
        let mut ports: Vec<u16> = self
            .tunnels
            .iter()
            .find(|tunnel| tunnel.id == id)
            .map(|tunnel| tunnel.forwards.iter().map(|f| f.local_port).collect())
            .unwrap_or_default();
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    /// Process ids of every ssh this app is running.
    pub fn tunnel_pids(&self) -> Vec<u32> {
        self.live
            .values()
            .filter_map(|live| live.running.as_ref().map(|running| running.pid))
            .collect()
    }

    /// Sentences about other tunnels that forward `port`. With `busy_only`,
    /// only tunnels that hold the port right now count.
    pub fn port_neighbors(&self, id: Option<&str>, port: u16, busy_only: bool) -> Vec<String> {
        self.tunnels
            .iter()
            .filter(|tunnel| Some(tunnel.id.as_str()) != id)
            .filter(|tunnel| tunnel.forwards.iter().any(|f| f.local_port == port))
            .filter_map(|tunnel| {
                let busy = self
                    .live
                    .get(&tunnel.id)
                    .is_some_and(|live| live.running.is_some() || live.starting);
                if busy {
                    Some(format!("端口 {port} 正被隧道「{}」使用。", tunnel.display_name()))
                } else if busy_only {
                    None
                } else {
                    Some(format!(
                        "端口 {port} 也用在隧道「{}」里，两条不能同时运行。",
                        tunnel.display_name()
                    ))
                }
            })
            .collect()
    }

    /// Stops a running tunnel (returning the pid to signal), or cancels a
    /// scheduled reconnect.
    pub fn request_stop(&mut self, id: &str) -> Option<u32> {
        let live = self.live.get_mut(id)?;
        if let Some(running) = &live.running {
            running.stop_requested.store(true, Ordering::SeqCst);
            return Some(running.pid);
        }
        if live.retry.is_some() || live.status == TunnelStatus::Waiting {
            live.retry = None;
            live.attempts = 0;
            live.series = false;
            live.status = TunnelStatus::Stopped;
            live.message = None;
            live.last_line = None;
            append_line(live, "已停止");
        }
        None
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
            note: tunnel.note.clone(),
            host: tunnel.host.clone(),
            port: tunnel.port,
            user: tunnel.user.clone(),
            password: tunnel.password.clone(),
            auth: tunnel.auth,
            key_file: tunnel.key_file.clone(),
            jump: tunnel.jump.clone(),
            forwards: tunnel.forwards.clone(),
            auto_reconnect: tunnel.auto_reconnect,
            status: live
                .map(|item| item.status)
                .unwrap_or(TunnelStatus::Stopped),
            message: live.and_then(|item| item.message.clone()),
            connected_at: live.and_then(|item| item.connected_at),
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
            lines: Vec::new(),
            next_seq: 0,
            floor: 0,
            connected_at: None,
            running: None,
            starting: false,
            attempt_floor: 0,
            was_connected: false,
            retry: None,
            attempts: 0,
            series: false,
        }
    }
}

/// Failures that another try cannot fix: wrong credentials, a changed host
/// key, a bad key file or a local port that is taken.
fn is_fatal_line(line: &str) -> bool {
    if is_noise_line(line) {
        return false;
    }
    let lower = line.to_ascii_lowercase();
    [
        "permission denied",
        "host key verification failed",
        "remote host identification has changed",
        "address already in use",
        "unprotected private key",
        "load key",
        "not accessible",
        "incorrect passphrase",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn attempt_hit_fatal(live: &Live) -> bool {
    live.lines
        .iter()
        .filter(|entry| entry.seq >= live.attempt_floor)
        .any(|entry| is_fatal_line(&entry.line))
}

pub fn session_ready(line: &str) -> bool {
    line.contains("Local forwarding listening") || line.contains("Entering interactive session")
}

fn is_noise_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("debug1:")
        || trimmed.starts_with("debug2:")
        || trimmed.starts_with("debug3:")
        || trimmed.starts_with("OpenSSH_")
        || trimmed.starts_with("Authenticated to ")
        || trimmed.starts_with("Transferred:")
        || trimmed.starts_with("Bytes per second:")
}

fn mentions_address_in_use(line: &str) -> bool {
    line.to_ascii_lowercase().contains("address already in use")
}

fn line_mentions_port(line: &str, port: u16) -> bool {
    let token = port.to_string();
    line.split(|ch: char| !ch.is_ascii_digit())
        .any(|part| part == token)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub command: String,
    pub pid: u32,
}

pub fn parse_lsof(output: &str) -> Vec<Listener> {
    let mut rows = Vec::new();
    let mut pid: Option<u32> = None;
    let mut command = String::new();
    let mut listen = false;
    let flush = |rows: &mut Vec<(u32, String, bool)>,
                 pid: &mut Option<u32>,
                 command: &mut String,
                 listen: &mut bool| {
        if let Some(pid_value) = pid.take() {
            let name = if command.is_empty() {
                "未知进程".to_string()
            } else {
                std::mem::take(command)
            };
            rows.push((pid_value, name, *listen));
        } else {
            command.clear();
        }
        *listen = false;
    };
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if line.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
            continue;
        }
        if let Some(rest) = line.strip_prefix('p') {
            if !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()) {
                flush(&mut rows, &mut pid, &mut command, &mut listen);
                pid = rest.parse().ok();
                continue;
            }
        }
        if pid.is_none() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('c') {
            if command.is_empty() {
                command = rest.to_string();
            }
            continue;
        }
        if line == "TST=LISTEN" {
            listen = true;
        }
    }
    flush(&mut rows, &mut pid, &mut command, &mut listen);
    let mut seen = Vec::new();
    let mut listeners = Vec::new();
    for (pid, command, listen) in rows {
        if !listen {
            continue;
        }
        if seen.contains(&pid) {
            continue;
        }
        seen.push(pid);
        listeners.push(Listener { command, pid });
    }
    listeners
}

pub fn conflict_sentence(port: u16, listeners: &[Listener]) -> Option<String> {
    if listeners.is_empty() {
        return None;
    }
    let who = listeners
        .iter()
        .map(|item| format!("{}（pid {}）", item.command, item.pid))
        .collect::<Vec<_>>()
        .join("、");
    Some(format!("端口 {port} 已被 {who}占用。"))
}

fn append_line(live: &mut Live, line: &str) -> u64 {
    let seq = live.next_seq;
    live.next_seq = live.next_seq.saturating_add(1);
    live.lines.push(LogEntry {
        seq,
        line: line.to_string(),
    });
    while live.lines.len() > 1 && log_bytes(&live.lines) > MAX_LOG_CHARS {
        live.lines.remove(0);
    }
    seq
}

fn log_bytes(lines: &[LogEntry]) -> usize {
    if lines.is_empty() {
        return 0;
    }
    lines.iter().map(|line| line.line.len()).sum::<usize>() + lines.len() - 1
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
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
    #[serde(default)]
    tunnel_file: String,
    #[serde(default = "notify_by_default")]
    notify: bool,
}

fn notify_by_default() -> bool {
    true
}

fn settings_file(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

fn read_settings(dir: &Path) -> AppSettings {
    fs::read_to_string(settings_file(dir))
        .ok()
        .and_then(|text| serde_json::from_str::<AppSettings>(&text).ok())
        .unwrap_or(AppSettings {
            tunnel_file: String::new(),
            notify: true,
        })
}

fn write_settings(dir: &Path, settings: &AppSettings) -> Result<(), String> {
    let text = serde_json::to_string_pretty(settings).map_err(|e| format!("写入设置失败：{e}"))?;
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

/// ssh asks for each hop's password separately and names the account in the
/// prompt ("user@host's password: "), so the jump hop is recognised by that text.
pub const ASKPASS_SCRIPT: &str = r#"#!/bin/sh
if [ -n "$TAILTUNNEL_JUMP_PROMPT" ] && [ "${1#"$TAILTUNNEL_JUMP_PROMPT"}" != "$1" ]; then
  exec /bin/cat "$TAILTUNNEL_JUMP_PW_FILE"
fi
exec /bin/cat "$TAILTUNNEL_PW_FILE"
"#;

pub fn jump_sibling(pw_file: &Path) -> PathBuf {
    let mut name = pw_file.file_name().unwrap_or_default().to_os_string();
    name.push("-jump");
    pw_file.with_file_name(name)
}

pub fn askpass_path(dir: &Path) -> PathBuf {
    dir.join("askpass.sh")
}

pub fn write_askpass(dir: &Path) -> Result<(), String> {
    let path = askpass_path(dir);
    fs::write(&path, ASKPASS_SCRIPT).map_err(|e| format!("无法写入密码助手：{e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ForwardDraft, MAX_LOG_CHARS, TunnelDraft, TunnelStatus};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn draft() -> TunnelDraft {
        TunnelDraft {
            id: None,
            note: String::new(),
            host: "100.64.1.2".into(),
            port: 22,
            user: "root".into(),
            password: "s3cret".into(),
            auth: Default::default(),
            key_file: String::new(),
            jump: None,
            forwards: vec![ForwardDraft {
                local_port: 8080,
                remote_host: "127.0.0.1".into(),
                remote_port: 80,
            }],
            auto_reconnect: false,
        }
    }

    fn reconnecting() -> TunnelDraft {
        let mut item = draft();
        item.note = "NAS".into();
        item.auto_reconnect = true;
        item
    }

    /// Runs a tunnel until it is connected, then lets it drop with `line`.
    fn drop_with(manager: &mut TunnelManager, id: &str, line: &str, connect: bool) -> AfterExit {
        let plan = match manager.live.get(id).and_then(|live| live.retry) {
            Some(token) => manager.prepare_retry(id, token).unwrap(),
            None => manager.prepare_start(id).unwrap(),
        };
        manager.attach(
            id,
            Running {
                pid: 424_242,
                stop_requested: plan.stop_requested,
                pw_file: plan.pw_file,
            },
        );
        if connect {
            manager
                .push_log(id, "debug1: Local forwarding listening on 127.0.0.1 port 8080.")
                .unwrap();
        }
        manager.push_log(id, line).unwrap();
        assert!(manager.mark_exit(id, false, Some(255)));
        let token = manager.log_token(id).unwrap();
        manager.after_exit(id, token)
    }

    #[test]
    fn a_drop_schedules_reconnects_with_growing_delays() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(reconnecting()).unwrap();
        let first = drop_with(&mut manager, &id, "Connection reset by peer", true);
        let retry = first.retry.expect("重连");
        assert_eq!(retry.delay_secs, 2);
        assert_eq!(retry.line, "2 秒后第 1 次重连");
        assert_eq!(
            first.notice,
            Some(Notice {
                title: "NAS".into(),
                body: "连接断开，正在自动重连。Connection reset by peer".into(),
            })
        );
        let view = manager.snapshot().tunnels.into_iter().next().unwrap();
        assert_eq!(view.status, TunnelStatus::Waiting);
        assert_eq!(
            view.message.as_deref(),
            Some("2 秒后第 1 次重连 · Connection reset by peer")
        );

        // Further failures back off and stay quiet.
        let second = drop_with(&mut manager, &id, "Connection timed out", false);
        assert_eq!(second.retry.unwrap().delay_secs, 5);
        assert!(second.notice.is_none());
        // The log of the first drop is kept across reconnects.
        assert!(manager
            .log_of(&id)
            .lines
            .iter()
            .any(|entry| entry.line == "Connection reset by peer"));
        assert!(manager.log_of(&id).lines.iter().any(|entry| entry.line == "5 秒后第 2 次重连"));

        // Coming back up is announced once.
        let token = manager.live[&id].retry.unwrap();
        let plan = manager.prepare_retry(&id, token).unwrap();
        assert_eq!(plan.banner.as_deref(), Some("第 2 次重连"));
        manager.attach(
            &id,
            Running {
                pid: 1,
                stop_requested: plan.stop_requested,
                pw_file: plan.pw_file,
            },
        );
        let up = manager
            .push_log(&id, "debug1: Local forwarding listening on 127.0.0.1 port 8080.")
            .unwrap();
        assert_eq!(up.notice.map(|notice| notice.body).as_deref(), Some("已重新连上。"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn delays_stop_growing_at_the_last_step() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(reconnecting()).unwrap();
        let mut delays = Vec::new();
        for _ in 0..7 {
            delays.push(drop_with(&mut manager, &id, "Connection refused", false).retry.unwrap().delay_secs);
        }
        assert_eq!(delays, vec![2, 5, 10, 30, 60, 60, 60]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn wrong_password_or_busy_port_is_not_retried() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(reconnecting()).unwrap();
        let denied = drop_with(
            &mut manager,
            &id,
            "root@100.64.1.2: Permission denied (publickey,password).",
            false,
        );
        assert!(denied.retry.is_none());
        assert_eq!(
            denied.notice.unwrap().body,
            "连接失败：root@100.64.1.2: Permission denied (publickey,password)."
        );
        assert_eq!(manager.snapshot().tunnels[0].status, TunnelStatus::Failed);

        let busy = drop_with(&mut manager, &id, "bind [127.0.0.1]:8080: Address already in use", false);
        assert!(busy.retry.is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_fatal_line_from_an_earlier_attempt_does_not_block_retries() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(reconnecting()).unwrap();
        // A debug line mentioning a denial is noise, not a verdict.
        let first = drop_with(&mut manager, &id, "debug1: Permission denied hint", true);
        assert!(first.retry.is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn without_auto_reconnect_a_drop_only_notifies() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        let outcome = drop_with(&mut manager, &id, "Connection reset by peer", true);
        assert!(outcome.retry.is_none());
        assert_eq!(
            outcome.notice,
            Some(Notice {
                title: "root@100.64.1.2".into(),
                body: "连接断开：Connection reset by peer".into(),
            })
        );
        manager.set_notify(false).unwrap();
        let quiet = drop_with(&mut manager, &id, "Connection reset by peer", true);
        assert!(quiet.notice.is_none());
        let reloaded = TunnelManager::load(dir.clone()).unwrap();
        assert!(!reloaded.snapshot().notify);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stopping_a_waiting_tunnel_cancels_the_reconnect() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(reconnecting()).unwrap();
        let retry = drop_with(&mut manager, &id, "Connection reset by peer", true)
            .retry
            .unwrap();
        assert_eq!(manager.request_stop(&id), None);
        assert_eq!(manager.snapshot().tunnels[0].status, TunnelStatus::Stopped);
        assert!(!manager.retry_pending(&id, retry.token));
        assert_eq!(
            manager.prepare_retry(&id, retry.token).err().as_deref(),
            Some(RETRY_CANCELLED)
        );
        assert!(manager.refuse_start(&id, "端口被占", Some(retry.token)).is_none());
        assert_eq!(manager.snapshot().tunnels[0].status, TunnelStatus::Stopped);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_manual_start_replaces_a_scheduled_reconnect() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(reconnecting()).unwrap();
        let retry = drop_with(&mut manager, &id, "Connection reset by peer", true)
            .retry
            .unwrap();
        manager.prepare_start(&id).unwrap();
        assert!(!manager.retry_pending(&id, retry.token));
        assert!(manager.log_of(&id).lines.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_refused_reconnect_ends_the_series_with_a_notice() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(reconnecting()).unwrap();
        let retry = drop_with(&mut manager, &id, "Connection reset by peer", true)
            .retry
            .unwrap();
        let (_, notice) = manager
            .refuse_start(&id, "端口 8080 已被 node（pid 9）占用。", Some(retry.token))
            .unwrap();
        assert_eq!(
            notice.unwrap().body,
            "自动重连失败：端口 8080 已被 node（pid 9）占用。"
        );
        let view = manager.snapshot().tunnels.into_iter().next().unwrap();
        assert_eq!(view.status, TunnelStatus::Failed);
        assert!(!manager.retry_pending(&id, retry.token));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn other_tunnels_on_the_same_port_are_named() {
        let (dir, mut manager) = loaded();
        let a = manager.upsert(reconnecting()).unwrap();
        let mut other = draft();
        other.note = "备用".into();
        let b = manager.upsert(other).unwrap();
        assert_eq!(
            manager.port_neighbors(Some(&a), 8080, false),
            vec!["端口 8080 也用在隧道「备用」里，两条不能同时运行。".to_string()]
        );
        assert!(manager.port_neighbors(Some(&a), 8080, true).is_empty());
        assert!(manager.port_neighbors(Some(&a), 9090, false).is_empty());
        running(&mut manager, &b);
        assert_eq!(
            manager.port_neighbors(Some(&a), 8080, true),
            vec!["端口 8080 正被隧道「备用」使用。".to_string()]
        );
        assert_eq!(manager.port_neighbors(None, 8080, true).len(), 1);
        assert_eq!(manager.tunnel_pids(), vec![424_242]);
        assert_eq!(manager.local_ports(&a), vec![8080]);
        let _ = fs::remove_dir_all(dir);
    }

    fn loaded() -> (PathBuf, TunnelManager) {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tailtunnel-mgr-{}-{n}", std::process::id()));
        let manager = TunnelManager::load(dir.clone()).expect("临时目录");
        (dir, manager)
    }

    #[test]
    fn note_is_kept_and_falls_back_to_the_host() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        let saved = manager.snapshot().tunnels.into_iter().next().unwrap();
        assert_eq!(saved.note, "");
        assert_eq!(saved.host, "100.64.1.2");

        let mut item = draft();
        item.id = Some(id.clone());
        item.note = "  \u{200b}\u{3000}家里的 NAS\n ".into();
        manager.upsert(item).unwrap();
        let renamed = manager.snapshot().tunnels.into_iter().next().unwrap();
        assert_eq!(renamed.note, "家里的 NAS");

        let reloaded = TunnelManager::load(dir.clone()).expect("重新读取");
        assert_eq!(reloaded.snapshot().tunnels[0].note, "家里的 NAS");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stays_connecting_until_the_forward_is_listening() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        let plan = manager.prepare_start(&id).unwrap();
        manager.attach(
            &id,
            Running {
                pid: 424_242,
                stop_requested: plan.stop_requested,
                pw_file: plan.pw_file,
            },
        );
        let view = manager.snapshot().tunnels.into_iter().next().unwrap();
        assert_eq!(view.status, TunnelStatus::Connecting);
        assert!(view.connected_at.is_none());

        let early = manager
            .push_log(
                &id,
                "debug1: Connecting to 100.64.1.2 [100.64.1.2] port 22.",
            )
            .unwrap();
        assert!(!early.established);
        assert_eq!(
            manager.snapshot().tunnels[0].status,
            TunnelStatus::Connecting
        );

        let ready = manager
            .push_log(
                &id,
                "debug1: Local forwarding listening on 127.0.0.1 port 8080.",
            )
            .unwrap();
        assert!(ready.established);
        let view = manager.snapshot().tunnels.into_iter().next().unwrap();
        assert_eq!(view.status, TunnelStatus::Running);
        let started = view.connected_at.expect("已记录连接时间");

        let again = manager
            .push_log(
                &id,
                "debug1: Local forwarding listening on 127.0.0.1 port 8080.",
            )
            .unwrap();
        assert!(!again.established);
        assert_eq!(manager.snapshot().tunnels[0].connected_at, Some(started));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn interactive_session_also_counts_as_connected() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        let plan = manager.prepare_start(&id).unwrap();
        manager.attach(
            &id,
            Running {
                pid: 424_242,
                stop_requested: plan.stop_requested,
                pw_file: plan.pw_file,
            },
        );
        let ready = manager
            .push_log(&id, "debug1: Entering interactive session.")
            .unwrap();
        assert!(ready.established);
        assert_eq!(manager.snapshot().tunnels[0].status, TunnelStatus::Running);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repeated_lines_keep_their_own_sequence() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        manager.prepare_start(&id).unwrap();
        let first = manager.push_log(&id, "debug1: keepalive").unwrap();
        let second = manager.push_log(&id, "debug1: keepalive").unwrap();
        assert_ne!(first.seq, second.seq);
        let page = manager.log_of(&id);
        assert_eq!(page.lines.len(), 2);
        assert_eq!(page.lines[0].line, page.lines[1].line);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn restart_drops_old_lines_and_raises_the_floor() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        manager.prepare_start(&id).unwrap();
        manager.push_log(&id, "a").unwrap();
        manager.push_log(&id, "b").unwrap();
        manager.fail_start(&id, &dir.join(format!("pw-{id}")), "没连上".into());
        manager.prepare_start(&id).unwrap();
        let page = manager.log_of(&id);
        assert!(page.lines.is_empty());
        assert_eq!(page.floor, 3);
        let next = manager.push_log(&id, "c").unwrap();
        assert_eq!(next.seq, 3);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn long_log_drops_oldest_whole_lines() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        let line = "x".repeat(30_000);
        manager.push_log(&id, &line).unwrap();
        manager.push_log(&id, &line).unwrap();
        manager.push_log(&id, &line).unwrap();
        let page = manager.log_of(&id);
        assert_eq!(page.lines.len(), 2);
        assert_eq!(page.lines[0].seq, 1);
        assert_eq!(page.lines[1].seq, 2);
        assert!(super::log_bytes(&page.lines) <= MAX_LOG_CHARS);
        let _ = fs::remove_dir_all(dir);
    }

    fn running(manager: &mut TunnelManager, id: &str) {
        let plan = manager.prepare_start(id).unwrap();
        manager.attach(
            id,
            Running {
                pid: 424_242,
                stop_requested: plan.stop_requested,
                pw_file: plan.pw_file,
            },
        );
    }

    #[test]
    fn failure_reason_keeps_the_plain_line_when_debug_follows() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        running(&mut manager, &id);
        manager.push_log(&id, "ssh -N -v root@100.64.1.2").unwrap();
        manager
            .push_log(
                &id,
                "root@100.64.1.2: Permission denied (publickey,password).",
            )
            .unwrap();
        manager
            .push_log(
                &id,
                "debug1: Authentications that can continue: publickey,password",
            )
            .unwrap();
        manager.push_log(&id, "debug2: more detail").unwrap();
        manager.push_log(&id, "debug3: even more").unwrap();
        assert!(manager.mark_exit(&id, false, Some(255)));
        let view = manager.snapshot().tunnels.into_iter().next().unwrap();
        assert_eq!(view.status, TunnelStatus::Failed);
        assert_eq!(
            view.message.as_deref(),
            Some("root@100.64.1.2: Permission denied (publickey,password).")
        );
        manager
            .push_log(&id, "debug1: this arrived after exit")
            .unwrap();
        assert_eq!(
            manager.snapshot().tunnels[0].message.as_deref(),
            Some("root@100.64.1.2: Permission denied (publickey,password).")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failure_without_a_plain_line_uses_the_exit_code() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        running(&mut manager, &id);
        manager.push_log(&id, "ssh -N -v root@100.64.1.2").unwrap();
        manager
            .push_log(&id, "debug1: Connecting to 100.64.1.2 port 22.")
            .unwrap();
        assert!(manager.mark_exit(&id, false, Some(255)));
        assert_eq!(
            manager.snapshot().tunnels[0].message.as_deref(),
            Some("进程已退出，退出码 255")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn verbose_summary_lines_do_not_become_the_reason() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        running(&mut manager, &id);
        for line in [
            "OpenSSH_9.9p2, LibreSSL 3.3.6",
            "Authenticated to 100.64.1.2 ([100.64.1.2]:22) using \"password\".",
            "Connection to 100.64.1.2 port 22 timed out",
            "Transferred: sent 1232, received 1424 bytes, in 45.1 seconds",
            "Bytes per second: sent 27.3, received 31.6",
        ] {
            manager.push_log(&id, line).unwrap();
        }
        assert!(manager.mark_exit(&id, false, Some(255)));
        assert_eq!(
            manager.snapshot().tunnels[0].message.as_deref(),
            Some("Connection to 100.64.1.2 port 22 timed out")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_second_start_is_refused_before_the_process_is_attached() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        manager.prepare_start(&id).unwrap();
        assert_eq!(
            manager.snapshot().tunnels[0].status,
            TunnelStatus::Connecting
        );
        assert_eq!(
            manager.prepare_start(&id).err().as_deref(),
            Some("这条隧道正在连接。")
        );
        manager.fail_start(&id, &dir.join(format!("pw-{id}")), "无法启动 ssh".into());
        assert_eq!(manager.snapshot().tunnels[0].status, TunnelStatus::Failed);
        manager.prepare_start(&id).unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn port_note_is_dropped_once_the_tunnel_restarts() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        running(&mut manager, &id);
        manager
            .push_log(&id, "bind [127.0.0.1]:8080: Address already in use")
            .unwrap();
        assert!(manager.mark_exit(&id, false, Some(255)));
        let token = manager.log_token(&id).unwrap();
        manager.prepare_start(&id).unwrap();
        assert!(!manager.set_failure_message(
            &id,
            "端口 8080 已被 ssh（pid 7）占用。".into(),
            token
        ));
        assert!(manager.snapshot().tunnels[0].message.is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn address_in_use_lists_local_ports_after_failure() {
        let (dir, mut manager) = loaded();
        let mut item = draft();
        item.forwards.push(ForwardDraft {
            local_port: 9090,
            remote_host: "127.0.0.1".into(),
            remote_port: 90,
        });
        let id = manager.upsert(item).unwrap();
        running(&mut manager, &id);
        manager
            .push_log(&id, "bind [127.0.0.1]:8080: Address already in use")
            .unwrap();
        manager
            .push_log(&id, "Could not request local forwarding.")
            .unwrap();
        assert!(manager.address_in_use_ports(&id).is_empty());
        assert!(manager.mark_exit(&id, false, Some(255)));
        assert_eq!(manager.address_in_use_ports(&id), vec![8080]);
        assert_eq!(
            manager.snapshot().tunnels[0].message.as_deref(),
            Some("Could not request local forwarding.")
        );
        manager
            .push_log(&id, "channel setup: Address already in use")
            .unwrap();
        assert_eq!(manager.address_in_use_ports(&id), vec![8080]);
        assert!(manager.set_failure_message(
            &id,
            "端口 8080 已被 ssh（pid 7）占用。".into(),
            manager.log_token(&id).unwrap(),
        ));
        assert_eq!(
            manager.snapshot().tunnels[0].message.as_deref(),
            Some("端口 8080 已被 ssh（pid 7）占用。")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unnamed_address_in_use_checks_every_local_port() {
        let (dir, mut manager) = loaded();
        let mut item = draft();
        item.forwards.push(ForwardDraft {
            local_port: 9090,
            remote_host: "127.0.0.1".into(),
            remote_port: 90,
        });
        let id = manager.upsert(item).unwrap();
        running(&mut manager, &id);
        manager
            .push_log(&id, "channel setup: Address already in use")
            .unwrap();
        assert!(manager.mark_exit(&id, false, Some(255)));
        assert_eq!(manager.address_in_use_ports(&id), vec![8080, 9090]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stopped_tunnel_does_not_report_address_in_use() {
        let (dir, mut manager) = loaded();
        let id = manager.upsert(draft()).unwrap();
        running(&mut manager, &id);
        manager
            .push_log(&id, "bind [127.0.0.1]:8080: Address already in use")
            .unwrap();
        assert!(manager.mark_exit(&id, true, Some(0)));
        assert!(manager.address_in_use_ports(&id).is_empty());
        assert!(!manager.set_failure_message(
            &id,
            "不应写上".into(),
            manager.log_token(&id).unwrap(),
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn lsof_parser_keeps_only_listeners_and_formats_the_reason() {
        let output = "\
p4242
cssh
f5
TST=LISTEN
p99
cnode
f3
TST=ESTABLISHED
p4242
cssh
f6
TST=LISTEN
";
        assert_eq!(
            super::parse_lsof(output),
            vec![super::Listener {
                command: "ssh".into(),
                pid: 4242,
            }]
        );
        assert_eq!(
            super::conflict_sentence(8080, &super::parse_lsof(output)).as_deref(),
            Some("端口 8080 已被 ssh（pid 4242）占用。")
        );
        let clients = "p99\ncnode\nT\nTST=ESTABLISHED\np100\ncunknown\nT\nTST=ESTABLISHED\n";
        assert!(super::parse_lsof(clients).is_empty());
        let two = [
            super::Listener {
                command: "node".into(),
                pid: 99,
            },
            super::Listener {
                command: "未知进程".into(),
                pid: 100,
            },
        ];
        assert_eq!(
            super::conflict_sentence(8080, &two).as_deref(),
            Some("端口 8080 已被 node（pid 99）、未知进程（pid 100）占用。")
        );
        assert!(super::conflict_sentence(8080, &[]).is_none());
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

#[cfg(test)]
mod askpass_tests {
    use super::*;
    use std::process::Command;

    fn answer(dir: &Path, prompt: &str, with_jump: bool) -> String {
        let script = dir.join("askpass.sh");
        fs::write(&script, ASKPASS_SCRIPT).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .arg(&script)
            .arg(prompt)
            .env("TAILTUNNEL_PW_FILE", dir.join("pw-a"));
        if with_jump {
            command
                .env("TAILTUNNEL_JUMP_PROMPT", "hop@jump.example.com's password")
                .env("TAILTUNNEL_JUMP_PW_FILE", dir.join("pw-a-jump"));
        }
        String::from_utf8(command.output().unwrap().stdout).unwrap()
    }

    #[test]
    fn askpass_picks_the_password_for_the_hop_being_asked() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("tailtunnel-askpass-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pw-a"), "target-pass").unwrap();
        fs::write(dir.join("pw-a-jump"), "jump-pass").unwrap();
        assert_eq!(
            answer(&dir, "hop@jump.example.com's password: ", true),
            "jump-pass"
        );
        assert_eq!(
            answer(&dir, "root@10.0.0.5's password: ", true),
            "target-pass"
        );
        assert_eq!(answer(&dir, "root@10.0.0.5's password: ", false), "target-pass");
        // Without a jump configured, an empty prefix must not match everything.
        assert_eq!(answer(&dir, "anything", false), "target-pass");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn jump_password_file_sits_next_to_the_target_one() {
        assert_eq!(
            jump_sibling(Path::new("/x/pw-abc")),
            PathBuf::from("/x/pw-abc-jump")
        );
    }
}
