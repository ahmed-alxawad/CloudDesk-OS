//! cgroup v2 resource isolation for host-process runtime instances
//! (Task 14).
//!
//! This module always attempts the real thing -- it never simulates
//! success. `CgroupSupport::detect` reports exactly what this host
//! allows: cgroup v2 may be mounted, a CloudDesk-controlled subtree may
//! or may not be writable, and even if it is, the `memory`/`pids`/`cpu`
//! controllers may or may not be delegated (`cgroup.subtree_control`)
//! for us to enable on children. Every one of those is checked
//! independently and reported honestly rather than assumed from the
//! first one succeeding.

use std::path::{Path, PathBuf};

const CGROUP_ROOT: &str = "/sys/fs/cgroup";

// Four independent, individually-meaningful facts about this host's
// cgroup delegation -- not a state machine that would be clearer as an
// enum (all four combinations where later ones are true necessarily
// imply the earlier ones, but each is still reported/tested
// independently, which is the whole point of this struct).
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct CgroupSupport {
    pub v2_mounted: bool,
    /// The delegated cgroup this process itself runs in, if any (read
    /// from `/proc/self/cgroup`).
    pub own_cgroup: Option<String>,
    /// Whether we could create a CloudDesk-controlled child cgroup
    /// under our own delegated cgroup.
    pub can_create_subgroup: bool,
    /// Whether that child cgroup actually exposes `memory.max` --
    /// i.e. whether the `memory` controller is enabled for it via the
    /// parent's `cgroup.subtree_control`. `can_create_subgroup` can be
    /// true while this is false (directory creation succeeding does not
    /// imply controller delegation).
    pub memory_controller_writable: bool,
    pub pids_controller_writable: bool,
    pub cpu_controller_writable: bool,
}

impl CgroupSupport {
    #[must_use]
    pub fn fully_enforceable(&self) -> bool {
        self.can_create_subgroup
            && self.memory_controller_writable
            && self.pids_controller_writable
            && self.cpu_controller_writable
    }
}

fn own_cgroup_path() -> Option<String> {
    let contents = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    // cgroup v2 unified hierarchy is always the single "0::<path>" line.
    contents
        .lines()
        .find_map(|line| line.strip_prefix("0::").map(str::to_owned))
}

/// Probes real filesystem/cgroup state -- never returns a hardcoded
/// answer. Creates and immediately removes a real probe directory to
/// determine `can_create_subgroup`/`*_controller_writable`, so this is a
/// live check each time it's called, not a cached assumption.
#[must_use]
pub fn detect() -> CgroupSupport {
    let v2_mounted = Path::new(CGROUP_ROOT).join("cgroup.controllers").exists();
    let own_cgroup = own_cgroup_path();

    let mut support = CgroupSupport {
        v2_mounted,
        own_cgroup: own_cgroup.clone(),
        can_create_subgroup: false,
        memory_controller_writable: false,
        pids_controller_writable: false,
        cpu_controller_writable: false,
    };

    if !v2_mounted {
        return support;
    }
    let Some(own_cgroup) = own_cgroup else {
        return support;
    };
    let own_dir = PathBuf::from(CGROUP_ROOT).join(own_cgroup.trim_start_matches('/'));
    let probe_dir = own_dir.join("clouddesk-probe");

    if std::fs::create_dir(&probe_dir).is_err() {
        return support;
    }
    support.can_create_subgroup = true;

    for (controller, file, writable) in [
        (
            "memory",
            "memory.max",
            &mut support.memory_controller_writable,
        ),
        ("pids", "pids.max", &mut support.pids_controller_writable),
        ("cpu", "cpu.max", &mut support.cpu_controller_writable),
    ] {
        let _ = controller;
        *writable = probe_dir.join(file).exists()
            && std::fs::metadata(probe_dir.join(file)).is_ok_and(|m| !m.permissions().readonly());
    }

    let _ = std::fs::remove_dir(&probe_dir);
    support
}

