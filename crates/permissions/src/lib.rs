use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Capability(&'static str);

impl Capability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

pub const CAPABILITIES: &[Capability] = &[
    Capability("files.local.read"),
    Capability("files.local.write"),
    Capability("files.root.request"),
    Capability("files.permissions.change"),
    Capability("remote.servers.read"),
    Capability("remote.servers.manage"),
    Capability("remote.terminal.open"),
    Capability("transfers.create"),
    Capability("transfers.cancel"),
    Capability("terminal.local.open"),
    Capability("apps.browser.use"),
    Capability("apps.code.use"),
    Capability("apps.office.use"),
    Capability("apps.media.use"),
    Capability("system.services.manage"),
    Capability("system.packages.manage"),
    Capability("system.firewall.manage"),
    Capability("system.power.manage"),
    Capability("users.manage"),
    Capability("roles.manage"),
    Capability("audit.read"),
    Capability("secrets.manage"),
];

#[must_use]
pub fn is_known_capability(value: &str) -> bool {
    CAPABILITIES.iter().any(|capability| capability.0 == value)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn capability_names_are_unique() {
        let unique: HashSet<_> = CAPABILITIES.iter().map(|capability| capability.0).collect();
        assert_eq!(unique.len(), CAPABILITIES.len());
    }

    #[test]
    fn no_generic_privileged_command_capability_exists() {
        assert!(!CAPABILITIES.iter().any(|capability| {
            matches!(
                capability.0,
                "root.shell" | "root.command" | "system.command"
            )
        }));
    }
}
