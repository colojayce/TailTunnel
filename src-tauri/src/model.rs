use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

pub const MAX_LOG_CHARS: usize = 80_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Forward {
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TunnelStatus {
    Stopped,
    Connecting,
    Running,
    Failed,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tunnel {
    pub id: String,
    #[serde(default)]
    pub note: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub forwards: Vec<Forward>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelView {
    pub id: String,
    pub note: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub forwards: Vec<Forward>,
    pub status: TunnelStatus,
    pub message: Option<String>,
    pub connected_at: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub tunnels: Vec<TunnelView>,
    pub selected_id: Option<String>,
    pub config_path: String,
}

#[derive(Clone, Serialize)]
pub struct LogEntry {
    pub seq: u64,
    pub line: String,
}

#[derive(Clone, Serialize)]
pub struct LogPage {
    pub floor: u64,
    pub lines: Vec<LogEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardDraft {
    pub local_port: u32,
    pub remote_host: String,
    pub remote_port: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelDraft {
    pub id: Option<String>,
    #[serde(default)]
    pub note: String,
    pub host: String,
    pub port: u32,
    pub user: String,
    pub password: String,
    pub forwards: Vec<ForwardDraft>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
struct FileStore {
    tunnels: Vec<Tunnel>,
}

pub fn config_file(dir: &Path) -> std::path::PathBuf {
    dir.join("tunnels.json")
}

pub fn load_store(dir: &Path) -> Result<Vec<Tunnel>, String> {
    load_store_at(&config_file(dir))
}

pub fn load_store_at(path: &Path) -> Result<Vec<Tunnel>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("读配置失败：{e}"))?;
    let store: FileStore =
        serde_json::from_str(&text).map_err(|e| format!("配置文件无法解析：{e}"))?;
    Ok(store.tunnels)
}

pub fn save_store(dir: &Path, tunnels: &[Tunnel]) -> Result<(), String> {
    save_store_at(&config_file(dir), tunnels)
}

pub fn save_store_at(path: &Path, tunnels: &[Tunnel]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&FileStore {
        tunnels: tunnels.to_vec(),
    })
    .map_err(|e| format!("写入配置失败：{e}"))?;
    write_private_atomic(path, &text)
}

pub fn tunnel_export(tunnel: &Tunnel) -> Result<String, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Document<'a> {
        #[serde(skip_serializing_if = "str::is_empty")]
        note: &'a str,
        host: &'a str,
        port: u16,
        user: &'a str,
        password: &'a str,
        forwards: &'a [Forward],
    }
    serde_json::to_string_pretty(&Document {
        note: &tunnel.note,
        host: &tunnel.host,
        port: tunnel.port,
        user: &tunnel.user,
        password: &tunnel.password,
        forwards: &tunnel.forwards,
    })
    .map_err(|e| format!("导出失败：{e}"))
}

pub fn write_private_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
        }
    }
    let tmp = beside_temp(path);
    let _ = std::fs::remove_file(&tmp);
    let write_result = (|| -> Result<(), String> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|e| format!("写入配置失败：{e}"))?;
        file.write_all(text.as_bytes())
            .map_err(|e| format!("写入配置失败：{e}"))?;
        file.sync_all().map_err(|e| format!("写入配置失败：{e}"))?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入配置失败：{error}"));
    }
    set_private(path);
    Ok(())
}

fn beside_temp(path: &Path) -> std::path::PathBuf {
    let mut file_name = path.file_name().unwrap_or_default().to_os_string();
    file_name.push(".tmp");
    path.with_file_name(file_name)
}

pub fn set_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

pub fn validate_draft(draft: &TunnelDraft) -> Result<Tunnel, String> {
    let host = draft.host.trim().to_string();
    let user = draft.user.trim().to_string();
    if host.is_empty() {
        return Err("填写主机。".into());
    }
    if host.contains(char::is_whitespace) || host.contains('@') {
        return Err("主机不能包含空格或 @。".into());
    }
    if user.is_empty() {
        return Err("填写用户。".into());
    }
    if user.contains(char::is_whitespace) || user.contains('@') {
        return Err("用户不能包含空格或 @。".into());
    }
    let port = check_port(draft.port, "SSH 端口")?;
    if draft.password.contains(['\n', '\r']) {
        return Err("密码不能换行。".into());
    }
    let note = clean_note(&draft.note)?;
    if draft.forwards.is_empty() {
        return Err("至少添加一行本地转发。".into());
    }
    let mut forwards = Vec::with_capacity(draft.forwards.len());
    for (index, forward) in draft.forwards.iter().enumerate() {
        let nth = index + 1;
        let remote_host = forward.remote_host.trim().to_string();
        if remote_host.is_empty() {
            return Err(format!("第 {nth} 行转发要填写远端地址。"));
        }
        if remote_host.contains(char::is_whitespace) || remote_host.contains(':') {
            return Err(format!("第 {nth} 行远端地址不能包含空格或冒号。"));
        }
        forwards.push(Forward {
            local_port: check_port(forward.local_port, &format!("第 {nth} 行本地端口"))?,
            remote_host,
            remote_port: check_port(forward.remote_port, &format!("第 {nth} 行远端端口"))?,
        });
    }
    Ok(Tunnel {
        id: String::new(),
        note,
        host,
        port,
        user,
        password: draft.password.clone(),
        forwards,
    })
}

