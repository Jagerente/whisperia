use std::{
    fs,
    io::{self, Write},
    path::Path,
};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

const CERT_FILE: &str = "cert.der";
const KEY_FILE: &str = "key.der";

#[derive(Debug, thiserror::Error)]
pub enum CertError {
    #[error("cannot access server identity in {dir}: {source}")]
    Io { dir: String, source: io::Error },
    #[error("the server identity in {dir} is incomplete; delete the folder for a new identity")]
    Incomplete { dir: String },
    #[error("cannot generate a certificate: {0}")]
    Generate(#[from] rcgen::Error),
}

pub struct Identity {
    pub cert: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
}

impl Identity {
    pub fn load_or_create(dir: &Path) -> Result<Self, CertError> {
        let io_err = |source| CertError::Io {
            dir: dir.display().to_string(),
            source,
        };
        let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));

        let missing = |read: &io::Result<Vec<u8>>| {
            read.as_ref()
                .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        };
        match (fs::read(&cert_path), fs::read(&key_path)) {
            (Ok(cert), Ok(key)) => {
                return Ok(Self {
                    cert: cert.into(),
                    key: PrivatePkcs8KeyDer::from(key).into(),
                })
            }
            (cert, key) if missing(&cert) && missing(&key) => {}
            (Err(e), _) | (_, Err(e)) if e.kind() != io::ErrorKind::NotFound => {
                return Err(io_err(e))
            }
            _ => {
                return Err(CertError::Incomplete {
                    dir: dir.display().to_string(),
                })
            }
        }

        let generated =
            rcgen::generate_simple_self_signed(vec![whisperia_proto::TLS_SERVER_NAME.to_owned()])?;
        let cert = generated.cert.der().to_vec();
        let key = generated.signing_key.serialize_der();
        fs::create_dir_all(dir).map_err(io_err)?;
        write_atomically(&key_path, &key).map_err(io_err)?;
        write_atomically(&cert_path, &cert).map_err(io_err)?;
        Ok(Self {
            cert: cert.into(),
            key: PrivatePkcs8KeyDer::from(key).into(),
        })
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        Sha256::digest(&self.cert).into()
    }
}

pub fn format_fingerprint(fp: &[u8; 32]) -> String {
    fp.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("new");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ww-cert-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn an_identity_is_made_once_and_kept() {
        let dir = folder("kept");
        let first = Identity::load_or_create(&dir).unwrap().fingerprint();
        let again = Identity::load_or_create(&dir).unwrap().fingerprint();
        assert_eq!(first, again);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn half_an_identity_is_refused_not_replaced() {
        let dir = folder("half");
        Identity::load_or_create(&dir).unwrap();
        fs::remove_file(dir.join(KEY_FILE)).unwrap();
        assert!(matches!(
            Identity::load_or_create(&dir),
            Err(CertError::Incomplete { .. })
        ));
        assert!(dir.join(CERT_FILE).exists(), "the certificate is kept");
        let _ = fs::remove_dir_all(dir);
    }
}
