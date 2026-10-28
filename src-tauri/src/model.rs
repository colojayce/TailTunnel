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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JumpHost {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump: Option<JumpHost>,
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
    pub jump: Option<JumpHost>,
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
pub struct JumpDraft {
    pub host: String,
    pub port: u32,
    pub user: String,
    pub password: String,
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
    #[serde(default)]
    pub jump: Option<JumpDraft>,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        jump: &'a Option<JumpHost>,
        forwards: &'a [Forward],
    }
    serde_json::to_string_pretty(&Document {
        note: &tunnel.note,
        host: &tunnel.host,
        port: tunnel.port,
        user: &tunnel.user,
        password: &tunnel.password,
        jump: &tunnel.jump,
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
    check_login(&host, &user, "")?;
    let port = check_port(draft.port, "SSH 端口")?;
    check_password(&draft.password, "")?;
    let jump = draft.jump.as_ref().map(validate_jump).transpose()?;
    if jump.is_some() && !host.chars().all(is_plain_host_char) {
        // With a jump host the target is substituted into a shell command (%h).
        return Err("经跳板连接时，主机只能包含字母、数字和 . - _ : %。".into());
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
        jump,
        forwards,
    })
}

fn is_plain_host_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':' | '%')
}

fn validate_jump(draft: &JumpDraft) -> Result<JumpHost, String> {
    let host = draft.host.trim().to_string();
    let user = draft.user.trim().to_string();
    check_login(&host, &user, "跳板")?;
    check_password(&draft.password, "跳板")?;
    Ok(JumpHost {
        host,
        port: check_port(draft.port, "跳板 SSH 端口")?,
        user,
        password: draft.password.clone(),
    })
}

fn check_login(host: &str, user: &str, label: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err(format!("填写{label}主机。"));
    }
    if host.contains(char::is_whitespace) || host.contains('@') {
        return Err(format!("{label}主机不能包含空格或 @。"));
    }
    if user.is_empty() {
        return Err(format!("填写{label}用户。"));
    }
    if user.contains(char::is_whitespace) || user.contains('@') {
        return Err(format!("{label}用户不能包含空格或 @。"));
    }
    Ok(())
}

