//! Paired devices: `devices.json` plus one 32-byte key file per device, in a
//! private directory. Every change runs under `store.lock`, so `mobile pair`,
//! `mobile revoke` and a running gateway never lose each other's writes.

use super::crypto::{self, KEY_LEN};
use crate::agent_panel::private_fs;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

/// A pending pairing is usable for this long, once.
pub const PAIR_TTL_MS: u64 = 10 * 60 * 1000;
/// Consecutive failed handshakes that disable a device until it is paired again.
pub const MAX_FAILURES: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Pending,
    Active,
    Disabled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// The panel profile every call from this device is bound to.
    pub profile: String,
    pub state: State,
    pub created: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ip: Option<String>,
    #[serde(default)]
    pub failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_token: Option<String>,
}

impl Device {
    /// Whether a handshake may be attempted for this device now.
    pub fn usable(&self, now: u64) -> bool {
        match self.state {
            State::Active => true,
            State::Pending => self.expires.is_some_and(|e| now < e),
            State::Disabled => false,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registry {
    pub schema_version: u32,
    /// Random identity of this gateway, shown to the phone as `fp`.
    pub gateway_id: String,
    pub devices: Vec<Device>,
}

impl Registry {
    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }
    pub fn device_mut(&mut self, id: &str) -> Option<&mut Device> {
        self.devices.iter_mut().find(|d| d.id == id)
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct Store {
    directory: PathBuf,
}

impl Store {
    /// `$CHROME_USE_RELAY_DIR/mobile` when set (as the panel service uses
    /// `$CHROME_USE_RELAY_DIR/agent-panel`), otherwise `~/.chrome-use/mobile`.
    pub fn open() -> Result<Self, String> {
        let directory = match std::env::var_os("CHROME_USE_RELAY_DIR").filter(|s| !s.is_empty()) {
            Some(dir) => {
                let dir = PathBuf::from(dir);
                if !dir.is_absolute() {
                    return Err("CHROME_USE_RELAY_DIR must be absolute".into());
                }
                dir.join("mobile")
            }
            None => dirs::home_dir()
                .ok_or("Cannot locate the user's home directory")?
                .join(".chrome-use/mobile"),
        };
        Self::at(directory)
    }

    pub fn at(directory: PathBuf) -> Result<Self, String> {
        private_fs::directory(&directory)?;
        private_fs::directory(&directory.join("keys"))?;
        Ok(Self { directory })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn registry_path(&self) -> PathBuf {
        self.directory.join("devices.json")
    }

    fn key_path(&self, id: &str) -> Result<PathBuf, String> {
        uuid::Uuid::parse_str(id).map_err(|_| "Invalid device id")?;
        Ok(self.directory.join("keys").join(format!("{id}.key")))
    }

    fn load(&self) -> Result<Registry, String> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(Registry {
                schema_version: 1,
                gateway_id: crypto::b64url(&crypto::random::<16>()?),
                devices: Vec::new(),
            });
        }
        let mut bytes = Vec::new();
        private_fs::open(&path, false)?
            .take(4 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|_| "Cannot read the paired-device list")?;
        let registry: Registry =
            serde_json::from_slice(&bytes).map_err(|_| "The paired-device list is invalid")?;
        if registry.schema_version != 1 {
            return Err("The paired-device list has an unsupported version".into());
        }
        Ok(registry)
    }

    /// Read without changing anything. Taken under the lock too, so a reader
    /// never sees a half-applied change of another process.
    pub fn read(&self) -> Result<Registry, String> {
        let lock = private_fs::open(&self.directory.join("store.lock"), true)?;
        lock.lock()
            .map_err(|_| "Cannot lock the paired-device list")?;
        self.load()
    }

    /// Apply one change and write it back atomically.
    pub fn update<T>(
        &self,
        change: impl FnOnce(&mut Registry) -> Result<T, String>,
    ) -> Result<T, String> {
        let lock = private_fs::open(&self.directory.join("store.lock"), true)?;
        lock.lock()
            .map_err(|_| "Cannot lock the paired-device list")?;
        let mut registry = self.load()?;
        let result = change(&mut registry)?;
        let bytes =
            serde_json::to_vec_pretty(&registry).map_err(|_| "Cannot encode device list")?;
        private_fs::write(&self.registry_path(), &bytes)?;
        Ok(result)
    }

    pub fn secret(&self, id: &str) -> Result<[u8; KEY_LEN], String> {
        let mut bytes = Vec::new();
        private_fs::open(&self.key_path(id)?, false)?
            .take(KEY_LEN as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Cannot read the device key")?;
        bytes
            .try_into()
            .map_err(|_| "The device key file is invalid".into())
    }

    /// A new pending device and its pair secret. Expired pending entries are
    /// removed on the way.
    pub fn create_pending(
        &self,
        name: &str,
        profile: &str,
    ) -> Result<(Device, [u8; KEY_LEN], String), String> {
        let secret = crypto::random::<KEY_LEN>()?;
        let now = now_ms();
        let device = Device {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            profile: profile.into(),
            state: State::Pending,
            created: now,
            expires: Some(now + PAIR_TTL_MS),
            last_seen: None,
            last_ip: None,
            failures: 0,
            relay_token: None,
        };
        // The key exists before the record that refers to it.
        private_fs::write(&self.key_path(&device.id)?, &secret)?;
        let stored = device.clone();
        let expired = self.update(move |registry| {
            let expired: Vec<String> = registry
                .devices
                .iter()
                .filter(|d| d.state == State::Pending && !d.usable(now))
                .map(|d| d.id.clone())
                .collect();
            registry.devices.retain(|d| !expired.contains(&d.id));
            registry.devices.push(stored);
            Ok((expired, registry.gateway_id.clone()))
        });
        let (expired, gateway) = match expired {
            Ok(v) => v,
            Err(error) => {
                let _ = std::fs::remove_file(self.key_path(&device.id)?);
                return Err(error);
            }
        };
        for id in expired {
            let _ = std::fs::remove_file(self.key_path(&id)?);
        }
        Ok((device, secret, gateway))
    }

    /// Delete a device's record, key and push token.
    pub fn revoke(&self, id: &str) -> Result<Device, String> {
        let key = self.key_path(id)?;
        let device = self.update(|registry| {
            let index = registry
                .devices
                .iter()
                .position(|d| d.id == id)
                .ok_or("No paired device has that id")?;
            Ok(registry.devices.remove(index))
        })?;
        match std::fs::remove_file(key) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("The device was removed but its key file remains".into()),
        }
        Ok(device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_records_are_private_and_revocable() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::at(temp.path().join("mobile")).unwrap();
        let (device, secret, gateway) = store.create_pending("Phone", "profile-1").unwrap();
        assert_eq!(store.secret(&device.id).unwrap(), secret);
        let registry = store.read().unwrap();
        assert_eq!(registry.gateway_id, gateway);
        let stored = registry.device(&device.id).unwrap();
        assert_eq!(stored.state, State::Pending);
        assert!(stored.usable(now_ms()));
        assert!(!stored.usable(now_ms() + PAIR_TTL_MS));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(store.directory()), 0o700);
            assert_eq!(mode(&store.directory().join("keys")), 0o700);
            assert_eq!(mode(&store.directory().join("devices.json")), 0o600);
            assert_eq!(mode(&store.key_path(&device.id).unwrap()), 0o600);
        }
        // The gateway identity is stable across changes.
        store.create_pending("Tablet", "profile-1").unwrap();
        assert_eq!(store.read().unwrap().gateway_id, gateway);
        store.revoke(&device.id).unwrap();
        assert!(store.read().unwrap().device(&device.id).is_none());
        assert!(store.secret(&device.id).is_err());
        assert!(store.revoke(&device.id).is_err());
        assert!(store.secret("../escape").is_err());
    }

    #[test]
    fn expired_pending_devices_are_pruned() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::at(temp.path().join("mobile")).unwrap();
        let (old, _, _) = store.create_pending("Old", "p").unwrap();
        store
            .update(|r| {
                r.device_mut(&old.id).unwrap().expires = Some(1);
                Ok(())
            })
            .unwrap();
        store.create_pending("New", "p").unwrap();
        assert!(store.read().unwrap().device(&old.id).is_none());
        assert!(store.secret(&old.id).is_err());
    }
}