fn clean_note(raw: &str) -> Result<String, String> {
    let note = raw
        .chars()
        .filter(|ch| *ch != '\u{200b}' && *ch != '\u{feff}')
        .collect::<String>();
    let note = note.trim().to_string();
    if note.chars().count() > 40 {
        return Err("备注最多 40 个字。".into());
    }
    if note.contains(['\n', '\r']) {
        return Err("备注不能换行。".into());
    }
    Ok(note)
}

fn check_port(value: u32, label: &str) -> Result<u16, String> {
    u16::try_from(value)
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| format!("{label}要在 1 到 65535 之间。"))
}

pub fn ssh_args(tunnel: &Tunnel, known_hosts: &Path) -> Vec<String> {
    let mut args = vec![
        "-N".into(),
        "-v".into(),
        "-p".into(),
        tunnel.port.to_string(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "NumberOfPasswordPrompts=1".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
        "-o".into(),
        "ServerAliveInterval=15".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-o".into(),
        format!("UserKnownHostsFile={}", known_hosts.display()),
        "-o".into(),
        "GlobalKnownHostsFile=/dev/null".into(),
        "-o".into(),
        "PreferredAuthentications=password".into(),
        "-o".into(),
        "PubkeyAuthentication=no".into(),
        "-F".into(),
        "/dev/null".into(),
    ];
    for forward in &tunnel.forwards {
        args.push("-L".into());
        args.push(format!(
            "{}:{}:{}",
            forward.local_port, forward.remote_host, forward.remote_port
        ));
    }
    args.push(format!("{}@{}", tunnel.user, tunnel.host));
    args
}

pub fn display_command(args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push("ssh".to_string());
    parts.extend(args.iter().cloned());
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_draft() -> TunnelDraft {
        TunnelDraft {
            id: None,
            note: String::new(),
            host: " 100.68.115.34 ".into(),
            port: 2225,
            user: "root".into(),
            password: "s3cret".into(),
            forwards: vec![
                ForwardDraft {
                    local_port: 3430,
                    remote_host: "127.0.0.1".into(),
                    remote_port: 3430,
                },
                ForwardDraft {
                    local_port: 3425,
                    remote_host: "127.0.0.1".into(),
                    remote_port: 3425,
                },
            ],
        }
    }

    #[test]
    fn command_matches_local_forwards_and_hides_password() {
        let tunnel = validate_draft(&sample_draft()).unwrap();
        let args = ssh_args(&tunnel, Path::new("/tmp/known_hosts"));
        let rendered = display_command(&args);
        assert!(!rendered.contains("s3cret"));
        assert!(rendered.contains("-L 3430:127.0.0.1:3430"));
        assert!(rendered.contains("-L 3425:127.0.0.1:3425"));
        assert!(rendered.ends_with("root@100.68.115.34"));
        assert!(args.iter().any(|arg| arg == "-p"));
        assert!(args.iter().any(|arg| arg == "2225"));
    }

    #[test]
    fn rejects_missing_forward() {
        let mut draft = sample_draft();
        draft.forwards.clear();
        assert!(validate_draft(&draft).is_err());
    }

    #[test]
    fn roundtrip_keeps_password() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("tailtunnel-test-{nanos}"));
        let mut tunnel = validate_draft(&sample_draft()).unwrap();
        tunnel.id = "a".into();
        save_store(&dir, &[tunnel.clone()]).unwrap();
        let loaded = load_store(&dir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].password, "s3cret");
        assert_eq!(loaded[0].forwards.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_replaces_a_broken_file_and_leaves_no_temp() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("tailtunnel-atomic-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = config_file(&dir);
        std::fs::write(&path, "{").unwrap();
        let mut tunnel = validate_draft(&sample_draft()).unwrap();
        tunnel.id = "a".into();
        save_store_at(&path, &[tunnel]).unwrap();
        assert!(!beside_temp(&path).exists());
        let loaded = load_store_at(&path).unwrap();
        assert_eq!(loaded[0].password, "s3cret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn export_document_is_one_tunnel() {
        let tunnel = validate_draft(&sample_draft()).unwrap();
        let text = tunnel_export(&tunnel).unwrap();
        assert!(text.contains("\"host\": \"100.68.115.34\""));
        assert!(text.contains("\"password\": \"s3cret\""));
        assert!(text.contains("\"localPort\": 3430"));
        assert!(!text.contains("\"id\""));
        assert!(!text.contains("\"status\""));
    }
}
