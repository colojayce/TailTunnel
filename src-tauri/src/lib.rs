mod manager;
mod model;
mod tailscale;

use manager::{signal_pid, Running, TunnelManager};
use model::{write_private_atomic, Snapshot, TunnelDraft};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

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
    let _ = app.emit("tunnels-changed", &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn delete_tunnel(
    app: AppHandle,
    state: State<AppState>,
    id: String,
) -> Result<Snapshot, String> {
    let pid = {
        let mut manager = lock(&state);
        manager.delete(&id)?
    };
    if let Some(pid) = pid {
        signal_pid(pid, libc::SIGKILL);
    }
    let snapshot = lock(&state).snapshot();
    let _ = app.emit("tunnels-changed", &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn select_tunnel(app: AppHandle, state: State<AppState>, id: String) -> Result<Snapshot, String> {
    let snapshot = {
        let mut manager = lock(&state);
        manager.select(Some(id))?;
        manager.snapshot()
    };
    let _ = app.emit("tunnels-changed", &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn set_config_path(app: AppHandle, state: State<AppState>, path: String) -> Result<Snapshot, String> {
    let snapshot = {
        let mut manager = lock(&state);
        manager.set_tunnel_file(std::path::PathBuf::from(path))?;
        manager.snapshot()
    };
    let _ = app.emit("tunnels-changed", &snapshot);
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
fn start_tunnel(app: AppHandle, state: State<AppState>, id: String) -> Result<Snapshot, String> {
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
            if let Some(stdout) = stdout {
                pump_output(app.clone(), plan.id.clone(), stdout);
            }
            if let Some(stderr) = stderr {
                pump_output(app.clone(), plan.id.clone(), stderr);
            }
            let app_wait = app.clone();
            let id_wait = plan.id.clone();
            let stop_requested = plan.stop_requested.clone();
            thread::spawn(move || {
                let code = child.wait().ok().and_then(|status| status.code());
                let intentional = stop_requested.load(std::sync::atomic::Ordering::SeqCst);
                let changed = {
                    let state = app_wait.state::<AppState>();
                    let mut manager = state.0.lock().expect("隧道状态锁已损坏");
                    if manager.is_shutting_down() {
                        return;
                    }
                    manager.mark_exit(&id_wait, intentional, code)
                };
                if changed {
                    let snapshot = app_wait.state::<AppState>().0.lock().expect("隧道状态锁已损坏").snapshot();
                    let _ = app_wait.emit("tunnels-changed", &snapshot);
                }
            });
            let snapshot = lock(&state).snapshot();
            let _ = app.emit("tunnels-changed", &snapshot);
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
            let _ = app.emit("tunnels-changed", &snapshot);
            Err(message)
        }
    }
}

#[tauri::command]
fn stop_tunnel(state: State<AppState>, id: String) -> Result<Snapshot, String> {
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

fn pump_output<R>(app: AppHandle, id: String, stream: R)
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
                let _ = app.emit("tunnels-changed", &snapshot);
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            app.manage(AppState(Mutex::new(TunnelManager::load(dir)?)));
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { .. } = event {
                kill_ssh_children(window.app_handle());
                window.app_handle().exit(0);
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
        });
}
