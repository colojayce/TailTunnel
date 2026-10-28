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
    /// Dropped, with an automatic reconnect scheduled.
    Waiting,
}

/// How a hop logs in. With `Key`, the stored password is the key's passphrase.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum AuthMethod {
    #[default]
    Password,
    Key,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JumpHost {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    #[serde(default)]
    pub auth: AuthMethod,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key_file: String,
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
    #[serde(default)]
    pub auth: AuthMethod,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump: Option<JumpHost>,
    pub forwards: Vec<Forward>,
    #[serde(default)]
    pub auto_reconnect: bool,
}

impl Tunnel {
    pub fn display_name(&self) -> String {
        if self.note.is_empty() {
            format!("{}@{}", self.user, self.host)
        } else {
            self.note.clone()
        }
    }
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
    pub auth: AuthMethod,
    pub key_file: String,
    pub jump: Option<JumpHost>,
    pub forwards: Vec<Forward>,
    pub auto_reconnect: bool,
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
    pub notify: bool,
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
    #[serde(default)]
    pub auth: AuthMethod,
    #[serde(default)]
    pub key_file: String,
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
    pub auth: AuthMethod,
    #[serde(default)]
    pub key_file: String,
    #[serde(default)]
    pub jump: Option<JumpDraft>,
    pub forwards: Vec<ForwardDraft>,
    #[serde(default)]
    pub auto_reconnect: bool,
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
        auth: AuthMethod,
        #[serde(skip_serializing_if = "str::is_empty")]
        key_file: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        jump: &'a Option<JumpHost>,
        forwards: &'a [Forward],
        auto_reconnect: bool,
    }
    serde_json::to_string_pretty(&Document {
        note: &tunnel.note,
        host: &tunnel.host,
        port: tunnel.port,
        user: &tunnel.user,
        password: &tunnel.password,
        auth: tunnel.auth,
        key_file: &tunnel.key_file,
        jump: &tunnel.jump,
        forwards: &tunnel.forwards,
        auto_reconnect: tunnel.auto_reconnect,
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
    let key_file = check_secret(draft.auth, &draft.key_file, &draft.password, "")?;
    let jump = draft.jump.as_ref().map(validate_jump).transpose()?;
    if jump.is_some() && !host.chars().all(is_plain_host_char) {
        // With a jump host the target is substituted into a shell command (%h).
        return Err("经跳板连接时，主机只能包含字母、数字和 . - _ : %。".into());
    }
    let note = clean_note(&draft.note)?;
    if draft.forwards.is_empty() {
        return Err("至少添加一行本地转发。".into());
    }
    let mut forwards: Vec<Forward> = Vec::with_capacity(draft.forwards.len());
    for (index, forward) in draft.forwards.iter().enumerate() {
        let nth = index + 1;
        let remote_host = forward.remote_host.trim().to_string();
        if remote_host.is_empty() {
            return Err(format!("第 {nth} 行转发要填写远端地址。"));
        }
        if remote_host.contains(char::is_whitespace) || remote_host.contains(':') {
            return Err(format!("第 {nth} 行远端地址不能包含空格或冒号。"));
        }
        let local_port = check_port(forward.local_port, &format!("第 {nth} 行本地端口"))?;
        if forwards.iter().any(|item| item.local_port == local_port) {
            return Err(format!("本地端口 {local_port} 填了不止一次。"));
        }
        forwards.push(Forward {
            local_port,
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
        auth: draft.auth,
        key_file,
        jump,
        forwards,
        auto_reconnect: draft.auto_reconnect,
    })
}

fn is_plain_host_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':' | '%')
}

fn validate_jump(draft: &JumpDraft) -> Result<JumpHost, String> {
    let host = draft.host.trim().to_string();
    let user = draft.user.trim().to_string();
    check_login(&host, &user, "跳板")?;
    let key_file = check_secret(draft.auth, &draft.key_file, &draft.password, "跳板")?;
    Ok(JumpHost {
        host,
        port: check_port(draft.port, "跳板 SSH 端口")?,
        user,
        password: draft.password.clone(),
        auth: draft.auth,
        key_file,
    })
}

