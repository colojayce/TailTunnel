//! Merging a newer snapshot from another Mac into the local config.
//!
//! Snapshots are never merged field by field into a blended tunnel, and local
//! tunnels are never removed. A tunnel the snapshot does not know about stays
//! as it is; a tunnel present on both sides takes the snapshot's forwarding
//! settings. Login credentials are the exception: they belong to this Mac and
//! are only adopted when there is nothing to overwrite. A differing credential
//! is reported so the app can ask the user instead of silently replacing it.

use crate::model::{AuthMethod, Tunnel};
use std::collections::HashMap;

/// User-facing names of the credential fields a conflict can involve.
pub const FIELD_PASSWORD: &str = "密码";
pub const FIELD_AUTH: &str = "登录方式";
pub const FIELD_KEY_FILE: &str = "私钥";
pub const FIELD_JUMP_PASSWORD: &str = "跳板机密码";
pub const FIELD_JUMP_AUTH: &str = "跳板机登录方式";
pub const FIELD_JUMP_KEY_FILE: &str = "跳板机私钥";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialConflict {
    pub tunnel_id: String,
    pub tunnel_name: String,
    pub fields: Vec<&'static str>,
}

// 不派生 Debug：Tunnel 里有明文 SSH 密码，避免被顺手 {:?} 打进日志。
#[derive(Default)]
pub struct MergeOutcome {
    pub tunnels: Vec<Tunnel>,
    pub conflicts: Vec<CredentialConflict>,
    pub added: usize,
    pub updated: usize,
}

