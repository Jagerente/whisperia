use std::{collections::BTreeMap, fmt, io, path::PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::files;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Fingerprint([u8; 32]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a certificate fingerprint")]
pub struct NotAFingerprint;

impl Fingerprint {
    pub fn of(certificate: &[u8]) -> Self {
        Self(Sha256::digest(certificate).into())
    }

    pub fn short(&self) -> String {
        self.0[..8]
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl From<[u8; 32]> for Fingerprint {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl std::str::FromStr for Fingerprint {
    type Err = NotAFingerprint;

    fn from_str(hex: &str) -> Result<Self, NotAFingerprint> {
        if hex.len() != 64 || !hex.is_ascii() {
            return Err(NotAFingerprint);
        }
        let mut bytes = [0u8; 32];
        for (at, byte) in bytes.iter_mut().enumerate() {
            *byte =
                u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).map_err(|_| NotAFingerprint)?;
        }
        Ok(Self(bytes))
    }
}

impl TryFrom<String> for Fingerprint {
    type Error = NotAFingerprint;

    fn try_from(hex: String) -> Result<Self, NotAFingerprint> {
        hex.parse()
    }
}

impl From<Fingerprint> for String {
    fn from(fingerprint: Fingerprint) -> String {
        fingerprint.hex()
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", self.short())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Trusted,
    FirstContact,
    Changed,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    servers: BTreeMap<String, Fingerprint>,
}

#[derive(Debug)]
pub struct TrustStore {
    path: Option<PathBuf>,
    stored: Stored,
}

impl TrustStore {
    pub fn load(path: PathBuf) -> Self {
        let (stored, problem) = files::load_json(&path);
        if let Some(problem) = problem {
            tracing::warn!("known servers: {problem}");
        }
        Self {
            path: Some(path),
            stored,
        }
    }

    pub fn in_memory() -> Self {
        Self {
            path: None,
            stored: Stored::default(),
        }
    }

    pub fn check(&self, key: &str, fingerprint: &Fingerprint) -> Verdict {
        match self.stored.servers.get(key) {
            Some(known) if known == fingerprint => Verdict::Trusted,
            Some(_) => Verdict::Changed,
            None if self
                .stored
                .servers
                .values()
                .any(|known| known == fingerprint) =>
            {
                Verdict::Trusted
            }
            None => Verdict::FirstContact,
        }
    }

    pub fn remember(&mut self, key: &str, fingerprint: &Fingerprint) -> io::Result<()> {
        if self.stored.servers.get(key) == Some(fingerprint) {
            return Ok(());
        }
        self.stored.servers.insert(key.to_owned(), *fingerprint);
        match &self.path {
            Some(path) => files::save_json(path, &self.stored),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(byte: u8) -> Fingerprint {
        Fingerprint::from([byte; 32])
    }

    #[test]
    fn a_server_is_met_then_trusted_and_a_new_certificate_is_a_change() {
        let mut store = TrustStore::in_memory();
        let (a, b) = (fingerprint(1), fingerprint(2));
        assert_eq!(store.check("DESKTOP-AV83", &a), Verdict::FirstContact);
        store.remember("DESKTOP-AV83", &a).unwrap();
        assert_eq!(store.check("DESKTOP-AV83", &a), Verdict::Trusted);
        assert_eq!(store.check("DESKTOP-AV83", &b), Verdict::Changed);
        assert_eq!(store.check("DESKTOP-9FJD", &b), Verdict::FirstContact);
    }

    #[test]
    fn a_known_certificate_is_trusted_at_a_new_address() {
        let mut store = TrustStore::in_memory();
        store.remember("DESKTOP-AV83", &fingerprint(1)).unwrap();
        assert_eq!(
            store.check("192.168.1.5:47820", &fingerprint(1)),
            Verdict::Trusted
        );
    }

    #[test]
    fn another_known_server_showing_up_under_a_pinned_name_is_a_change() {
        let mut store = TrustStore::in_memory();
        store.remember("DESKTOP-AV83", &fingerprint(1)).unwrap();
        store.remember("DESKTOP-R8ZE", &fingerprint(2)).unwrap();
        assert_eq!(
            store.check("DESKTOP-AV83", &fingerprint(2)),
            Verdict::Changed
        );
    }

    #[test]
    fn what_is_remembered_is_there_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("ww-trust-{}", std::process::id()));
        let path = dir.join("known_servers.json");
        let mut store = TrustStore::load(path.clone());
        store.remember("DESKTOP-AV83", &fingerprint(7)).unwrap();
        assert_eq!(
            TrustStore::load(path).check("DESKTOP-AV83", &fingerprint(7)),
            Verdict::Trusted
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_file_does_not_lose_what_was_pinned() {
        let dir = std::env::temp_dir().join(format!("ww-trust-broken-{}", std::process::id()));
        let path = dir.join("known_servers.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "{\"servers\": {\"DESKTOP-AV83\": ").unwrap();
        let mut store = TrustStore::load(path);
        store.remember("DESKTOP-R8ZE", &fingerprint(2)).unwrap();
        let aside = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains(".broken-"));
        assert!(aside, "the damaged file is kept beside the new one");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_fingerprint_is_written_and_read_as_hex() {
        let written = String::from(fingerprint(0xab));
        assert_eq!(written, "ab".repeat(32));
        assert_eq!(written.parse(), Ok(fingerprint(0xab)));
        assert_eq!("ab".parse::<Fingerprint>(), Err(NotAFingerprint));
        assert_eq!("zz".repeat(32).parse::<Fingerprint>(), Err(NotAFingerprint));
        assert_eq!(fingerprint(0xab).short(), "AB:AB:AB:AB:AB:AB:AB:AB");
    }
}
