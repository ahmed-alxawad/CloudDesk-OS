use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use zeroize::Zeroizing;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PrivilegedAction {
    SpawnUserWorker {
        uid: u32,
        gid: u32,
        worker: WorkerKind,
    },
    LocalFileOperation {
        uid: u32,
        gid: u32,
        root: String,
        writable: bool,
        operation: clouddesk_vfs::LocalFileOperation,
    },
    OpenTerminalSession {
        uid: u32,
        gid: u32,
        rows: u16,
        cols: u16,
        shell: Option<String>,
    },
    ServiceControl {
        unit: ServiceUnit,
        operation: ServiceOperation,
    },
    Power {
        operation: PowerOperation,
    },
}

impl PrivilegedAction {
    #[must_use]
    pub const fn required_capability(&self) -> &'static str {
        match self {
            Self::SpawnUserWorker { worker, .. } => match worker {
                WorkerKind::IdentityProbe | WorkerKind::Files => "files.local.read",
                WorkerKind::Terminal => "terminal.local.open",
            },
            Self::LocalFileOperation { operation, .. } => {
                if operation.requires_write() {
                    "files.local.write"
                } else {
                    "files.local.read"
                }
            }
            Self::OpenTerminalSession { .. } => "terminal.local.open",
            Self::ServiceControl { .. } => "system.services.manage",
            Self::Power { .. } => "system.power.manage",
        }
    }

    #[must_use]
    pub const fn requires_step_up(&self) -> bool {
        matches!(self, Self::ServiceControl { .. } | Self::Power { .. })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalClientMessage {
    Data { data: Vec<u8> },
    Resize { rows: u16, cols: u16 },
    Close,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalServerMessage {
    Output { data: Vec<u8> },
    Exit { code: u32 },
    Error { message: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerKind {
    IdentityProbe,
    Files,
    Terminal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct ServiceUnit(String);

impl ServiceUnit {
    pub fn new(value: &str) -> Result<Self, GrantError> {
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'@' | b'_' | b'-')
            })
        {
            return Err(GrantError::InvalidServiceUnit);
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ServiceUnit {
    type Error = GrantError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<ServiceUnit> for String {
    fn from(value: ServiceUnit) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceOperation {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerOperation {
    Reboot,
    Shutdown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GrantClaims {
    pub subject_user_id: String,
    pub session_id_hash: String,
    pub action: PrivilegedAction,
    pub issued_at: i64,
    pub expires_at: i64,
    pub nonce: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignedGrant {
    pub claims: GrantClaims,
    pub signature: String,
}

#[derive(Clone)]
pub struct GrantSigner {
    key: Zeroizing<[u8; 32]>,
}

impl GrantSigner {
    pub fn new(key: &[u8]) -> Result<Self, GrantError> {
        Ok(Self {
            key: Zeroizing::new(key.try_into().map_err(|_| GrantError::InvalidKey)?),
        })
    }

    pub fn issue(
        &self,
        subject_user_id: &str,
        session_id_hash: &str,
        action: PrivilegedAction,
        issued_at: i64,
        lifetime_seconds: i64,
    ) -> Result<SignedGrant, GrantError> {
        if !(1..=300).contains(&lifetime_seconds) {
            return Err(GrantError::InvalidLifetime);
        }
        let claims = GrantClaims {
            subject_user_id: subject_user_id.to_owned(),
            session_id_hash: session_id_hash.to_owned(),
            action,
            issued_at,
            expires_at: issued_at + lifetime_seconds,
            nonce: random_nonce(),
        };
        let signature = self.signature(&claims)?;
        Ok(SignedGrant { claims, signature })
    }

    pub fn verify(&self, grant: &SignedGrant, now: i64) -> Result<(), GrantError> {
        if grant.claims.expires_at < now || grant.claims.issued_at > now + 30 {
            return Err(GrantError::Expired);
        }
        if grant.claims.expires_at - grant.claims.issued_at > 300 {
            return Err(GrantError::InvalidLifetime);
        }
        let actual = URL_SAFE_NO_PAD
            .decode(grant.signature.as_bytes())
            .map_err(|_| GrantError::InvalidSignature)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(self.key.as_ref())
            .map_err(|_| GrantError::InvalidKey)?;
        mac.update(&serde_json::to_vec(&grant.claims)?);
        mac.verify_slice(&actual)
            .map_err(|_| GrantError::InvalidSignature)?;
        Ok(())
    }

    fn signature(&self, claims: &GrantClaims) -> Result<String, GrantError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.key.as_ref())
            .map_err(|_| GrantError::InvalidKey)?;
        mac.update(&serde_json::to_vec(claims)?);
        Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
    }
}

fn random_nonce() -> String {
    let mut bytes = [0_u8; 24];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PrivdRequest {
    pub grant: SignedGrant,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PrivdResponse {
    pub accepted: bool,
    pub message: String,
    pub output: Option<serde_json::Value>,
}

#[derive(Debug, Error)]
pub enum GrantError {
    #[error("grant key must be exactly 32 bytes")]
    InvalidKey,
    #[error("grant lifetime must be between 1 and 300 seconds")]
    InvalidLifetime,
    #[error("grant has expired or is not yet valid")]
    Expired,
    #[error("grant signature is invalid")]
    InvalidSignature,
    #[error("service unit name is invalid")]
    InvalidServiceUnit,
    #[error("grant serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action() -> PrivilegedAction {
        PrivilegedAction::SpawnUserWorker {
            uid: 1_000,
            gid: 1_000,
            worker: WorkerKind::IdentityProbe,
        }
    }

    #[test]
    fn signed_grants_are_scoped_short_lived_and_tamper_evident() {
        let signer = GrantSigner::new(&[3_u8; 32]).unwrap();
        let grant = signer
            .issue("user-1", "session-1", action(), 100, 60)
            .unwrap();
        signer.verify(&grant, 120).unwrap();
        assert!(signer.verify(&grant, 161).is_err());

        let mut tampered = grant;
        tampered.claims.action = PrivilegedAction::Power {
            operation: PowerOperation::Reboot,
        };
        assert!(signer.verify(&tampered, 120).is_err());
    }

    #[test]
    fn service_units_cannot_smuggle_shell_syntax() {
        assert!(ServiceUnit::new("sshd.service").is_ok());
        assert!(ServiceUnit::new("sshd; reboot").is_err());
        assert!(ServiceUnit::new("$(reboot)").is_err());
    }
}