pub fn merge_snapshots(local: &[Tunnel], incoming: &[Tunnel]) -> MergeOutcome {
    let mut outcome = MergeOutcome {
        tunnels: local.to_vec(),
        ..MergeOutcome::default()
    };
    let index: HashMap<&str, usize> = local
        .iter()
        .enumerate()
        .map(|(at, tunnel)| (tunnel.id.as_str(), at))
        .collect();
    for source in incoming {
        let Some(&at) = index.get(source.id.as_str()) else {
            // 本地还没有这个隧道，整条采用，包括它的登录信息。
            outcome.tunnels.push(source.clone());
            outcome.added += 1;
            continue;
        };
        let target = &mut outcome.tunnels[at];
        let mut fields = merge_credentials(
            &mut target.password,
            &mut target.auth,
            &mut target.key_file,
            &source.password,
            source.auth,
            &source.key_file,
            (FIELD_PASSWORD, FIELD_AUTH, FIELD_KEY_FILE),
        );
        target.host = source.host.clone();
        target.port = source.port;
        target.user = source.user.clone();
        target.note = source.note.clone();
        target.forwards = source.forwards.clone();
        target.auto_reconnect = source.auto_reconnect;
        match (&mut target.jump, &source.jump) {
            (Some(here), Some(there)) => {
                here.host = there.host.clone();
                here.port = there.port;
                here.user = there.user.clone();
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
            (slot @ None, Some(there)) => *slot = Some(there.clone()),
            // 快照里没有跳板机时保留本地的，不静默丢掉它的登录信息。
            (Some(_), None) | (None, None) => {}
        }
        if !fields.is_empty() {
            outcome.conflicts.push(CredentialConflict {
                tunnel_id: source.id.clone(),
                tunnel_name: source.display_name(),
                fields,
            });
        }
        outcome.updated += 1;
    }
    outcome
}

/// Applies the snapshot's login settings unless the local tunnel already has
/// its own. Returns the user-facing names of the fields that differ and were
/// therefore left alone.
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
    // 本地这条隧道还没配过登录信息，采用快照里的不算覆盖。
    if password.is_empty() && key_file.is_empty() && *auth == AuthMethod::Password {
        *password = incoming_password.to_string();
        *auth = incoming_auth;
        *key_file = incoming_key_file.to_string();
        return Vec::new();
    }
    let mut fields = Vec::new();
    // 快照里的空值表示“没有”，不是要清除本地已有的密码或私钥。
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

    #[test]
    fn adds_new_tunnels_after_the_local_ones() {
        let local = vec![tunnel("a", 5432, "")];
        let incoming = vec![tunnel("b", 15432, "remote-pass")];
        let outcome = merge_snapshots(&local, &incoming);
        let ids: Vec<_> = outcome.tunnels.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(outcome.added, 1);
        assert!(outcome.conflicts.is_empty());
        // 本地没有这个隧道，密码不算覆盖，直接采用。
        assert_eq!(outcome.tunnels[1].password, "remote-pass");
    }

    #[test]
    fn never_removes_local_tunnels() {
        let local = vec![tunnel("a", 5432, ""), tunnel("b", 5433, "")];
        let incoming = vec![tunnel("b", 5433, "")];
        let outcome = merge_snapshots(&local, &incoming);
        assert_eq!(outcome.tunnels.len(), 2);
        assert_eq!(outcome.added, 0);
    }

    #[test]
    fn overwrites_forwarding_settings_of_an_existing_tunnel() {
        let local = vec![tunnel("a", 5432, "")];
        let mut incoming = tunnel("a", 25432, "");
        incoming.note = "家里的 NAS".into();
        incoming.auto_reconnect = true;
        let outcome = merge_snapshots(&local, std::slice::from_ref(&incoming));
        assert_eq!(outcome.updated, 1);
        assert_eq!(outcome.tunnels[0].forwards[0].local_port, 25432);
        assert_eq!(outcome.tunnels[0].note, "家里的 NAS");
        assert!(outcome.tunnels[0].auto_reconnect);
        assert!(outcome.conflicts.is_empty());
    }

    #[test]
    fn keeps_the_local_password_and_reports_the_conflict() {
        let local = vec![tunnel("a", 5432, "local-pass")];
        let incoming = vec![tunnel("a", 5432, "remote-pass")];
        let outcome = merge_snapshots(&local, &incoming);
        assert_eq!(outcome.tunnels[0].password, "local-pass");
        assert_eq!(outcome.conflicts.len(), 1);
        assert_eq!(outcome.conflicts[0].fields, [FIELD_PASSWORD]);
        assert_eq!(outcome.conflicts[0].tunnel_id, "a");
    }

    #[test]
    fn adopts_a_password_when_the_local_side_has_none() {
        let local = vec![tunnel("a", 5432, "")];
        let incoming = vec![tunnel("a", 5432, "remote-pass")];
        let outcome = merge_snapshots(&local, &incoming);
        assert_eq!(outcome.tunnels[0].password, "remote-pass");
        assert!(outcome.conflicts.is_empty());
    }

    #[test]
    fn reports_the_whole_credential_group_of_a_jump_host() {
        let mut local = tunnel("a", 5432, "same");
        local.jump = Some(jump("jump-local"));
        let mut incoming = tunnel("a", 5432, "same");
        incoming.jump = Some(JumpHost {
            password: "jump-remote".into(),
            auth: AuthMethod::Key,
            key_file: "/home/ops/id".into(),
            ..jump("jump-remote")
        });
        let outcome = merge_snapshots(
            std::slice::from_ref(&local),
            std::slice::from_ref(&incoming),
        );
        assert_eq!(
            outcome.conflicts[0].fields,
            [FIELD_JUMP_PASSWORD, FIELD_JUMP_AUTH, FIELD_JUMP_KEY_FILE]
        );
        // 跳板机本身还指向同一台机器，主机和端口仍按快照更新。
        let kept = outcome.tunnels[0].jump.as_ref().unwrap();
        assert_eq!(kept.password, "jump-local");
        assert_eq!(kept.auth, AuthMethod::Password);
        assert_eq!(kept.host, "10.0.0.9");
    }

    #[test]
    fn keeps_a_local_jump_host_the_snapshot_omits() {
        let mut local = tunnel("a", 5432, "same");
        local.jump = Some(jump("jump-pass"));
        let incoming = tunnel("a", 6432, "same");
        let outcome = merge_snapshots(
            std::slice::from_ref(&local),
            std::slice::from_ref(&incoming),
        );
        assert!(outcome.tunnels[0].jump.is_some());
        assert_eq!(outcome.tunnels[0].jump.as_ref().unwrap().password, "jump-pass");
        assert!(outcome.conflicts.is_empty());
    }

    #[test]
    fn identical_credentials_are_not_reported() {
        let local = vec![tunnel("a", 5432, "same")];
        let mut incoming = tunnel("a", 6432, "same");
        incoming.jump = local[0].jump.clone();
        let outcome = merge_snapshots(&local, std::slice::from_ref(&incoming));
        assert!(outcome.conflicts.is_empty());
        assert_eq!(outcome.updated, 1);
    }

    #[test]
    fn reports_a_changed_login_method() {
        let local = vec![tunnel("a", 5432, "local-pass")];
        let mut incoming = tunnel("a", 5432, "");
        incoming.auth = AuthMethod::Key;
        incoming.key_file = "/home/root/id_ed25519".into();
        let outcome = merge_snapshots(&local, std::slice::from_ref(&incoming));
        assert_eq!(outcome.conflicts[0].fields, [FIELD_AUTH, FIELD_KEY_FILE]);
        assert_eq!(outcome.tunnels[0].auth, AuthMethod::Password);
        assert_eq!(outcome.tunnels[0].key_file, "");
    }
}
