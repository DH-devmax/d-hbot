use crypto_box::{aead::rand_core::OsRng, SecretKey};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

pub struct Vault {
    root: PathBuf,
    key: SecretKey,
}
impl Vault {
    pub fn open(root: &Path) -> Result<Self, &'static str> {
        let path = root.join("protocol.key");
        let key = match read_private(&path) {
            Ok(bytes) => SecretKey::from(
                <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| "Invalid vault key")?,
            ),
            Err(_) if !path.exists() && !root.join("deployment.sealed").exists() => {
                let key = SecretKey::generate(&mut OsRng);
                write_new(&path, &key.to_bytes())?;
                key
            }
            Err(_) => return Err("Cannot read private vault key"),
        };
        Ok(Self {
            root: root.to_owned(),
            key,
        })
    }
    pub fn read(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, &'static str> {
        let path = self.root.join(name);
        if !path.exists() {
            return Ok(None);
        }
        let sealed = read_private(&path)?;
        self.key
            .unseal(&sealed)
            .map(|v| Some(Zeroizing::new(v)))
            .map_err(|_| "Vault authentication failed")
    }
    pub fn save(&self, name: &str, value: &[u8]) -> Result<(), &'static str> {
        let sealed = self
            .key
            .public_key()
            .seal(&mut OsRng, value)
            .map_err(|_| "Vault encryption failed")?;
        let temporary = self.root.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        write_new(&temporary, &sealed)?;
        if fs::rename(&temporary, self.root.join(name)).is_err() {
            let _ = fs::remove_file(temporary);
            return Err("Vault commit failed");
        }
        #[cfg(unix)]
        fs::File::open(&self.root)
            .and_then(|f| f.sync_all())
            .map_err(|_| "Vault sync failed")?;
        Ok(())
    }
    pub fn clear_session(&self) -> Result<(), &'static str> {
        self.save("session.sealed", b"null")
    }
}
pub fn read_private(path: &Path) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let meta = fs::symlink_metadata(path).map_err(|_| "Private file unavailable")?;
    if !meta.is_file() || meta.len() > 65536 {
        return Err("Invalid private file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err("Private file permissions must be 0600");
        }
    }
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(path)
        .and_then(|f| f.take(65537).read_to_end(&mut bytes))
        .map_err(|_| "Private file read failed")?;
    if bytes.len() > 65536 {
        return Err("Private file too large");
    }
    Ok(bytes)
}
fn write_new(path: &Path, value: &[u8]) -> Result<(), &'static str> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "Private file creation failed")?;
    file.write_all(value)
        .and_then(|_| file.sync_all())
        .map_err(|_| "Private file write failed")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_restart_tamper_and_logout() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.save("session.sealed", b"synthetic-secret").unwrap();
        assert!(!fs::read(dir.path().join("session.sealed"))
            .unwrap()
            .windows(16)
            .any(|v| v == b"synthetic-secret"));
        let restored = Vault::open(dir.path()).unwrap();
        assert_eq!(
            &**restored.read("session.sealed").unwrap().as_ref().unwrap(),
            b"synthetic-secret"
        );
        restored.clear_session().unwrap();
        assert_eq!(
            &**restored.read("session.sealed").unwrap().as_ref().unwrap(),
            b"null"
        );
        let path = dir.path().join("session.sealed");
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 1;
        fs::write(path, bytes).unwrap();
        assert!(restored.read("session.sealed").is_err());
    }
}
