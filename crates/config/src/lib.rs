use std::{fs, net::IpAddr, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_PORT: u16 = 9870;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub tls: TlsConfig,
    pub security: SecurityConfig,
    pub privilege: PrivilegeConfig,
    pub database: DatabaseConfig,
    pub web: WebConfig,
    pub media: MediaConfig,
    pub runtime: RuntimeConfig,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path).map_err(ConfigError::Read)?;
        Self::from_toml(&contents)
    }

    pub fn from_toml(contents: &str) -> Result<Self, ConfigError> {
        toml::from_str(contents).map_err(ConfigError::Parse)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub address: IpAddr,
    pub port: u16,
    pub development_http: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            address: [0, 0, 0, 0].into(),
            port: DEFAULT_PORT,
            development_http: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TlsConfig {
    pub certificate: String,
    pub private_key: String,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            certificate: "/etc/clouddesk/tls/server.crt".to_owned(),
            private_key: "/etc/clouddesk/tls/server.key".to_owned(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "sqlite://var/clouddesk.db".to_owned(),
            max_connections: 5,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecurityConfig {
    pub master_key: String,
    pub bootstrap_secret: String,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            master_key: "/etc/clouddesk/keys/master.key".to_owned(),
            bootstrap_secret: "/var/lib/clouddesk/bootstrap.secret".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrivilegeConfig {
    pub enabled: bool,
    pub socket: String,
    pub grant_key: String,
}

impl Default for PrivilegeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            socket: "/run/clouddesk/privd.sock".to_owned(),
            grant_key: "/etc/clouddesk/keys/privd-grant.key".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebConfig {
    pub static_dir: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            static_dir: "apps/web/dist".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaConfig {
    /// Root directory for per-job remux/transcode workspaces. Each job
    /// gets its own unpredictably-named, `0700`-permissioned
    /// subdirectory here (see `clouddesk_media::exec::job_workspace`).
    pub cache_dir: String,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            cache_dir: "/var/lib/clouddesk/media-cache".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    /// Server-owned root all optional-runtime (Code/Office/Browser)
    /// instance storage lives under (Phase 6). Absence of this
    /// directory being writable does not prevent `clouddeskd` from
    /// starting -- every runtime kind simply reports `Unavailable`
    /// until it is (Task 36).
    pub state_dir: String,
    /// Trusted, version-pinned `code-server` OCI image reference
    /// (Phase 7 Task 33) -- never a request-supplied value. Pinned to
    /// an immutable content digest, not just a mutable tag (Phase 7
    /// closure Task 14): `codercom/code-server:4.133.0` was pulled and
    /// verified during this closure pass to resolve to digest
    /// `sha256:e073a441c61c85821a7f16b64cf93b4e77b4092899bb1f3bed906fbd558afd62`
    /// (confirmed via `docker inspect --format '{{index .RepoDigests 0}}'`
    /// and re-verified runnable via `docker run
    /// codercom/code-server@sha256:e073...` -> `4.133.0 ... with Code
    /// 1.133.0`). A digest reference cannot be silently retagged to
    /// different content the way a tag can; see `PHASE7_CODE_EVIDENCE.md`.
    pub code_image: String,
    /// Trusted, version-pinned Collabora Online image reference (Phase
    /// 8 Task 2/14/60) -- never a request-supplied value. This is
    /// **CODE, the development/test edition**, not a claim about
    /// Collabora's recommended production deployment; see
    /// `PHASE8_OFFICE_EVIDENCE.md` and `docs/THIRD_PARTY_NOTICES.md`.
    /// Pinned to `collabora/code:26.04.3.1.1`'s immutable content
    /// digest (confirmed via `docker inspect --format
    /// '{{index .RepoDigests 0}}'`), not just the mutable tag.
    /// Administrators may instead configure an external, already-
    /// supported Collabora Online deployment (Task 1/61) --
    /// `office_external_url`, below.
    pub office_image: String,
    /// Administrator-configured external Collabora Online endpoint
    /// (Task 1/61), used instead of the managed OCI runtime when set.
    /// Only an administrator can set this (enforced at the settings
    /// API layer, not here); this struct only carries the already-
    /// validated value.
    pub office_external_url: Option<String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            state_dir: "/var/lib/clouddesk/runtimes".to_owned(),
            code_image: "codercom/code-server@sha256:e073a441c61c85821a7f16b64cf93b4e77b4092899bb1f3bed906fbd558afd62".to_owned(),
            office_image: "collabora/code@sha256:6b70f91f0b6e9c76f75f162f58ef0a12cf9415d78e14713d33c0318ddc4a2cc0".to_owned(),
            office_external_url: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read configuration: {0}")]
    Read(std::io::Error),
    #[error("configuration is invalid: {0}")]
    Parse(toml::de::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_architecture_listener() {
        let config = Config::default();
        assert_eq!(config.server.address, IpAddr::from([0, 0, 0, 0]));
        assert_eq!(config.server.port, DEFAULT_PORT);
    }

    #[test]
    fn rejects_unknown_configuration_keys() {
        let error = Config::from_toml("[server]\nroot_shell = true").unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }
}
