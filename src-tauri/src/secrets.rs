/// Where remembered vault passwords live. The password itself never touches
/// `tunnels.json` or any file that is synced.
pub const SERVICE: &str = "com.tailtunnel.app.vault";

pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// The account name scopes a password to one config and one password
/// generation, so a new `key_id` never silently reuses an old password.
pub fn account(vault_id: &str, key_id: &str) -> Result<String, String> {
    // The two ids are joined with ':', so a ':' inside either one would let
    // two different pairs collide on the same account.
    if vault_id.contains(':') || key_id.contains(':') {
        return Err("配置文件已损坏。".into());
    }
    Ok(format!("{vault_id}:{key_id}"))
}

pub fn recall(
    store: &dyn SecretStore,
    vault_id: &str,
    key_id: &str,
) -> Result<Option<String>, String> {
    store.get(&account(vault_id, key_id)?)
}

pub fn remember(
    store: &dyn SecretStore,
    vault_id: &str,
    key_id: &str,
    password: &str,
) -> Result<(), String> {
    if password.is_empty() {
        return Err("密码不能为空。".into());
    }
    store.set(&account(vault_id, key_id)?, password)
}

pub fn forget(store: &dyn SecretStore, vault_id: &str, key_id: &str) -> Result<(), String> {
    store.delete(&account(vault_id, key_id)?)
}

#[cfg(target_os = "macos")]
pub struct KeychainStore;

#[cfg(target_os = "macos")]
impl SecretStore for KeychainStore {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(SERVICE, account)
            .map_err(|error| format!("无法访问钥匙串：{error}"))?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!("读取钥匙串失败：{error}")),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(SERVICE, account)
            .map_err(|error| format!("无法访问钥匙串：{error}"))?;
        entry
            .set_password(secret)
            .map_err(|error| format!("写入钥匙串失败：{error}"))
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(SERVICE, account)
            .map_err(|error| format!("无法访问钥匙串：{error}"))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(format!("删除钥匙串条目失败：{error}")),
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub struct KeychainStore;

#[cfg(not(target_os = "macos"))]
impl SecretStore for KeychainStore {
    fn get(&self, _account: &str) -> Result<Option<String>, String> {
        Err("当前平台还不支持在钥匙串里记住密码。".into())
    }

    fn set(&self, _account: &str, _secret: &str) -> Result<(), String> {
        Err("当前平台还不支持在钥匙串里记住密码。".into())
    }

    fn delete(&self, _account: &str) -> Result<(), String> {
        Err("当前平台还不支持在钥匙串里记住密码。".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryStore(Mutex<HashMap<String, String>>);

    impl SecretStore for MemoryStore {
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(account).cloned())
        }

        fn set(&self, account: &str, secret: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(account.to_string(), secret.to_string());
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(account);
            Ok(())
        }
    }

    #[test]
    fn account_scopes_by_vault_and_key() {
        assert_eq!(account("vault-1", "key-1").unwrap(), "vault-1:key-1");
    }

    #[test]
    fn account_rejects_ids_containing_a_colon() {
        assert!(account("a:b", "c").is_err());
        assert!(account("a", "b:c").is_err());
        let store = MemoryStore::default();
        assert!(remember(&store, "a:b", "c", "hunter2").is_err());
        assert!(recall(&store, "a:b", "c").is_err());
        assert!(forget(&store, "a", "b:c").is_err());
    }

    #[test]
    fn remembers_and_recalls_a_password() {
        let store = MemoryStore::default();
        remember(&store, "vault-1", "key-1", "hunter2").unwrap();
        assert_eq!(recall(&store, "vault-1", "key-1").unwrap().as_deref(), Some("hunter2"));
    }

    #[test]
    fn another_key_id_does_not_reuse_the_password() {
        let store = MemoryStore::default();
        remember(&store, "vault-1", "key-1", "hunter2").unwrap();
        assert_eq!(recall(&store, "vault-1", "key-2").unwrap(), None);
        assert_eq!(recall(&store, "vault-2", "key-1").unwrap(), None);
    }

    #[test]
    fn forgetting_removes_the_entry() {
        let store = MemoryStore::default();
        remember(&store, "vault-1", "key-1", "hunter2").unwrap();
        forget(&store, "vault-1", "key-1").unwrap();
        assert_eq!(recall(&store, "vault-1", "key-1").unwrap(), None);
    }

    #[test]
    fn empty_password_is_not_remembered() {
        let store = MemoryStore::default();
        assert_eq!(
            remember(&store, "vault-1", "key-1", "").unwrap_err(),
            "密码不能为空。"
        );
    }
}