/// Checks the password, or for key login the key file and its passphrase.
/// Returns the key file to store: an absolute path, or empty for password login.
fn check_secret(
    auth: AuthMethod,
    key_file: &str,
    secret: &str,
    label: &str,
) -> Result<String, String> {
    match auth {
        AuthMethod::Password => {
            check_password(secret, label)?;
            Ok(String::new())
        }
        AuthMethod::Key => {
            if secret.contains(['\n', '\r']) {
                return Err(format!("{label}私钥口令不能换行。"));
            }
            let raw = key_file.trim();
            if raw.is_empty() {
                return Err(format!("选择{label}私钥文件。"));
            }
            if raw.contains(['\n', '\r']) {
                return Err(format!("{label}私钥文件路径不能换行。"));
            }
            if raw.contains("${") {
                return Err(format!(
                    "{label}私钥文件路径不能包含 ${{，ssh 会把它当成环境变量。"
                ));
            }
            let path = expand_home(raw);
            if !Path::new(&path).is_absolute() {
                return Err(format!("{label}私钥文件需要完整路径。"));
            }
            if !Path::new(&path).is_file() {
                return Err(format!("找不到{label}私钥文件：{path}"));
            }
            Ok(path)
        }
    }
}

fn expand_home(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return path.to_string();
    }
    if path == "~" {
        return home;
    }
    match path.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home.trim_end_matches('/')),
        None => path.to_string(),
    }
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
fn connection_options(known_hosts: &Path, auth: AuthMethod, key_file: &str) -> Vec<String> {
    let mut options = Vec::new();
    let login: &[&str] = match auth {
        AuthMethod::Password => &["PreferredAuthentications=password", "PubkeyAuthentication=no"],
        AuthMethod::Key => &[
            "PreferredAuthentications=publickey",
            "PubkeyAuthentication=yes",
            "IdentitiesOnly=yes",
            "PasswordAuthentication=no",
            "KbdInteractiveAuthentication=no",
        ],
    };
    for option in [
        "StrictHostKeyChecking=accept-new".to_string(),
        "NumberOfPasswordPrompts=1".into(),
        "ConnectTimeout=10".into(),
        "ServerAliveInterval=15".into(),
        "ServerAliveCountMax=3".into(),
        format!("UserKnownHostsFile={}", known_hosts.display()),
        "GlobalKnownHostsFile=/dev/null".into(),
    ]
    .into_iter()
    .chain(login.iter().map(|option| option.to_string()))
    {
        options.push("-o".to_string());
        options.push(option);
    }
    if auth == AuthMethod::Key {
        options.push("-o".into());
        options.push(identity_file_option(key_file));
    }
    options.push("-F".into());
    options.push("/dev/null".into());
    options
}

/// `IdentityFile` for `-o` rather than `-i`: both expand %-tokens, but only
/// `IdentityFile` turns `%%` back into `%`. The quotes keep spaces in the path.
/// ssh has no escape for `${`, so `check_secret` rejects it.
fn identity_file_option(key_file: &str) -> String {
    let escaped = key_file
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("IdentityFile=\"{escaped}\"")
}

/// The start of the prompt `ssh` shows when asking for this hop's secret.
/// ssh prints at most 100 characters of a key path, so the closing quote is
/// only part of the prefix when the whole path fits.
pub fn secret_prompt(auth: AuthMethod, user: &str, host: &str, key_file: &str) -> String {
    match auth {
        AuthMethod::Password => format!("{user}@{host}'s password"),
        AuthMethod::Key => {
            let shown: String = key_file.chars().take(100).collect();
            if key_file.chars().count() > 100 {
                format!("Enter passphrase for key '{shown}")
            } else {
                format!("Enter passphrase for key '{shown}'")
            }
        }
    }
}

