use std::{
    ffi::CString,
    fs,
    path::{Path, PathBuf},
};

use nix::unistd::{getgrouplist, Gid, Uid, User};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LinuxIdentity {
    pub username: String,
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
    pub home: PathBuf,
    pub shell: PathBuf,
}

pub fn lookup_user(username: &str) -> Result<Option<LinuxIdentity>, LinuxError> {
    if username.as_bytes().contains(&0) {
        return Err(LinuxError::InvalidUsername);
    }
    let Some(user) = User::from_name(username)? else {
        return Ok(None);
    };
    let c_username = CString::new(username).map_err(|_| LinuxError::InvalidUsername)?;
    let mut groups: Vec<u32> = getgrouplist(&c_username, user.gid)?
        .into_iter()
        .map(Gid::as_raw)
        .collect();
    groups.sort_unstable();
    groups.dedup();
    Ok(Some(LinuxIdentity {
        username: user.name,
        uid: user.uid.as_raw(),
        gid: user.gid.as_raw(),
        groups,
        home: user.dir,
        shell: user.shell,
    }))
}

pub fn lookup_uid(uid: u32) -> Result<Option<LinuxIdentity>, LinuxError> {
    let Some(user) = User::from_uid(Uid::from_raw(uid))? else {
        return Ok(None);
    };
    lookup_user(&user.name)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessMode {
    Read,
    ReadWrite,
}

#[derive(Clone, Debug)]
pub struct AssignedRoot {
    canonical_path: PathBuf,
    pub access_mode: AccessMode,
}

impl AssignedRoot {
    pub fn new(path: impl AsRef<Path>, access_mode: AccessMode) -> Result<Self, LinuxError> {
        let canonical_path = fs::canonicalize(path).map_err(LinuxError::Canonicalize)?;
        if !canonical_path.is_absolute() {
            return Err(LinuxError::RootNotAbsolute);
        }
        Ok(Self {
            canonical_path,
            access_mode,
        })
    }

    pub fn authorize_existing(
        &self,
        candidate: impl AsRef<Path>,
        write: bool,
    ) -> Result<PathBuf, LinuxError> {
        if write && self.access_mode != AccessMode::ReadWrite {
            return Err(LinuxError::ReadOnlyRoot);
        }
        let canonical = fs::canonicalize(candidate).map_err(LinuxError::Canonicalize)?;
        if !canonical.starts_with(&self.canonical_path) {
            return Err(LinuxError::OutsideAssignedRoot);
        }
        Ok(canonical)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.canonical_path
    }
}

#[derive(Debug, Error)]
pub enum LinuxError {
    #[error("Linux identity lookup failed: {0}")]
    Nix(#[from] nix::Error),
    #[error("username contains an invalid NUL byte")]
    InvalidUsername,
    #[error("assigned root must be absolute")]
    RootNotAbsolute,
    #[error("could not canonicalize path: {0}")]
    Canonicalize(std::io::Error),
    #[error("path escapes the assigned root")]
    OutsideAssignedRoot,
    #[error("assigned root is read-only")]
    ReadOnlyRoot,
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use nix::unistd::Group;

    use super::*;

    #[test]
    fn assigned_roots_reject_traversal_and_symlink_escape() {
        let temp = tempfile::tempdir().unwrap();
        let root_path = temp.path().join("assigned");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root_path).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret"), "secret").unwrap();
        symlink(&outside, root_path.join("escape")).unwrap();
        let root = AssignedRoot::new(&root_path, AccessMode::ReadWrite).unwrap();

        assert!(root
            .authorize_existing(root_path.join("escape/secret"), false)
            .is_err());
        assert!(root
            .authorize_existing(outside.join("secret"), false)
            .is_err());
    }

    #[test]
    fn read_only_roots_reject_write_authorization() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, "data").unwrap();
        let root = AssignedRoot::new(temp.path(), AccessMode::Read).unwrap();
        assert!(root.authorize_existing(&file, false).is_ok());
        assert!(matches!(
            root.authorize_existing(&file, true),
            Err(LinuxError::ReadOnlyRoot)
        ));
    }

    #[test]
    fn system_user_lookup_preserves_uid_and_primary_gid() {
        let uid = 0;
        let identity = lookup_uid(uid).unwrap().unwrap();
        assert_eq!(identity.uid, uid);
        assert!(Group::from_gid(Gid::from_raw(identity.gid))
            .unwrap()
            .is_some());
    }
}
