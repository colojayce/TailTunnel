//! Three-way merge between this Mac's tunnels and the newest copy another Mac
//! put in iCloud.
//!
//! The common ancestor is not stored as a config; only a fingerprint of each
//! tunnel from the last copy this Mac merged is kept (`base`). That is enough
//! to tell which side changed what:
//!
//! - only one side changed a tunnel: that side wins;
//! - both changed the forwarding settings: the newer save wins, so every Mac
//!   comes to the same answer and the copies converge;
//! - both changed the login: this Mac keeps its own and reports it, because a
//!   password or key path belongs to the machine it was typed on;
//! - the other Mac deleted a tunnel: it goes here too, unless this Mac changed
//!   it since or it is running, in which case it stays and is reported;
//! - a tunnel this Mac never synced is never deleted.

use crate::model::{AuthMethod, Tunnel};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

/// User-facing names of the login fields a conflict can involve.
pub const FIELD_PASSWORD: &str = "密码";
pub const FIELD_AUTH: &str = "登录方式";
pub const FIELD_KEY_FILE: &str = "私钥";
pub const FIELD_JUMP_PASSWORD: &str = "跳板机密码";
pub const FIELD_JUMP_AUTH: &str = "跳板机登录方式";
pub const FIELD_JUMP_KEY_FILE: &str = "跳板机私钥";

/// Fingerprints of a tunnel's two halves. They are kept only in this Mac's
/// private settings file, next to the plaintext config they are taken from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Print {
    pub settings: String,
    pub login: String,
}

pub type Prints = BTreeMap<String, Print>;