fn check_password(password: &str, label: &str) -> Result<(), String> {
    if password.contains(['\n', '\r']) {
        return Err(format!("{label}密码不能换行。"));
    }
    Ok(())
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

/// Options shared by the target connection and the jump hop. The jump hop is a
/// separate `ssh` process that does not inherit `-o` flags from the main one.
fn connection_options(known_hosts: &Path) -> Vec<String> {
    let mut options = Vec::new();
    for option in [
        "StrictHostKeyChecking=accept-new".to_string(),
        "NumberOfPasswordPrompts=1".into(),
        "ConnectTimeout=10".into(),
        "ServerAliveInterval=15".into(),
        "ServerAliveCountMax=3".into(),
        format!("UserKnownHostsFile={}", known_hosts.display()),
        "GlobalKnownHostsFile=/dev/null".into(),
        "PreferredAuthentications=password".into(),
        "PubkeyAuthentication=no".into(),
    ] {
        options.push("-o".to_string());
        options.push(option);
    }
    options.push("-F".into());
    options.push("/dev/null".into());
    options
}

/// The text `ssh` puts in front of its password prompt for this jump host,
/// which the askpass helper uses to tell the two hops apart.
pub fn jump_prompt(jump: &JumpHost) -> String {
    format!("{}@{}'s password", jump.user, jump.host)
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// `ProxyCommand` equivalent to `-J user@host:port`, with the same hardening
/// options as the main connection (plain `-J` would drop them for the jump hop).
fn jump_proxy_command(jump: &JumpHost, known_hosts: &Path) -> String {
    let mut parts = vec![
        "ssh".to_string(),
        "-W".into(),
        "%h:%p".into(),
        "-p".into(),
        jump.port.to_string(),
    ];
    parts.extend(connection_options(known_hosts));
    parts.push(format!("{}@{}", jump.user, jump.host));
    parts
        .iter()
        .map(|part| {
            if part == "%h:%p" {
                part.clone()
            } else {
                shell_quote(&part.replace('%', "%%"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn ssh_args(tunnel: &Tunnel, known_hosts: &Path) -> Vec<String> {
    let mut args = vec![
        "-N".into(),
        "-v".into(),
        "-p".into(),
        tunnel.port.to_string(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
    ];
    args.extend(connection_options(known_hosts));
    if let Some(jump) = &tunnel.jump {
        args.push("-o".into());
        args.push(format!(
            "ProxyCommand={}",
            jump_proxy_command(jump, known_hosts)
        ));
    }
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

fn bracketed(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn jump_target(jump: &JumpHost) -> String {
    format!("{}@{}", jump.user, jump.host)
}

/// A control master to the jump host, used by the latency probe to open
/// channels to the target without paying for a new login each time.
pub fn probe_master_args(jump: &JumpHost, known_hosts: &Path, socket: &Path) -> Vec<String> {
    let mut args = vec![
        "-N".into(),
        "-p".into(),
        jump.port.to_string(),
        "-o".into(),
        "ControlMaster=yes".into(),
        "-o".into(),
        format!("ControlPath={}", socket.display()),
    ];
    args.extend(connection_options(known_hosts));
    args.push(jump_target(jump));
    args
}

/// A client of the control master. `operation` is `-O check`/`-O exit`, or `None`
/// with `forward_to` set for `-W host:port`.
pub fn probe_client_args(
    jump: &JumpHost,
    socket: &Path,
    operation: Option<&str>,
    forward_to: Option<(&str, u16)>,
) -> Vec<String> {
    let mut args = vec![
        "-F".into(),
        "/dev/null".into(),
        "-o".into(),
        "ControlMaster=no".into(),
        "-o".into(),
        format!("ControlPath={}", socket.display()),
        "-p".into(),
        jump.port.to_string(),
    ];
    if let Some(operation) = operation {
        args.push("-O".into());
        args.push(operation.into());
    }
    if let Some((host, port)) = forward_to {
        args.push("-W".into());
        args.push(format!("{}:{port}", bracketed(host)));
    }
    args.push(jump_target(jump));
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
            jump: None,
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

    fn jump_draft() -> JumpDraft {
        JumpDraft {
            host: " jump.example.com ".into(),
            port: 2200,
            user: "hop".into(),
            password: "j-pass".into(),
        }
    }

    #[test]
    fn no_jump_means_no_proxy_command() {
        let tunnel = validate_draft(&sample_draft()).unwrap();
        assert!(tunnel.jump.is_none());
        let args = ssh_args(&tunnel, Path::new("/tmp/known_hosts"));
        assert!(!args.iter().any(|arg| arg.starts_with("ProxyCommand")));
    }

    #[test]
    fn jump_rejects_a_target_host_with_shell_characters() {
        let mut draft = sample_draft();
        draft.jump = Some(jump_draft());
        draft.host = "10.0.0.5;id".into();
        assert!(validate_draft(&draft).is_err());
        draft.jump = None;
        assert!(validate_draft(&draft).is_ok());
    }

    #[test]
    fn jump_adds_proxy_command_with_same_hardening() {
        let mut draft = sample_draft();
        draft.jump = Some(jump_draft());
        let tunnel = validate_draft(&draft).unwrap();
        let jump = tunnel.jump.clone().unwrap();
        assert_eq!(jump.host, "jump.example.com");
        assert_eq!(jump_prompt(&jump), "hop@jump.example.com's password");
        let args = ssh_args(&tunnel, Path::new("/Users/a b/known_hosts"));
        let proxy = args
            .iter()
            .find_map(|arg| arg.strip_prefix("ProxyCommand="))
            .expect("proxy command");
        assert!(proxy.starts_with("'ssh' '-W' %h:%p '-p' '2200'"));
        assert!(proxy.contains("'UserKnownHostsFile=/Users/a b/known_hosts'"));
        assert!(proxy.contains("'PubkeyAuthentication=no'"));
        assert!(proxy.ends_with("'hop@jump.example.com'"));
        assert!(!display_command(&args).contains("j-pass"));
        assert!(display_command(&args).ends_with("root@100.68.115.34"));
    }

    #[test]
    fn jump_is_validated_and_roundtrips() {
        let mut draft = sample_draft();
        let mut bad = jump_draft();
        bad.user = "a@b".into();
        draft.jump = Some(bad);
        assert!(validate_draft(&draft).err().unwrap().contains("跳板"));
        let mut bad_port = jump_draft();
        bad_port.port = 0;
        draft.jump = Some(bad_port);
        assert!(validate_draft(&draft).is_err());

        draft.jump = Some(jump_draft());
        let mut tunnel = validate_draft(&draft).unwrap();
        tunnel.id = "a".into();
        let text = serde_json::to_string(&FileStore {
            tunnels: vec![tunnel],
        })
        .unwrap();
        let back: FileStore = serde_json::from_str(&text).unwrap();
        assert_eq!(back.tunnels[0].jump.as_ref().unwrap().password, "j-pass");
        // Old config files without "jump" still load.
        let old = text.replace(",\"jump\"", ",\"_x\"");
        assert!(serde_json::from_str::<FileStore>(&old).is_ok());
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