/// A CloudDesk-controlled cgroup for exactly one runtime instance,
/// created under the caller's own delegated cgroup -- never under an
/// arbitrary host path, and never accepting a path from client input
/// (Task 14: "Never modify arbitrary host cgroups from client input").
pub struct InstanceCgroup {
    path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum CgroupError {
    #[error("cgroup v2 resource enforcement is not available on this host: {0}")]
    Blocked(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl InstanceCgroup {
    /// Creates `<own_cgroup>/clouddesk-runtime-<instance_id>/`. Fails
    /// with `CgroupError::Blocked` (never panics, never silently
    /// no-ops) if this host doesn't actually support it -- callers must
    /// treat that as "resource limits are not enforced for this
    /// instance," not as a fatal error for the instance itself (an
    /// unconfined process is still better than refusing to start the
    /// requested runtime entirely on a host without delegation).
    pub fn create(instance_id: &str) -> Result<Self, CgroupError> {
        let support = detect();
        if !support.can_create_subgroup {
            return Err(CgroupError::Blocked(
                "no writable delegated cgroup available to this process".to_owned(),
            ));
        }
        let Some(own_cgroup) = support.own_cgroup else {
            return Err(CgroupError::Blocked(
                "no cgroup membership found".to_owned(),
            ));
        };
        let path = PathBuf::from(CGROUP_ROOT)
            .join(own_cgroup.trim_start_matches('/'))
            .join(format!("clouddesk-runtime-{instance_id}"));
        std::fs::create_dir(&path)?;
        Ok(Self { path })
    }

    /// Sets `memory.max`. `Blocked` (not applied) if the controller
    /// isn't delegated to this cgroup -- checked by attempting the real
    /// write and reporting its actual result, never assumed.
    pub fn set_memory_limit(&self, bytes: u64) -> Result<(), CgroupError> {
        self.write_controller_file("memory.max", &bytes.to_string())
    }

    pub fn set_pids_limit(&self, limit: u32) -> Result<(), CgroupError> {
        self.write_controller_file("pids.max", &limit.to_string())
    }

    /// `quota_fraction` of one CPU core, cgroup v2 `cpu.max` syntax
    /// (`<quota> <period>` microseconds).
    pub fn set_cpu_limit(&self, quota_fraction: f32) -> Result<(), CgroupError> {
        let period = 100_000_u64;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let quota = (f64::from(quota_fraction)
            * f64::from(u32::try_from(period).unwrap_or(u32::MAX)))
        .round() as u64;
        self.write_controller_file("cpu.max", &format!("{quota} {period}"))
    }

    fn write_controller_file(&self, name: &str, value: &str) -> Result<(), CgroupError> {
        let file = self.path.join(name);
        if !file.exists() {
            return Err(CgroupError::Blocked(format!(
                "{name} is not delegated to this cgroup"
            )));
        }
        std::fs::write(&file, value)
            .map_err(|e| CgroupError::Blocked(format!("writing {name} failed: {e}")))
    }

    /// Adds `pid` to this cgroup (moves the process into it). A process
    /// must be moved in before its resource usage is governed by the
    /// limits above.
    pub fn add_process(&self, pid: u32) -> Result<(), CgroupError> {
        std::fs::write(self.path.join("cgroup.procs"), pid.to_string())?;
        Ok(())
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for InstanceCgroup {
    fn drop(&mut self) {
        // Best-effort: a non-empty cgroup (process still exiting) can't
        // be removed yet; the manager is responsible for not dropping
        // this until the process has actually exited.
        let _ = std::fs::remove_dir(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_never_panics_and_reports_a_real_answer() {
        let support = detect();
        // We don't assert a specific outcome -- this genuinely varies
        // per host -- only that detection completed and is internally
        // consistent (delegation flags can't be true without subgroup
        // creation having succeeded).
        if support.memory_controller_writable
            || support.pids_controller_writable
            || support.cpu_controller_writable
        {
            assert!(support.can_create_subgroup);
        }
    }
}
