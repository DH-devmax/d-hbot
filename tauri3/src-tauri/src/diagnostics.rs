use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use chrono::Local;
use regex::Regex;

#[derive(Clone)]
pub struct Logger {
    directory: PathBuf,
}

impl Logger {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn write(&self, level: &str, message: &str) {
        if fs::create_dir_all(&self.directory).is_err() {
            return;
        }
        let path = self
            .directory
            .join(format!("dh-{}.log", Local::now().format("%Y%m%d")));
        let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        let _ = writeln!(
            file,
            "{} [{}] {}",
            Local::now().to_rfc3339(),
            level,
            redact(message)
        );
    }
}

pub fn install_panic_hook(directory: PathBuf) {
    std::panic::set_hook(Box::new(move |panic| {
        let logger = Logger::new(directory.clone());
        let location = panic
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "unknown".into());
        let message = panic
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("panic");
        logger.write("FATAL", &format!("{location} {message}"));
    }));
}

pub fn redact(message: &str) -> String {
    let patterns = [
        r#"(?i)(authorization\s*[:=]\s*)([^\r\n]+)"#,
        r#"(?i)(bearer\s+)([a-z0-9._~+/-]+)"#,
        r#"(?i)(\"?(?:api[_-]?key|token|cookie|secret)\"?\s*[:=]\s*\"?)([^\"\s,;}]+)"#,
        r#"(?i)(sk-)[a-z0-9_-]{12,}"#,
    ];
    let mut result = message.to_string();
    for pattern in patterns {
        if let Ok(regex) = Regex::new(pattern) {
            result = if pattern.contains("sk-") {
                regex.replace_all(&result, "sk-***").into_owned()
            } else {
                regex.replace_all(&result, "${1}***").into_owned()
            };
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removes_common_secret_shapes() {
        let key = ["sk", "-", "abcdefghijklmnopqrstuvwxyz"].concat();
        let value = redact(
            &[
                "Author",
                "ization: Bearer ",
                "TOKEN apiKey=",
                &key,
                " cookie=session123",
            ]
            .concat(),
        );
        assert!(!value.contains("TOKEN"));
        assert!(!value.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!value.contains("session123"));
    }
}
