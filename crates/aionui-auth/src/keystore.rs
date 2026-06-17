//! Persistencia cifrada de la semilla de identidad Ed25519 (Alinea Fase 2 #5 — §5.3).
//!
//! La semilla (la **raíz** de la identidad firmada: quien la posea puede forjar el
//! token de cualquier rol) se cifra en reposo con AES-256-GCM —*envelope
//! encryption*, reusando `aionui_common::crypto`— y vive en
//! `<data_dir>/secrets/identity.enc` con permisos `0600`.
//!
//! El KEK **no** se persiste junto al ciphertext: lo deriva el llamador desde un
//! secreto del entorno (deploy keyfile). Así, una fuga del archivo sin el KEK es
//! inútil, y una fuga de la DB no toca el archivo. Los dos factores
//! (filesystem + entorno) deben comprometerse a la vez para recuperar la semilla.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use aionui_common::{decrypt_string, encrypt_string};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

use crate::identity::RequestIdentityService;

#[derive(Debug, thiserror::Error)]
pub enum KeystoreError {
    #[error("keystore io error: {0}")]
    Io(#[from] io::Error),
    #[error("keystore crypto error: {0}")]
    Crypto(String),
    #[error("corrupt identity seed (expected 32 bytes)")]
    CorruptSeed,
    #[error("entropy error")]
    Entropy,
}

/// Carga la semilla de identidad cifrada de `<data_dir>/secrets/identity.enc`, o
/// —si no existe— genera una nueva, la cifra con `kek` y la persiste con `0600`.
///
/// Devuelve siempre un [`RequestIdentityService`] listo para firmar. La clave
/// pública (`public_key_base64`) es estable entre reinicios mientras el archivo y
/// el `kek` no cambien.
pub fn load_or_create_identity(data_dir: &Path, kek: &[u8; 32]) -> Result<RequestIdentityService, KeystoreError> {
    let path = seed_path(data_dir);
    if path.exists() {
        let ct = fs::read_to_string(&path)?;
        let seed_b64 = decrypt_string(ct.trim(), kek).map_err(|e| KeystoreError::Crypto(e.to_string()))?;
        let seed_bytes = B64.decode(seed_b64.trim()).map_err(|_| KeystoreError::CorruptSeed)?;
        let seed: [u8; 32] = seed_bytes.try_into().map_err(|_| KeystoreError::CorruptSeed)?;
        Ok(RequestIdentityService::from_seed(seed))
    } else {
        let (svc, seed) = RequestIdentityService::generate().map_err(|_| KeystoreError::Entropy)?;
        let ct = encrypt_string(&B64.encode(seed), kek).map_err(|e| KeystoreError::Crypto(e.to_string()))?;
        write_secret_file(&path, ct.as_bytes())?;
        Ok(svc)
    }
}

fn seed_path(data_dir: &Path) -> PathBuf {
    data_dir.join("secrets").join("identity.enc")
}

/// Escribe `bytes` en `path`, creando el directorio padre. En Unix fuerza `0700`
/// en el directorio `secrets/` y `0600` en el archivo (sin paso intermedio
/// world-readable: los permisos se fijan en `open`, no después).
fn write_secret_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("alinea-ks-{}-{}", std::process::id(), label));
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn persists_and_reloads_same_pubkey() {
        let dir = tmp("reload");
        let kek = [9u8; 32];
        let a = load_or_create_identity(&dir, &kek).unwrap();
        let b = load_or_create_identity(&dir, &kek).unwrap(); // reload desde disco
        assert_eq!(a.public_key_base64(), b.public_key_base64());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_kek_fails_to_decrypt() {
        let dir = tmp("wrongkek");
        load_or_create_identity(&dir, &[1u8; 32]).unwrap();
        assert!(load_or_create_identity(&dir, &[2u8; 32]).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn seed_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("perms");
        load_or_create_identity(&dir, &[3u8; 32]).unwrap();
        let mode = fs::metadata(seed_path(&dir)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }
}
