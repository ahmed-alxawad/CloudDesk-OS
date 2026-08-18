//! Safe, predictable storage layout for runtime state (Task 6).
//!
//! ```text
//! runtime-root/
//!   code/
//!     users/<owner_user_id>/<instance_id>/
//!   office/
//!     users/<owner_user_id>/<instance_id>/
//!   browser/
//!     users/<owner_user_id>/<instance_id>/
//! ```
//!
//! Every path segment below `runtime-root` is either a fixed literal
//! (the kind name) or a server-generated identifier
//! (`owner_user_id`/`instance_id`, both already validated identifier-safe
//! strings from `clouddesk_auth`) -- never a client-supplied path
//! fragment, so there is no path-traversal surface here at all.

use crate::model::{InstanceId, RuntimeKind};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("invalid identifier segment: {0}")]
    InvalidSegment(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("refusing to use a symlink as a runtime state directory: {0}")]
    SymlinkRejected(PathBuf),
}

/// Identifiers used to build storage paths must be plain
/// `[a-zA-Z0-9_-]` -- this is a defense-in-depth check, not the primary
/// guarantee (identifiers are always server-generated via
/// `clouddesk_auth::random_identifier` or the database's own user ids,
/// never taken from a request path/body verbatim), but it means even a
/// future bug that let one through cannot smuggle a `..` or `/`.
fn is_safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Creates (if needed) and returns `runtime_root/<kind>/users/<owner>/<instance>`.
/// Refuses to proceed through an existing symlink at any level (created
/// component-by-component with an explicit symlink check, mirroring the
/// discipline already used by `clouddesk-vfs` for archive extraction).
pub fn instance_state_dir(runtime_root: &Path, id: &InstanceId) -> Result<PathBuf, StorageError> {
    if !is_safe_segment(&id.owner_user_id) {
        return Err(StorageError::InvalidSegment(id.owner_user_id.clone()));
    }
    if !is_safe_segment(&id.instance_id) {
        return Err(StorageError::InvalidSegment(id.instance_id.clone()));
    }
    let mut path = runtime_root.to_path_buf();
    create_dir_symlink_safe(&path)?;
    for segment in [
        kind_dir_name(id.kind),
        "users",
        id.owner_user_id.as_str(),
        id.instance_id.as_str(),
    ] {
        path.push(segment);
        create_dir_symlink_safe(&path)?;
    }
    Ok(path)
}

fn kind_dir_name(kind: RuntimeKind) -> &'static str {
    kind.as_str()
}

/// Creates `path` as a directory (0700) if it doesn't exist. If
/// something already exists at `path`, it must be a real directory --
/// a symlink (or any other file type) is rejected outright rather than
/// followed, so a pre-planted symlink can never redirect runtime state
/// outside `runtime_root`.
fn create_dir_symlink_safe(path: &Path) -> Result<(), StorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(StorageError::SymlinkRejected(path.to_path_buf()));
            }
            if !meta.is_dir() {
                return Err(StorageError::Io(std::io::Error::other(format!(
                    "{} exists and is not a directory",
                    path.display()
                ))));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(e) => Err(StorageError::Io(e)),
    }
}

/// Removes an instance's state directory entirely -- only ever called
/// for `Persistence::Ephemeral` instances (Task 7/8).
pub fn remove_instance_state_dir(runtime_root: &Path, id: &InstanceId) -> Result<(), StorageError> {
    let dir = instance_state_dir(runtime_root, id)?;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(_) if !dir.exists() => Ok(()),
        Err(e) => Err(StorageError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(owner: &str, instance: &str) -> InstanceId {
        InstanceId {
            kind: RuntimeKind::TestFixture,
            owner_user_id: owner.to_owned(),
            instance_id: instance.to_owned(),
        }
    }

    #[test]
    fn rejects_traversal_looking_identifiers() {
        let root = tempfile::tempdir().unwrap();
        assert!(instance_state_dir(root.path(), &id("../../etc", "x")).is_err());
        assert!(instance_state_dir(root.path(), &id("x", "../../etc")).is_err());
        assert!(instance_state_dir(root.path(), &id("a/b", "x")).is_err());
    }

    #[test]
    fn creates_the_expected_layout_with_restrictive_permissions() {
        let root = tempfile::tempdir().unwrap();
        let dir = instance_state_dir(root.path(), &id("user1", "inst1")).unwrap();
        assert!(dir.ends_with("test_fixture/users/user1/inst1"));
        assert!(dir.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn refuses_to_follow_a_preplanted_symlink() {
        let root = tempfile::tempdir().unwrap();
        let kind_dir = root.path().join("test_fixture");
        std::fs::create_dir_all(&kind_dir).unwrap();
        let escape_target = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(escape_target.path(), kind_dir.join("users")).unwrap();

        let result = instance_state_dir(root.path(), &id("user1", "inst1"));
        assert!(matches!(result, Err(StorageError::SymlinkRejected(_))));
    }

    #[test]
    fn remove_instance_state_dir_deletes_only_that_instance() {
        let root = tempfile::tempdir().unwrap();
        let a = instance_state_dir(root.path(), &id("user1", "inst1")).unwrap();
        let b = instance_state_dir(root.path(), &id("user1", "inst2")).unwrap();
        std::fs::write(a.join("marker"), b"x").unwrap();
        remove_instance_state_dir(root.path(), &id("user1", "inst1")).unwrap();
        assert!(!a.exists());
        assert!(b.exists());
    }
}