/// The prompt prefix for the jump hop, which the askpass helper uses to tell
/// the two hops apart.
pub fn jump_prompt(jump: &JumpHost) -> String {
    secret_prompt(jump.auth, &jump.user, &jump.host, &jump.key_file)
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
    parts.extend(connection_options(known_hosts, jump.auth, &jump.key_file));
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
    args.extend(connection_options(known_hosts, tunnel.auth, &tunnel.key_file));
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
    args.extend(connection_options(known_hosts, jump.auth, &jump.key_file));
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
            auth: AuthMethod::Password,
            key_file: String::new(),
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
            auto_reconnect: false,
        }
    }

    fn temp_key(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("tailtunnel-key-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, "key").unwrap();
        path
    }

    #[test]
    fn duplicate_local_ports_are_rejected() {
        let mut draft = sample_draft();
        draft.forwards[1].local_port = 3430;
        assert_eq!(
            validate_draft(&draft).err().as_deref(),
            Some("本地端口 3430 填了不止一次。")
        );
    }

    #[test]
    fn key_login_uses_the_key_and_no_password() {
        let key = temp_key("id_100%");
        let mut draft = sample_draft();
        draft.auth = AuthMethod::Key;
        draft.key_file = String::new();
        assert_eq!(validate_draft(&draft).err().as_deref(), Some("选择私钥文件。"));
        draft.key_file = "/nope/id_ed25519".into();
        assert!(validate_draft(&draft).err().unwrap().starts_with("找不到私钥文件"));
        draft.key_file = format!(" {} ", key.display());
        draft.auto_reconnect = true;
        let tunnel = validate_draft(&draft).unwrap();
        assert_eq!(tunnel.key_file, key.display().to_string());
        assert!(tunnel.auto_reconnect);
        let args = ssh_args(&tunnel, Path::new("/tmp/known_hosts"));
        let identity = format!("IdentityFile=\"{}\"", key.display()).replace('%', "%%");
        assert!(args.contains(&identity));
        assert!(!args.iter().any(|arg| arg == "-i"));
        assert!(args.iter().any(|arg| arg == "PreferredAuthentications=publickey"));
        assert!(args.iter().any(|arg| arg == "IdentitiesOnly=yes"));
        assert!(!args.iter().any(|arg| arg == "PubkeyAuthentication=no"));
        let _ = std::fs::remove_dir_all(key.parent().unwrap());
    }

    #[test]
    fn password_login_drops_a_leftover_key_file() {
        let mut draft = sample_draft();
        draft.key_file = "/nope/id_ed25519".into();
        let tunnel = validate_draft(&draft).unwrap();
        assert!(tunnel.key_file.is_empty());
        assert!(
            !ssh_args(&tunnel, Path::new("/tmp/kh"))
                .iter()
                .any(|arg| arg.starts_with("IdentityFile="))
        );
    }

    #[test]
    fn identity_file_option_escapes_quotes_backslashes_and_percent() {
        assert_eq!(
            identity_file_option(r#"/k/my "keys"\100%"#),
            r#"IdentityFile="/k/my \"keys\"\\100%%""#
        );
    }

    #[test]
    fn key_paths_with_an_env_reference_are_rejected() {
        let mut draft = sample_draft();
        draft.auth = AuthMethod::Key;
        draft.key_file = "/k/${HOME}/id".into();
        assert!(validate_draft(&draft).err().unwrap().contains("${"));
    }

    #[test]
    fn key_on_the_jump_goes_into_the_proxy_command_and_prompt() {
        let key = temp_key("jump_100%");
        let mut draft = sample_draft();
        let mut jump = jump_draft();
        jump.auth = AuthMethod::Key;
        jump.key_file = key.display().to_string();
        draft.jump = Some(jump);
        let tunnel = validate_draft(&draft).unwrap();
        let jump = tunnel.jump.clone().unwrap();
        assert_eq!(
            jump_prompt(&jump),
            format!("Enter passphrase for key '{}'", key.display())
        );
        let args = ssh_args(&tunnel, Path::new("/tmp/known_hosts"));
        let proxy = args
            .iter()
            .find_map(|arg| arg.strip_prefix("ProxyCommand="))
            .unwrap();
        // The outer ssh turns `%%%%` into `%%` for the jump ssh, whose
        // IdentityFile then turns it into `%`.
        let identity = format!("'IdentityFile=\"{}\"'", key.display()).replace('%', "%%%%");
        assert!(proxy.contains(&identity));
        assert!(proxy.contains("'PreferredAuthentications=publickey'"));
        // The target still logs in with its password.
        assert!(args.iter().any(|arg| arg == "PreferredAuthentications=password"));
        let _ = std::fs::remove_dir_all(key.parent().unwrap());
    }

    #[test]
    fn long_key_paths_match_the_truncated_prompt() {
        let long = format!("/k/{}", "a".repeat(120));
        let prompt = secret_prompt(AuthMethod::Key, "u", "h", &long);
        assert!(!prompt.ends_with('\''));
        assert_eq!(prompt.chars().count(), "Enter passphrase for key '".len() + 100);
        assert_eq!(
            secret_prompt(AuthMethod::Key, "u", "h", "/k/id"),
            "Enter passphrase for key '/k/id'"
        );
    }

    #[test]
    fn old_files_load_as_password_login_without_reconnect() {
        let text = r#"{"tunnels":[{"id":"a","host":"h","port":22,"user":"u","password":"p","forwards":[]}]}"#;
        let store: FileStore = serde_json::from_str(text).unwrap();
        assert_eq!(store.tunnels[0].auth, AuthMethod::Password);
        assert!(!store.tunnels[0].auto_reconnect);
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
            auth: AuthMethod::Password,
            key_file: String::new(),
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
