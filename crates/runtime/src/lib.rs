use clouddesk_permissions::is_known_capability;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppManifest {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub route: String,
    pub required_permissions: Vec<String>,
    #[serde(default)]
    pub file_associations: Vec<String>,
    pub runtime_dependency: Option<RuntimeDependency>,
    pub enabled: bool,
}

impl AppManifest {
    pub fn from_json(contents: &str) -> Result<Self, ManifestError> {
        let manifest: Self = serde_json::from_str(contents)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if !is_identifier(&self.id) {
            return Err(ManifestError::InvalidId(self.id.clone()));
        }
        if !self.route.starts_with('/') {
            return Err(ManifestError::InvalidRoute(self.route.clone()));
        }
        if let Some(permission) = self
            .required_permissions
            .iter()
            .find(|permission| !is_known_capability(permission))
        {
            return Err(ManifestError::UnknownCapability(permission.clone()));
        }
        Ok(())
    }
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeDependency {
    Browser,
    Code,
    Office,
    Media,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("application manifest is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid application id: {0}")]
    InvalidId(String),
    #[error("application route must begin with '/': {0}")]
    InvalidRoute(String),
    #[error("unknown capability in application manifest: {0}")]
    UnknownCapability(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_builtin_manifest() {
        let manifests = [
            include_str!("../../../apps/web/public/manifests/files.json"),
            include_str!("../../../apps/web/public/manifests/gallery.json"),
            include_str!("../../../apps/web/public/manifests/terminal.json"),
            include_str!("../../../apps/web/public/manifests/servers.json"),
            include_str!("../../../apps/web/public/manifests/transfers.json"),
            include_str!("../../../apps/web/public/manifests/settings.json"),
            include_str!("../../../apps/web/public/manifests/browser.json"),
            include_str!("../../../apps/web/public/manifests/code.json"),
            include_str!("../../../apps/web/public/manifests/office.json"),
            include_str!("../../../apps/web/public/manifests/video.json"),
        ];
        let parsed: Vec<_> = manifests
            .into_iter()
            .map(AppManifest::from_json)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(parsed.len(), 10);
        assert!(parsed.iter().all(|manifest| manifest.enabled));
    }

    #[test]
    fn rejects_frontend_only_permission_names() {
        let manifest = AppManifest {
            id: "unsafe".to_owned(),
            name: "Unsafe".to_owned(),
            icon: "warning".to_owned(),
            route: "/unsafe".to_owned(),
            required_permissions: vec!["root.shell".to_owned()],
            file_associations: vec![],
            runtime_dependency: None,
            enabled: false,
        };

        assert!(matches!(
            manifest.validate(),
            Err(ManifestError::UnknownCapability(_))
        ));
    }
}
