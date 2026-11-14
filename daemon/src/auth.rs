use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Paired phones. Only token hashes are stored, so the file alone can't be used to connect.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DeviceStore {
    #[serde(default)]
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub name: String,
    pub token_sha256: String,
    pub created: u64,
}

fn hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

impl DeviceStore {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        crate::util::write_private(path, toml::to_string_pretty(self)?.as_bytes())
    }

    /// Returns the device name if the token belongs to a paired device.
    pub fn verify(&self, token: &str) -> Option<&str> {
        let h = hash(token);
        let mut found = None;
        // Check every entry so timing doesn't reveal which one matched.
        for d in &self.devices {
            let Ok(stored) = hex::decode(&d.token_sha256) else { continue };
            if bool::from(stored.ct_eq(&h)) {
                found = Some(d.name.as_str());
            }
        }
        found
    }

    /// Create a fresh token for `name`, replacing any previous pairing with that name.
    pub fn pair(&mut self, name: &str) -> String {
        let token = hex::encode(rand::random::<[u8; 32]>());
        self.devices.retain(|d| d.name != name);
        self.devices.push(Device {
            name: name.to_string(),
            token_sha256: hex::encode(hash(&token)),
            created: crate::util::now_unix(),
        });
        token
    }

    pub fn revoke(&mut self, name: &str) -> bool {
        let before = self.devices.len();
        self.devices.retain(|d| d.name != name);
        self.devices.len() != before
    }

    /// First unused "iPhone", "iPhone 2", ... name.
    pub fn next_name(&self, base: &str) -> String {
        let taken = |n: &str| self.devices.iter().any(|d| d.name == n);
        if !taken(base) {
            return base.to_string();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_verify_revoke() {
        let mut s = DeviceStore::default();
        let t = s.pair("phone");
        assert_eq!(s.verify(&t), Some("phone"));
        assert_eq!(s.verify("nope"), None);
        let t2 = s.pair("phone");
        assert_eq!(s.verify(&t), None, "re-pairing replaces the old token");
        assert_eq!(s.verify(&t2), Some("phone"));
        assert!(s.revoke("phone"));
        assert_eq!(s.verify(&t2), None);
    }
}
