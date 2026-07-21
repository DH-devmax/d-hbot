use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::build_channel::{BuildChannel, RuntimeEndpoint};
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    pub v3: PathBuf,
    pub database: PathBuf,
    pub secrets: PathBuf,
    pub logs: PathBuf,
    pub legacy_backups: PathBuf,
    pub runtime_mode_file: PathBuf,
}

impl AppPaths {
    pub fn discover() -> AppResult<Self> {
        let root = if let Ok(appdata) = std::env::var("APPDATA") {
            PathBuf::from(appdata).join("DH")
        } else if let Ok(config) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(config).join("DH")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".config").join("DH")
        } else {
            std::env::current_dir()
                .map_err(|error| AppError::new("data_dir", error.to_string()))?
                .join("DH-data")
        };
        #[cfg(feature = "fixture")]
        let mode = std::env::var("DH_RUNTIME_MODE")
            .ok()
            .or_else(|| {
                fs::read_to_string(root.join("developer").join("runtime-mode"))
                    .ok()
                    .map(|value| value.trim().to_string())
            })
            .or_else(|| {
                fs::read_to_string(root.join("runtime-mode"))
                    .ok()
                    .map(|value| value.trim().to_string())
            })
            .unwrap_or_else(|| "real".into())
            .to_ascii_lowercase();
        #[cfg(feature = "fixture")]
        let v3 = select_data_dir(&root, &mode);
        #[cfg(not(feature = "fixture"))]
        let v3 = select_data_dir(&root, "");
        Ok(Self {
            database: v3.join("dh.db"),
            secrets: v3.join("secrets.dat"),
            logs: v3.join("logs"),
            legacy_backups: root.join("legacy-backups"),
            runtime_mode_file: if BuildChannel::CURRENT == BuildChannel::Developer {
                root.join("developer").join("runtime-mode")
            } else {
                root.join("runtime-mode")
            },
            root,
            v3,
        })
    }

    #[cfg(feature = "fixture")]
    pub fn runtime_mode(&self) -> &'static str {
        if self.v3 == self.root.join("fixture") {
            "fixture"
        } else {
            "real"
        }
    }

    #[cfg(not(feature = "fixture"))]
    pub fn runtime_mode(&self) -> &'static str {
        "real"
    }

    #[cfg(feature = "fixture")]
    pub fn default_devtools_url(&self) -> &'static str {
        RuntimeEndpoint::for_mode(self.runtime_mode()).devtools_url
    }

    #[cfg(not(feature = "fixture"))]
    pub fn default_devtools_url(&self) -> &'static str {
        RuntimeEndpoint::PRODUCTION.devtools_url
    }

    #[cfg(feature = "fixture")]
    pub fn set_runtime_mode(&self, mode: &str) -> AppResult<()> {
        let normalized = match mode.trim().to_ascii_lowercase().as_str() {
            "fixture" => "fixture",
            "real" => "real",
            _ => {
                return Err(AppError::new(
                    "runtime_mode",
                    "运行环境只能是 real 或 fixture",
                ))
            }
        };
        fs::create_dir_all(
            self.runtime_mode_file
                .parent()
                .unwrap_or(self.root.as_path()),
        )
        .and_then(|_| fs::write(&self.runtime_mode_file, normalized))
        .map_err(|error| AppError::new("runtime_mode", format!("保存运行环境失败：{error}")))
    }

    pub fn prepare(&self) -> AppResult<Option<PathBuf>> {
        #[cfg(not(feature = "fixture"))]
        {
            fs::create_dir_all(&self.root)
                .and_then(|_| fs::write(self.root.join("runtime-mode"), "real"))
                .map_err(|error| {
                    AppError::new("runtime_mode", format!("恢复生产运行环境失败：{error}"))
                })?;
        }
        #[cfg(feature = "fixture")]
        {
            if !self.runtime_mode_file.exists() {
                fs::create_dir_all(
                    self.runtime_mode_file
                        .parent()
                        .unwrap_or(self.root.as_path()),
                )
                .and_then(|_| fs::write(&self.runtime_mode_file, self.runtime_mode()))
                .map_err(|error| {
                    AppError::new("runtime_mode", format!("迁移开发运行环境设置失败：{error}"))
                })?;
            }
        }
        fs::create_dir_all(&self.v3)
            .and_then(|_| fs::create_dir_all(&self.logs))
            .map_err(|error| {
                AppError::new("data_dir", format!("创建 3.0 数据目录失败：{error}"))
            })?;

        #[cfg(feature = "fixture")]
        if self.runtime_mode() == "fixture" {
            return Ok(None);
        }

        let marker = self.v3.join(".legacy-archived");
        if marker.exists() {
            return Ok(None);
        }

        let legacy_database = self.root.join("dh.db");
        let legacy_secrets = self.root.join("secrets.dat");
        let has_legacy = legacy_database.exists() || legacy_secrets.exists();
        let backup = if has_legacy {
            let folder = self
                .legacy_backups
                .join(Utc::now().format("%Y%m%d-%H%M%S").to_string());
            fs::create_dir_all(&folder).map_err(|error| {
                AppError::new("legacy_backup", format!("创建旧数据备份目录失败：{error}"))
            })?;
            move_if_exists(&legacy_database, &folder.join("dh.db"))?;
            move_if_exists(&legacy_secrets, &folder.join("secrets.dat"))?;
            Some(folder)
        } else {
            None
        };
        fs::write(&marker, Utc::now().to_rfc3339()).map_err(|error| {
            AppError::new("legacy_backup", format!("写入旧数据归档标记失败：{error}"))
        })?;
        Ok(backup)
    }
}

#[cfg(feature = "fixture")]
fn select_data_dir(root: &Path, requested_mode: &str) -> PathBuf {
    if requested_mode == "fixture" {
        root.join("fixture")
    } else {
        root.join("3.0")
    }
}

#[cfg(not(feature = "fixture"))]
fn select_data_dir(root: &Path, _requested_mode: &str) -> PathBuf {
    root.join("3.0")
}

fn move_if_exists(source: &Path, destination: &Path) -> AppResult<()> {
    if !source.exists() {
        return Ok(());
    }
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(source, destination).map_err(|error| {
                AppError::new(
                    "legacy_backup",
                    format!("备份 {} 失败：{error}", source.display()),
                )
            })?;
            fs::remove_file(source).map_err(|error| {
                AppError::new(
                    "legacy_backup",
                    format!("清理 {} 失败：{error}", source.display()),
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_keep_v3_separate_from_legacy_database() {
        let paths = AppPaths {
            root: PathBuf::from("root"),
            v3: PathBuf::from("root/3.0"),
            database: PathBuf::from("root/3.0/dh.db"),
            secrets: PathBuf::from("root/3.0/secrets.dat"),
            logs: PathBuf::from("root/3.0/logs"),
            legacy_backups: PathBuf::from("root/legacy-backups"),
            runtime_mode_file: PathBuf::from("root/runtime-mode"),
        };
        assert_ne!(paths.database, paths.root.join("dh.db"));
    }

    #[cfg(not(feature = "fixture"))]
    #[test]
    fn production_path_ignores_fixture_request() {
        let root = PathBuf::from("root");
        assert_eq!(select_data_dir(&root, "fixture"), root.join("3.0"));
    }

    #[cfg(feature = "fixture")]
    #[test]
    fn developer_path_keeps_fixture_separate() {
        let root = PathBuf::from("root");
        assert_eq!(select_data_dir(&root, "fixture"), root.join("fixture"));
        assert_eq!(select_data_dir(&root, "real"), root.join("3.0"));
    }
}
