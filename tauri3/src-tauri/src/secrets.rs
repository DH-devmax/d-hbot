use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct SecretStore {
    path: PathBuf,
}

impl SecretStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> AppResult<BTreeMap<String, String>> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new())
            }
            Err(error) => {
                return Err(AppError::new(
                    "secret_read",
                    format!("读取密钥文件失败：{error}"),
                ))
            }
        };
        let plain = unprotect(&bytes).map_err(|error| AppError::new("secret_read", error))?;
        serde_json::from_slice(&plain)
            .map_err(|error| AppError::new("secret_read", format!("解析密钥文件失败：{error}")))
    }

    pub fn save(&self, values: &BTreeMap<String, String>) -> AppResult<()> {
        let plain = serde_json::to_vec(values)
            .map_err(|error| AppError::new("secret_write", error.to_string()))?;
        let protected = protect(&plain).map_err(|error| AppError::new("secret_write", error))?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| AppError::new("secret_write", error.to_string()))?;
        }
        let temporary = self.path.with_extension("tmp");
        fs::write(&temporary, protected)
            .map_err(|error| AppError::new("secret_write", error.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
                .map_err(|error| AppError::new("secret_write", error.to_string()))?;
        }
        fs::rename(&temporary, &self.path)
            .map_err(|error| AppError::new("secret_write", error.to_string()))
    }
}

#[cfg(windows)]
fn protect(value: &[u8]) -> Result<Vec<u8>, String> {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};

    let input = CRYPT_INTEGER_BLOB {
        cbData: value.len() as u32,
        pbData: value.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            1,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("Windows DPAPI 加密失败".into());
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}

#[cfg(windows)]
fn unprotect(value: &[u8]) -> Result<Vec<u8>, String> {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let input = CRYPT_INTEGER_BLOB {
        cbData: value.len() as u32,
        pbData: value.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            1,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("Windows DPAPI 解密失败".into());
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}

#[cfg(not(windows))]
fn protect(value: &[u8]) -> Result<Vec<u8>, String> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD
        .encode(value)
        .into_bytes())
}

#[cfg(not(windows))]
fn unprotect(value: &[u8]) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|error| format!("开发环境密钥解码失败：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn saves_and_loads_secret_map() {
        let directory = tempdir().unwrap();
        let store = SecretStore::new(directory.path().join("secrets.dat"));
        let mut values = BTreeMap::new();
        values.insert("ai.api_key".into(), "TOKEN".into());
        store.save(&values).unwrap();
        assert_eq!(
            store.load().unwrap().get("ai.api_key").map(String::as_str),
            Some("TOKEN")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
