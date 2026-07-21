//! Compile-time separation between the public application and the developer build.
//!
//! The production binary must never become a fixture client because of a stale
//! setting, environment variable, or command argument.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildChannel {
    Production,
    Developer,
}

impl BuildChannel {
    pub const CURRENT: Self = if cfg!(feature = "fixture") {
        Self::Developer
    } else {
        Self::Production
    };

    pub const fn name(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Developer => "developer",
        }
    }

    pub const fn fixture_available(self) -> bool {
        matches!(self, Self::Developer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeEndpoint {
    pub devtools_url: &'static str,
    pub fixture: bool,
}

impl RuntimeEndpoint {
    pub const PRODUCTION: Self = Self {
        devtools_url: "http://127.0.0.1:9222",
        fixture: false,
    };

    #[cfg(feature = "fixture")]
    pub const FIXTURE: Self = Self {
        devtools_url: "http://127.0.0.1:9233",
        fixture: true,
    };

    #[cfg(feature = "fixture")]
    pub fn for_mode(mode: &str) -> Self {
        if mode == "fixture" {
            Self::FIXTURE
        } else {
            Self::PRODUCTION
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_endpoint_is_loopback_9222() {
        assert_eq!(
            RuntimeEndpoint::PRODUCTION.devtools_url,
            "http://127.0.0.1:9222"
        );
    }

    #[cfg(feature = "fixture")]
    #[test]
    fn developer_fixture_endpoint_is_9233_only_in_fixture_mode() {
        assert_eq!(
            RuntimeEndpoint::for_mode("fixture"),
            RuntimeEndpoint::FIXTURE
        );
        assert_eq!(
            RuntimeEndpoint::for_mode("real"),
            RuntimeEndpoint::PRODUCTION
        );
    }
}