fn digest(value: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    Sha256::digest(&bytes)
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn print_of(tunnel: &Tunnel) -> Print {
    let jump_settings = tunnel.jump.as_ref().map(|jump| {
        serde_json::json!({ "host": jump.host, "port": jump.port, "user": jump.user })
    });
    let settings = serde_json::json!({
        "note": tunnel.note,
        "host": tunnel.host,
        "port": tunnel.port,
        "user": tunnel.user,
        "forwards": tunnel.forwards,
        "autoReconnect": tunnel.auto_reconnect,
        "jump": jump_settings,
    });
    let jump_login = tunnel.jump.as_ref().map(|jump| {
        serde_json::json!({ "password": jump.password, "auth": jump.auth, "keyFile": jump.key_file })
    });
    let login = serde_json::json!({
        "password": tunnel.password,
        "auth": tunnel.auth,
        "keyFile": tunnel.key_file,
        "jump": jump_login,
    });
    Print {
        settings: digest(&settings),
        login: digest(&login),
    }
}

pub fn prints(tunnels: &[Tunnel]) -> Prints {
    tunnels
        .iter()
        .map(|tunnel| (tunnel.id.clone(), print_of(tunnel)))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NoteKind {
    /// Both Macs changed the login; this Mac kept its own.
    LoginKept,
    /// Both Macs changed the forwarding settings; the other Mac's newer save won.
    SettingsReplaced,
    /// The other Mac deleted it, but it changed here or is running, so it stays.
    KeptDeleted,
    /// Deleted here, but the other Mac changed it since, so it came back.
    Revived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncNote {
    pub tunnel_id: String,
    pub tunnel_name: String,
    pub kind: NoteKind,
    /// For `LoginKept`: which login fields differ.
    pub fields: Vec<&'static str>,
}

pub struct MergeInput<'a> {
    /// Fingerprints of the last copy from iCloud this Mac merged.
    pub base: &'a Prints,
    /// Fingerprints of what this Mac last uploaded.
    pub pushed: &'a Prints,
    pub local: &'a [Tunnel],
    pub remote: &'a [Tunnel],
    /// Tunnels that are running, connecting or waiting to reconnect here.
    pub busy: &'a HashSet<String>,
    /// Whether the remote copy was saved after this Mac's last upload.
    pub remote_is_newer: bool,
}

// 不派生 Debug：Tunnel 里有明文 SSH 密码，避免被顺手 {:?} 打进日志。
#[derive(Default)]
pub struct MergeOutcome {
    pub tunnels: Vec<Tunnel>,
    pub notes: Vec<SyncNote>,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

pub fn merge(input: &MergeInput) -> MergeOutcome {
    let mut outcome = MergeOutcome::default();
    let remote_by_id: HashMap<&str, &Tunnel> = input
        .remote
        .iter()
        .map(|tunnel| (tunnel.id.as_str(), tunnel))
        .collect();
    let local_ids: HashSet<&str> = input.local.iter().map(|tunnel| tunnel.id.as_str()).collect();

    for local in input.local {
        let base = input.base.get(&local.id);
        let Some(remote) = remote_by_id.get(local.id.as_str()) else {
            match base {
                // 另一台 Mac 删掉了它，本机也没动过，跟着删。
                Some(base) if *base == print_of(local) && !input.busy.contains(&local.id) => {
                    outcome.removed += 1;
                }
                Some(_) => {
                    outcome.notes.push(note(local, NoteKind::KeptDeleted, Vec::new()));
                    outcome.tunnels.push(local.clone());
                }
                // 本机从没同步过的隧道，永不删除。
                None => outcome.tunnels.push(local.clone()),
            }
            continue;
        };
        // 本机这条改过但还没上传，就是最新的改动。
        let local_newer = match input.pushed.get(&local.id) {
            Some(pushed) => *pushed != print_of(local) || !input.remote_is_newer,
            None => false,
        };
        let merged = merge_one(base, local_newer, local, remote, &mut outcome.notes);
        if print_of(&merged) != print_of(local) {
            outcome.updated += 1;
        }
        outcome.tunnels.push(merged);
    }

    for remote in input.remote {
        if local_ids.contains(remote.id.as_str()) {
            continue;
        }
        match input.base.get(&remote.id) {
            // 本机删掉的，另一台 Mac 没再改过，保持删除。
            Some(base) if *base == print_of(remote) => {}
            Some(_) => {
                outcome.notes.push(note(remote, NoteKind::Revived, Vec::new()));
                outcome.tunnels.push(remote.clone());
                outcome.added += 1;
            }
            None => {
                outcome.tunnels.push(remote.clone());
                outcome.added += 1;
            }
        }
    }
    outcome
}

fn note(tunnel: &Tunnel, kind: NoteKind, fields: Vec<&'static str>) -> SyncNote {
    SyncNote {
        tunnel_id: tunnel.id.clone(),
        tunnel_name: tunnel.display_name(),
        kind,
        fields,
    }
}

fn merge_one(
    base: Option<&Print>,
    local_newer: bool,
    local: &Tunnel,
    remote: &Tunnel,
    notes: &mut Vec<SyncNote>,
) -> Tunnel {
    let here = print_of(local);
    let there = print_of(remote);

    let settings_from_remote = if here.settings == there.settings {
        false
    } else {
        match base {
            Some(base) if here.settings == base.settings => true,
            Some(base) if there.settings == base.settings => false,
            // 两边都改了：较新的一次保存胜出，两台 Mac 得出同一个结果。
            Some(_) if local_newer => false,
            Some(_) => {
                notes.push(note(local, NoteKind::SettingsReplaced, Vec::new()));
                true
            }
            // 第一次见到这条隧道（刚加入同步）：沿用 iCloud 里的设置。
            None => !local_newer,
        }
    };

    // None 表示两边都改过登录信息（或第一次见面），按本机优先处理。
    let login_from_remote = if here.login == there.login {
        Some(false)
    } else {
        match base {
            Some(base) if here.login == base.login => Some(true),
            Some(base) if there.login == base.login => Some(false),
            _ => None,
        }
    };

    let mut merged = if settings_from_remote {
        remote.clone()
    } else {
        local.clone()
    };
    // 刚加入同步时，iCloud 里没有跳板机就保留本机的，不静默丢掉它的登录信息。
    if settings_from_remote && base.is_none() && remote.jump.is_none() {
        merged.jump = local.jump.clone();
    }

    let login_source = if login_from_remote == Some(true) {
        remote
    } else {
        local
    };
    merged.password = login_source.password.clone();
    merged.auth = login_source.auth;
    merged.key_file = login_source.key_file.clone();
    if let (Some(jump), Some(source)) = (merged.jump.as_mut(), login_source.jump.as_ref()) {
        jump.password = source.password.clone();
        jump.auth = source.auth;
        jump.key_file = source.key_file.clone();
    }

    if login_from_remote.is_none() {
        let mut fields = merge_credentials(
            &mut merged.password,
            &mut merged.auth,
            &mut merged.key_file,
            &remote.password,
            remote.auth,
            &remote.key_file,
            (FIELD_PASSWORD, FIELD_AUTH, FIELD_KEY_FILE),
        );
        if let (Some(here), Some(there)) = (merged.jump.as_mut(), remote.jump.as_ref()) {
            fields.extend(merge_credentials(
                &mut here.password,
                &mut here.auth,
                &mut here.key_file,
                &there.password,
                there.auth,
                &there.key_file,
                (FIELD_JUMP_PASSWORD, FIELD_JUMP_AUTH, FIELD_JUMP_KEY_FILE),
            ));
        }
        if !fields.is_empty() {
            notes.push(note(local, NoteKind::LoginKept, fields));
        }
    }
    merged
}

/// Applies the other Mac's login unless this tunnel already has its own.
/// Returns the user-facing names of the fields that differ and were therefore
/// left alone.
#[allow(clippy::too_many_arguments)]
fn merge_credentials(
    password: &mut String,
    auth: &mut AuthMethod,
    key_file: &mut String,
    incoming_password: &str,
    incoming_auth: AuthMethod,
    incoming_key_file: &str,
    names: (&'static str, &'static str, &'static str),
) -> Vec<&'static str> {
    // 本机这条隧道还没配过登录信息，采用另一台的不算覆盖。
    if password.is_empty() && key_file.is_empty() && *auth == AuthMethod::Password {
        *password = incoming_password.to_string();
        *auth = incoming_auth;
        *key_file = incoming_key_file.to_string();
        return Vec::new();
    }
    let mut fields = Vec::new();
    // 另一台的空值表示“没有”，不是要清除本机已有的密码或私钥。
    if !incoming_password.is_empty() && *password != incoming_password {
        fields.push(names.0);
    }
    if *auth != incoming_auth {
        fields.push(names.1);
    }
    if !incoming_key_file.is_empty() && *key_file != incoming_key_file {
        fields.push(names.2);
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthMethod, Forward, JumpHost};

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

    fn jump(password: &str) -> JumpHost {
        JumpHost {
            host: "10.0.0.9".into(),
            port: 22,
            user: "ops".into(),
            password: password.into(),
            auth: AuthMethod::Password,
            key_file: String::new(),
        }
    }

    struct Case {
        base: Vec<Tunnel>,
        pushed: Vec<Tunnel>,
        local: Vec<Tunnel>,
        remote: Vec<Tunnel>,
        busy: Vec<&'static str>,
        remote_is_newer: bool,
    }

    impl Case {
        /// First contact: nothing synced yet, as when joining an existing sync.
        fn join(local: Vec<Tunnel>, remote: Vec<Tunnel>) -> Self {
            Self {
                base: Vec::new(),
                pushed: Vec::new(),
                local,
                remote,
                busy: Vec::new(),
                remote_is_newer: true,
            }
        }

        /// Both Macs last agreed on `base`, and this Mac uploaded it.
        fn synced(base: Vec<Tunnel>, local: Vec<Tunnel>, remote: Vec<Tunnel>) -> Self {
            Self {
                pushed: base.clone(),
                base,
                local,
                remote,
                busy: Vec::new(),
                remote_is_newer: true,
            }
        }

        fn run(&self) -> MergeOutcome {
            let busy = self.busy.iter().map(|id| id.to_string()).collect();
            merge(&MergeInput {
                base: &prints(&self.base),
                pushed: &prints(&self.pushed),
                local: &self.local,
                remote: &self.remote,
                busy: &busy,
                remote_is_newer: self.remote_is_newer,
            })
        }
    }

    fn ids(outcome: &MergeOutcome) -> Vec<&str> {
        outcome.tunnels.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn prints_split_settings_from_login() {
        let a = tunnel("a", 5432, "one");
        let mut b = a.clone();
        b.password = "two".into();
        assert_eq!(print_of(&a).settings, print_of(&b).settings);
        assert_ne!(print_of(&a).login, print_of(&b).login);
        b.forwards[0].local_port = 1;
        assert_ne!(print_of(&a).settings, print_of(&b).settings);
    }

    #[test]
    fn joining_adds_new_tunnels_after_the_local_ones() {
        let outcome = Case::join(vec![tunnel("a", 5432, "")], vec![tunnel("b", 15432, "pw")]).run();
        assert_eq!(ids(&outcome), ["a", "b"]);
        assert_eq!(outcome.added, 1);
        assert!(outcome.notes.is_empty());
        assert_eq!(outcome.tunnels[1].password, "pw");
    }

    #[test]
    fn joining_never_removes_local_tunnels() {
        let outcome = Case::join(
            vec![tunnel("a", 5432, ""), tunnel("b", 5433, "")],
            vec![tunnel("b", 5433, "")],
        )
        .run();
        assert_eq!(outcome.tunnels.len(), 2);
        assert_eq!(outcome.removed, 0);
    }

    #[test]
    fn joining_takes_the_cloud_settings_but_keeps_local_login() {
        let mut remote = tunnel("a", 25432, "remote-pass");
        remote.note = "家里的 NAS".into();
        let outcome = Case::join(vec![tunnel("a", 5432, "local-pass")], vec![remote]).run();
        assert_eq!(outcome.tunnels[0].forwards[0].local_port, 25432);
        assert_eq!(outcome.tunnels[0].note, "家里的 NAS");
        assert_eq!(outcome.tunnels[0].password, "local-pass");
        assert_eq!(outcome.notes.len(), 1);
        assert_eq!(outcome.notes[0].kind, NoteKind::LoginKept);
        assert_eq!(outcome.notes[0].fields, [FIELD_PASSWORD]);
    }

    #[test]
    fn joining_adopts_a_password_when_the_local_side_has_none() {
        let outcome = Case::join(vec![tunnel("a", 5432, "")], vec![tunnel("a", 5432, "pw")]).run();
        assert_eq!(outcome.tunnels[0].password, "pw");
        assert!(outcome.notes.is_empty());
    }

    #[test]
    fn joining_keeps_a_local_jump_host_the_cloud_omits() {
        let mut local = tunnel("a", 5432, "same");
        local.jump = Some(jump("jump-pass"));
        let outcome = Case::join(vec![local], vec![tunnel("a", 6432, "same")]).run();
        assert_eq!(outcome.tunnels[0].jump.as_ref().unwrap().password, "jump-pass");
        assert!(outcome.notes.is_empty());
    }

    #[test]
    fn joining_reports_the_whole_jump_login() {
        let mut local = tunnel("a", 5432, "same");
        local.jump = Some(jump("jump-local"));
        let mut remote = tunnel("a", 5432, "same");
        remote.jump = Some(JumpHost {
            auth: AuthMethod::Key,
            key_file: "/home/ops/id".into(),
            ..jump("jump-remote")
        });
        let outcome = Case::join(vec![local], vec![remote]).run();
        assert_eq!(
            outcome.notes[0].fields,
            [FIELD_JUMP_PASSWORD, FIELD_JUMP_AUTH, FIELD_JUMP_KEY_FILE]
        );
        let kept = outcome.tunnels[0].jump.as_ref().unwrap();
        assert_eq!(kept.password, "jump-local");
        assert_eq!(kept.auth, AuthMethod::Password);
    }

    #[test]
    fn a_change_made_only_on_the_other_mac_is_taken_whole() {
        let base = tunnel("a", 5432, "old");
        let mut remote = tunnel("a", 6432, "new");
        remote.auto_reconnect = true;
        let outcome = Case::synced(vec![base.clone()], vec![base], vec![remote]).run();
        let merged = &outcome.tunnels[0];
        assert_eq!(merged.forwards[0].local_port, 6432);
        assert_eq!(merged.password, "new");
        assert!(merged.auto_reconnect);
        assert_eq!(outcome.updated, 1);
        assert!(outcome.notes.is_empty());
    }

    #[test]
    fn a_change_made_only_here_is_kept() {
        let base = tunnel("a", 5432, "old");
        let local = tunnel("a", 7000, "mine");
        let outcome = Case::synced(vec![base.clone()], vec![local], vec![base]).run();
        assert_eq!(outcome.tunnels[0].forwards[0].local_port, 7000);
        assert_eq!(outcome.tunnels[0].password, "mine");
        assert_eq!(outcome.updated, 0);
        assert!(outcome.notes.is_empty());
    }

    #[test]
    fn concurrent_settings_edits_go_to_the_newer_save() {
        let base = tunnel("a", 5432, "");
        let mut case = Case::synced(
            vec![base.clone()],
            vec![tunnel("a", 7000, "")],
            vec![tunnel("a", 8000, "")],
        );
        // 本机的改动已经上传过，另一台的更新：采用另一台的。
        case.pushed = case.local.clone();
        let outcome = case.run();
        assert_eq!(outcome.tunnels[0].forwards[0].local_port, 8000);
        assert_eq!(outcome.notes[0].kind, NoteKind::SettingsReplaced);

        // 另一台的更早：两台 Mac 都留下本机这一版。
        case.remote_is_newer = false;
        let outcome = case.run();
        assert_eq!(outcome.tunnels[0].forwards[0].local_port, 7000);
        assert!(outcome.notes.is_empty());

        // 本机还有没上传的改动：它是最新的一次保存。
        case.remote_is_newer = true;
        case.pushed = vec![base];
        let outcome = case.run();
        assert_eq!(outcome.tunnels[0].forwards[0].local_port, 7000);
    }

    #[test]
    fn concurrent_login_edits_keep_the_local_one() {
        let base = tunnel("a", 5432, "old");
        let outcome = Case::synced(
            vec![base],
            vec![tunnel("a", 5432, "mine")],
            vec![tunnel("a", 5432, "theirs")],
        )
        .run();
        assert_eq!(outcome.tunnels[0].password, "mine");
        assert_eq!(outcome.notes[0].kind, NoteKind::LoginKept);
        assert_eq!(outcome.notes[0].fields, [FIELD_PASSWORD]);
    }

    #[test]
    fn a_login_kept_after_a_conflict_is_not_reported_again() {
        // 上次冲突后 base 记的是另一台的密码；对方没再改，就不再提示。
        let theirs = tunnel("a", 5432, "theirs");
        let outcome = Case::synced(
            vec![theirs.clone()],
            vec![tunnel("a", 5432, "mine")],
            vec![theirs],
        )
        .run();
        assert_eq!(outcome.tunnels[0].password, "mine");
        assert!(outcome.notes.is_empty());
    }

    #[test]
    fn a_tunnel_deleted_elsewhere_and_untouched_here_goes() {
        let a = tunnel("a", 5432, "");
        let b = tunnel("b", 5433, "");
        let outcome = Case::synced(vec![a.clone(), b.clone()], vec![a.clone(), b], vec![a]).run();
        assert_eq!(ids(&outcome), ["a"]);
        assert_eq!(outcome.removed, 1);
    }

    #[test]
    fn a_tunnel_deleted_elsewhere_stays_if_running_or_changed_here() {
        let a = tunnel("a", 5432, "");
        let mut case = Case::synced(vec![a.clone()], vec![a.clone()], Vec::new());
        case.busy = vec!["a"];
        let outcome = case.run();
        assert_eq!(ids(&outcome), ["a"]);
        assert_eq!(outcome.notes[0].kind, NoteKind::KeptDeleted);

        let outcome = Case::synced(vec![a], vec![tunnel("a", 9999, "")], Vec::new()).run();
        assert_eq!(ids(&outcome), ["a"]);
        assert_eq!(outcome.notes[0].kind, NoteKind::KeptDeleted);
    }

    #[test]
    fn a_tunnel_never_synced_is_never_deleted() {
        let a = tunnel("a", 5432, "");
        let outcome = Case::synced(Vec::new(), vec![a], Vec::new()).run();
        assert_eq!(ids(&outcome), ["a"]);
        assert!(outcome.notes.is_empty());
    }

    #[test]
    fn a_tunnel_deleted_here_stays_deleted_unless_changed_elsewhere() {
        let a = tunnel("a", 5432, "");
        let outcome = Case::synced(vec![a.clone()], Vec::new(), vec![a.clone()]).run();
        assert!(outcome.tunnels.is_empty());

        let outcome = Case::synced(vec![a], Vec::new(), vec![tunnel("a", 6000, "")]).run();
        assert_eq!(ids(&outcome), ["a"]);
        assert_eq!(outcome.notes[0].kind, NoteKind::Revived);
    }

    #[test]
    fn a_jump_host_removed_on_the_other_mac_is_removed_here() {
        let mut base = tunnel("a", 5432, "");
        base.jump = Some(jump("j"));
        let outcome = Case::synced(vec![base.clone()], vec![base], vec![tunnel("a", 5432, "")]).run();
        assert!(outcome.tunnels[0].jump.is_none());
    }

    #[test]
    fn identical_copies_change_nothing() {
        let a = tunnel("a", 5432, "same");
        let outcome = Case::join(vec![a.clone()], vec![a]).run();
        assert_eq!(outcome.updated, 0);
        assert!(outcome.notes.is_empty());
    }
}
