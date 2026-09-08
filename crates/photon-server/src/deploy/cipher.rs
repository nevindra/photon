//! Secrets at rest (R8.5): AES-256-GCM under a key that is NOT the session secret. The key
//! lives in a file beside the control-plane DB (`deploy.key`, mode 0600), generated on first
//! start when absent; `[deploy].secret_key_file` points elsewhere for operators who manage
//! keys. A blob is `base64(nonce(12) || ciphertext)`, so a rotated key fails to decrypt loudly
//! rather than returning garbage.
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, Nonce};
use base64::Engine;
use photon_api::deploy::SecretCipher;
use photon_core::PhotonError;
use std::path::Path;

pub struct AesCipher {
    cipher: Aes256Gcm,
}

impl AesCipher {
    /// Load the key file, creating a fresh 32-byte key when it does not exist.
    pub fn open(path: &Path) -> Result<AesCipher, PhotonError> {
        let key_bytes: Vec<u8> = match std::fs::read(path) {
            Ok(b) if b.len() == 32 => b,
            Ok(b) => {
                // Hex or base64 written by an operator.
                let s = String::from_utf8_lossy(&b).trim().to_string();
                if let Ok(h) = hex::decode(&s) {
                    if h.len() == 32 {
                        h
                    } else {
                        return Err(PhotonError::Config(format!(
                            "{}: a deploy secret key must be 32 bytes",
                            path.display()
                        )));
                    }
                } else if let Ok(d) = base64::engine::general_purpose::STANDARD.decode(&s) {
                    if d.len() == 32 {
                        d
                    } else {
                        return Err(PhotonError::Config(format!(
                            "{}: a deploy secret key must be 32 bytes",
                            path.display()
                        )));
                    }
                } else {
                    return Err(PhotonError::Config(format!(
                        "{}: unreadable deploy secret key",
                        path.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| PhotonError::Io(e.to_string()))?;
                    }
                }
                let key = Aes256Gcm::generate_key(OsRng);
                std::fs::write(path, key.as_slice())
                    .map_err(|e| PhotonError::Io(format!("writing {}: {e}", path.display())))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
                }
                eprintln!("photon-server: generated a new deploy secret key at {} — back it up; secrets are unreadable without it", path.display());
                key.to_vec()
            }
            Err(e) => return Err(PhotonError::Io(format!("reading {}: {e}", path.display()))),
        };
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
        Ok(AesCipher {
            cipher: Aes256Gcm::new(key),
        })
    }

    #[cfg(test)]
    pub fn from_key(key: &[u8; 32]) -> AesCipher {
        AesCipher {
            cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key)),
        }
    }
}

impl SecretCipher for AesCipher {
    fn encrypt(&self, plaintext: &str) -> String {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .expect("aes-gcm encrypt");
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        base64::engine::general_purpose::STANDARD.encode(out)
    }

    fn decrypt(&self, blob: &str) -> Result<String, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(blob.trim())
            .map_err(|e| format!("not a secret blob: {e}"))?;
        if bytes.len() < 13 {
            return Err("secret blob too short".into());
        }
        let (n, ct) = bytes.split_at(12);
        let pt = self
            .cipher
            .decrypt(Nonce::from_slice(n), ct)
            .map_err(|_| "secret was written under a different key".to_string())?;
        String::from_utf8(pt).map_err(|_| "secret is not UTF-8".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_wrong_key() {
        let a = AesCipher::from_key(&[7u8; 32]);
        let blob = a.encrypt("hunter2");
        assert_ne!(blob, "hunter2");
        assert_eq!(a.decrypt(&blob).unwrap(), "hunter2");
        let b = AesCipher::from_key(&[8u8; 32]);
        assert!(b.decrypt(&blob).is_err());
        assert!(a.decrypt("nope").is_err());
        // Two encryptions of the same value differ (fresh nonce).
        assert_ne!(a.encrypt("x"), a.encrypt("x"));
    }

    #[test]
    fn key_file_is_generated_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("deploy.key");
        let a = AesCipher::open(&p).unwrap();
        let blob = a.encrypt("s");
        let b = AesCipher::open(&p).unwrap();
        assert_eq!(b.decrypt(&blob).unwrap(), "s");
        assert_eq!(std::fs::read(&p).unwrap().len(), 32);
    }
}
