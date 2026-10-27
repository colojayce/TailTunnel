mod manager;
mod model;
mod tailscale;

use manager::{Listener, Running, TunnelManager, conflict_sentence, parse_lsof, signal_pid};
use model::{Snapshot, TunnelDraft, TunnelStatus, TunnelView, write_private_atomic};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

const TRAY_ID: &str = "main";

struct AppState(Mutex<TunnelManager>);

fn kill_ssh_children(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let pids = state.0.lock().expect("隧道状态锁已损坏").stop_all();
    for pid in pids {
        signal_pid(pid, libc::SIGKILL);
    }
}

fn lock<'a>(state: &'a State<'a, AppState>) -> MutexGuard<'a, TunnelManager> {
    state.0.lock().expect("隧道状态锁已损坏")
}

#[tauri::command(async)]
fn tailscale_status() -> tailscale::TailscaleReport {
    tailscale::inspect()
}

#[tauri::command]
fn list_tunnels(state: State<AppState>) -> Snapshot {
    lock(&state).snapshot()
}

#[tauri::command]
fn save_tunnel(
    app: AppHandle,
    state: State<AppState>,
    draft: TunnelDraft,
) -> Result<Snapshot, String> {
    let snapshot = {
        let mut manager = lock(&state);
        manager.upsert(draft)?;
        manager.snapshot()
    };
    publish(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn delete_tunnel(app: AppHandle, state: State<AppState>, id: String) -> Result<Snapshot, String> {
    let pid = {
        let mut manager = lock(&state);
        manager.delete(&id)?
    };
    if let Some(pid) = pid {
        signal_pid(pid, libc::SIGKILL);
    }
    let snapshot = lock(&state).snapshot();
    publish(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn select_tunnel(app: AppHandle, state: State<AppState>, id: String) -> Result<Snapshot, String> {
    let snapshot = {
        let mut manager = lock(&state);
        manager.select(Some(id))?;
        manager.snapshot()
    };
    publish(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn set_config_path(
    app: AppHandle,
    state: State<AppState>,
    path: String,
) -> Result<Snapshot, String> {
    let snapshot = {
        let mut manager = lock(&state);
        manager.set_tunnel_file(std::path::PathBuf::from(path))?;
        manager.snapshot()
    };
    publish(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn tunnel_log(state: State<AppState>, id: String) -> model::LogPage {
    lock(&state).log_of(&id)
}

#[tauri::command]
fn export_tunnel(state: State<AppState>, id: String, path: String) -> Result<(), String> {
    let text = lock(&state).export_text(&id)?;
    write_private_atomic(std::path::Path::new(&path), &text)
}

#[tauri::command]
fn open_local(port: u16) -> Result<(), String> {
    if port == 0 {
        return Err("端口无效。".into());
    }
    let url = format!("http://localhost:{port}");
    let status = std::process::Command::new("open")
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("无法打开浏览器：{error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("无法打开浏览器。".into())
    }
}

#[tauri::command]
fn start_tunnel(app: AppHandle, id: String) -> Result<Snapshot, String> {
    let state = app.state::<AppState>();
    let plan = {
        let mut manager = lock(&state);
        manager.prepare_start(&id)?
    };
    let mut command = Command::new("ssh");
    command
        .args(&plan.args)
        .env("SSH_ASKPASS", &plan.askpass)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", ":0")
        .env("TAILTUNNEL_PW_FILE", &plan.pw_file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match command.spawn() {
        Ok(mut child) => {
            let pid = child.id();
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let update = {
                let mut manager = lock(&state);
                let update = manager.push_log(&plan.id, &plan.command);
                manager.attach(
                    &plan.id,
                    Running {
                        pid,
                        stop_requested: plan.stop_requested.clone(),
                        pw_file: plan.pw_file.clone(),
                    },
                );
                update
            };
            if let Some(update) = update {
                emit_log(&app, &plan.id, update.seq, &plan.command);
            }
            let stdout_task =
                stdout.map(|stream| pump_output(app.clone(), plan.id.clone(), stream));
            let stderr_task =
                stderr.map(|stream| pump_output(app.clone(), plan.id.clone(), stream));
            let app_wait = app.clone();
            let id_wait = plan.id.clone();
            let stop_requested = plan.stop_requested.clone();
            thread::spawn(move || {
                let code = child.wait().ok().and_then(|status| status.code());
                if let Some(task) = stdout_task {
                    let _ = task.join();
                }
                if let Some(task) = stderr_task {
                    let _ = task.join();
                }
                let intentional = stop_requested.load(std::sync::atomic::Ordering::SeqCst);
                finish_tunnel(&app_wait, &id_wait, intentional, code);
            });
            let snapshot = lock(&state).snapshot();
            publish(&app, &snapshot);
            Ok(snapshot)
        }
        Err(error) => {
            let message = if error.kind() == std::io::ErrorKind::NotFound {
                "找不到 ssh。这台 Mac 上需要有 OpenSSH。".into()
            } else {
                format!("无法启动 ssh：{error}")
            };
            let (seq, snapshot) = {
                let mut manager = lock(&state);
                let seq = manager.fail_start(&id, &plan.pw_file, message.clone());
                (seq, manager.snapshot())
            };
            if let Some(seq) = seq {
                emit_log(&app, &id, seq, &message);
            }
            publish(&app, &snapshot);
            Err(message)
        }
    }
}

#[tauri::command]
fn stop_tunnel(app: AppHandle, id: String) -> Result<Snapshot, String> {
    let state = app.state::<AppState>();
    let pid = lock(&state).request_stop(&id);
    if let Some(pid) = pid {
        signal_pid(pid, libc::SIGTERM);
    }
    Ok(lock(&state).snapshot())
}

#[derive(Clone, serde::Serialize)]
struct LogLine {
    id: String,
    seq: u64,
    line: String,
}

fn finish_tunnel(app: &AppHandle, id: &str, intentional: bool, code: Option<i32>) {
    let (changed, ports, token) = {
        let state = app.state::<AppState>();
        let mut manager = state.0.lock().expect("隧道状态锁已损坏");
        if manager.is_shutting_down() {
            return;
        }
        let changed = manager.mark_exit(id, intentional, code);
        let ports = manager.address_in_use_ports(id);
        let token = manager.log_token(id);
        (changed, ports, token)
    };
    if changed {
        let snapshot = app
            .state::<AppState>()
            .0
            .lock()
            .expect("隧道状态锁已损坏")
            .snapshot();
        publish(app, &snapshot);
    }
    let Some(note) = describe_port_conflict(&ports) else {
        return;
    };
    let Some(token) = token else {
        return;
    };
    let annotated = {
        let state = app.state::<AppState>();
        let mut manager = state.0.lock().expect("隧道状态锁已损坏");
        manager.set_failure_message(id, note, token)
    };
    if annotated {
        let snapshot = app
            .state::<AppState>()
            .0
            .lock()
            .expect("隧道状态锁已损坏")
            .snapshot();
        publish(app, &snapshot);
    }
}

fn describe_port_conflict(ports: &[u16]) -> Option<String> {
    let mut parts = Vec::new();
    for port in ports {
        if let Some(sentence) = conflict_sentence(*port, &listeners_on(*port)) {
            parts.push(sentence);
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.concat())
    }
}

fn listeners_on(port: u16) -> Vec<Listener> {
    let mut command = Command::new("lsof");
    command
        .args(["-nP", "-FpcT", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return Vec::new(),
    };
    let stdout = child.stdout.take();
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut stdout) = stdout {
            let _ = stdout.read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() >= Duration::from_secs(3) => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    parse_lsof(&reader.join().unwrap_or_default())
}

fn publish(app: &AppHandle, snapshot: &Snapshot) {
    let _ = app.emit("tunnels-changed", snapshot);
    refresh_tray(app, snapshot);
}

fn emit_log(app: &AppHandle, id: &str, seq: u64, line: &str) {
    let _ = app.emit(
        "tunnel-log",
        LogLine {
            id: id.to_string(),
            seq,
            line: line.to_string(),
        },
    );
}

fn pump_output<R>(app: AppHandle, id: String, stream: R) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let read = reader.read_until(b'\n', &mut buf);
            let Ok(count) = read else { break };
            if count == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf)
                .trim_end_matches(['\r', '\n'])
                .to_string();
            if line.is_empty() {
                continue;
            }
            let update = {
                let state = app.state::<AppState>();
                let mut manager = state.0.lock().expect("隧道状态锁已损坏");
                if manager.is_shutting_down() {
                    return;
                }
                manager.push_log(&id, &line)
            };
            let Some(update) = update else {
                continue;
            };
            emit_log(&app, &id, update.seq, &line);
            if update.established {
                let snapshot = app
                    .state::<AppState>()
                    .0
                    .lock()
                    .expect("隧道状态锁已损坏")
                    .snapshot();
                publish(&app, &snapshot);
            }
        }
    })
}

fn tray_label(tunnel: &TunnelView) -> String {
    tray_text(
        &tunnel.note,
        &tunnel.user,
        &tunnel.host,
        &tunnel
            .forwards
            .iter()
            .map(|forward| forward.local_port)
            .collect::<Vec<_>>(),
        tunnel.status,
    )
}

pub(crate) fn tray_text(
    note: &str,
    user: &str,
    host: &str,
    ports: &[u16],
    status: TunnelStatus,
) -> String {
    let ports = ports
        .iter()
        .map(|port| port.to_string())
        .collect::<Vec<_>>()
        .join("、");
    let status = match status {
        TunnelStatus::Connecting => "正在连接",
        TunnelStatus::Running => "运行中",
        TunnelStatus::Failed => "连接失败",
        TunnelStatus::Stopped => "已停止",
    };
    let identity = if note.is_empty() {
        format!("{user}@{host}")
    } else {
        note.to_string()
    };
    let text = if ports.is_empty() {
        format!("{identity}  {status}")
    } else {
        format!("{identity} · {ports}  {status}")
    };
    text.replace('&', "&&")
}

fn running_count(snapshot: &Snapshot) -> usize {
    snapshot
        .tunnels
        .iter()
        .filter(|tunnel| tunnel.status == TunnelStatus::Running)
        .count()
}

fn tray_menu(app: &AppHandle, snapshot: &Snapshot) -> tauri::Result<Menu<tauri::Wry>> {
    let show = MenuItem::with_id(app, "show", "打开窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::new(app)?;
    menu.append(&show)?;
    if !snapshot.tunnels.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        let items = snapshot
            .tunnels
            .iter()
            .map(|tunnel| {
                MenuItem::with_id(
                    app,
                    format!("tunnel:{}", tunnel.id),
                    tray_label(tunnel),
                    true,
                    None::<&str>,
                )
            })
            .collect::<tauri::Result<Vec<_>>>()?;
        for item in &items {
            menu.append(item)?;
        }
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&quit)?;
    Ok(menu)
}

fn refresh_tray(app: &AppHandle, snapshot: &Snapshot) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    if let Ok(menu) = tray_menu(app, snapshot) {
        let _ = tray.set_menu(Some(menu));
    }
    let _ = tray.set_title(Some(running_count(snapshot).to_string()));
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn handle_tray_menu(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();
    if id == "show" {
        show_main_window(app);
        return;
    }
    if id == "quit" {
        app.exit(0);
        return;
    }
    let Some(tunnel_id) = id.strip_prefix("tunnel:") else {
        return;
    };
    let tunnel_id = tunnel_id.to_string();
    let app = app.clone();
    thread::spawn(move || {
        let busy = {
            let state = app.state::<AppState>();
            let manager = state.0.lock().expect("隧道状态锁已损坏");
            manager.snapshot().tunnels.into_iter().any(|tunnel| {
                tunnel.id == tunnel_id
                    && (tunnel.status == TunnelStatus::Running
                        || tunnel.status == TunnelStatus::Connecting)
            })
        };
        if busy {
            let _ = stop_tunnel(app, tunnel_id);
        } else {
            let _ = start_tunnel(app, tunnel_id);
        }
    });
}

fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    let snapshot = app
        .state::<AppState>()
        .0
        .lock()
        .expect("隧道状态锁已损坏")
        .snapshot();
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("应用图标".into()))?;
    let menu = tray_menu(app.handle(), &snapshot)?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .tooltip("TailTunnel")
        .title(running_count(&snapshot).to_string())
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| handle_tray_menu(app, event))
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            app.manage(AppState(Mutex::new(TunnelManager::load(dir)?)));
            install_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_tunnels,
            tailscale_status,
            save_tunnel,
            delete_tunnel,
            select_tunnel,
            set_config_path,
            tunnel_log,
            export_tunnel,
            open_local,
            start_tunnel,
            stop_tunnel
        ])
        .build(tauri::generate_context!())
        .expect("启动 TailTunnel 失败")
        .run(|app, event| {
            if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
                kill_ssh_children(app);
            }
            #[cfg(target_os = "macos")]
            if let RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } = event
            {
                show_main_window(app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::tray_text;
    use crate::model::TunnelStatus;

    #[test]
    fn tray_shows_the_note_and_falls_back_to_the_host() {
        assert_eq!(
            tray_text(
                "家里的 NAS",
                "root",
                "100.64.1.2",
                &[3430, 3425],
                TunnelStatus::Running
            ),
            "家里的 NAS · 3430、3425  运行中"
        );
        assert_eq!(
            tray_text("", "root", "100.64.1.2", &[3430], TunnelStatus::Stopped),
            "root@100.64.1.2 · 3430  已停止"
        );
        assert_eq!(
            tray_text("A & B", "root", "100.64.1.2", &[], TunnelStatus::Failed),
            "A && B  连接失败"
        );
    }
}
