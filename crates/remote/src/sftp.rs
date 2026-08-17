use std::path::{Component, Path, PathBuf};

use clouddesk_vfs::{EntryKind, ProviderFeature, VfsEntry, VfsError, VfsProvider};
use russh_sftp::client::SftpSession;
use tokio::runtime::Handle;

pub struct SftpProvider {
    session: SftpSession,
    handle: Handle,
}

impl SftpProvider {
    #[must_use]
    pub fn new(session: SftpSession, handle: Handle) -> Self {
        Self { session, handle }
    }

    fn normalize_virtual_path(path: &str) -> Result<PathBuf, VfsError> {
        if path.as_bytes().contains(&0) {
            return Err(VfsError::InvalidPath);
        }
        let mut normalized = PathBuf::new();
        for component in Path::new(path).components() {
            match component {
                Component::Normal(value) => normalized.push(value),
                Component::CurDir | Component::RootDir => {}
                Component::ParentDir | Component::Prefix(_) => return Err(VfsError::Traversal),
            }
        }
        if normalized.as_os_str().is_empty() {
            normalized.push(".");
        }
        Ok(normalized)
    }

    fn to_vfs_entry(
        stat: &russh_sftp::protocol::FileAttributes,
        name: String,
        path: String,
    ) -> VfsEntry {
        let kind = if stat.is_dir() {
            EntryKind::Directory
        } else if stat.is_symlink() {
            EntryKind::Symlink
        } else if stat.is_regular() {
            EntryKind::File
        } else {
            EntryKind::Other
        };

        VfsEntry {
            name,
            path,
            kind,
            size: stat.size.unwrap_or(0),
            modified_at: stat.mtime.map(i64::from),
            mode: stat.permissions.unwrap_or(0),
            uid: stat.uid.unwrap_or(0),
            gid: stat.gid.unwrap_or(0),
        }
    }
}

impl VfsProvider for SftpProvider {
    fn capabilities(&self) -> Vec<ProviderFeature> {
        vec![
            ProviderFeature::Read,
            ProviderFeature::Write,
            ProviderFeature::Trash,
            ProviderFeature::Permissions,
            ProviderFeature::Ownership,
        ]
    }

    fn list(&self, path: &str) -> Result<Vec<VfsEntry>, VfsError> {
        let normalized = Self::normalize_virtual_path(path)?;
        let remote_path_str = if normalized == Path::new(".") {
            ".".to_string()
        } else {
            normalized
                .to_str()
                .ok_or(VfsError::InvalidPath)?
                .to_string()
        };

        let handle = self.handle.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let mut results = Vec::new();
                let dir_entries = self
                    .session
                    .read_dir(&remote_path_str)
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                for entry in dir_entries {
                    let name = entry.file_name();
                    if name == "." || name == ".." {
                        continue;
                    }
                    let child_path = if remote_path_str == "." {
                        format!("/{name}")
                    } else if remote_path_str.starts_with('/') {
                        format!("{remote_path_str}/{name}")
                    } else {
                        format!("/{remote_path_str}/{name}")
                    };

                    let stat = self
                        .session
                        .metadata(&child_path)
                        .await
                        .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                    results.push(Self::to_vfs_entry(&stat, name.clone(), child_path));
                }
                Ok(results)
            })
        })
    }

    fn stat(&self, path: &str) -> Result<VfsEntry, VfsError> {
        let normalized = Self::normalize_virtual_path(path)?;
        let remote_path_str = if normalized == Path::new(".") {
            ".".to_string()
        } else {
            normalized
                .to_str()
                .ok_or(VfsError::InvalidPath)?
                .to_string()
        };

        let handle = self.handle.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let stat = self
                    .session
                    .metadata(&remote_path_str)
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                let name = normalized
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let vfs_path = if remote_path_str == "." {
                    "/".to_string()
                } else if remote_path_str.starts_with('/') {
                    remote_path_str.clone()
                } else {
                    format!("/{remote_path_str}")
                };
                Ok(Self::to_vfs_entry(&stat, name, vfs_path))
            })
        })
    }

    fn create_directory(&self, path: &str) -> Result<(), VfsError> {
        let normalized = Self::normalize_virtual_path(path)?;
        let remote_path = normalized.to_str().unwrap_or(".").to_string();
        let handle = self.handle.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                self.session
                    .create_dir(remote_path)
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))
            })
        })
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), VfsError> {
        let n_from = Self::normalize_virtual_path(from)?
            .to_str()
            .unwrap_or(".")
            .to_string();
        let n_to = Self::normalize_virtual_path(to)?
            .to_str()
            .unwrap_or(".")
            .to_string();
        let handle = self.handle.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                self.session
                    .rename(n_from, n_to)
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))
            })
        })
    }

    fn copy_file(&self, from: &str, to: &str) -> Result<u64, VfsError> {
        let n_from = Self::normalize_virtual_path(from)?
            .to_str()
            .unwrap_or(".")
            .to_string();
        let n_to = Self::normalize_virtual_path(to)?
            .to_str()
            .unwrap_or(".")
            .to_string();
        let handle = self.handle.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                // SFTP doesn't have native copy, we must read then write
                let data = self
                    .session
                    .read(n_from)
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                self.session
                    .write(n_to, &data)
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                Ok(data.len() as u64)
            })
        })
    }

    fn trash(&self, path: &str) -> Result<String, VfsError> {
        // We will just do a hard delete for SFTP since there is no native trash
        let normalized = Self::normalize_virtual_path(path)?;
        let remote_path = normalized.to_str().unwrap_or(".").to_string();
        let handle = self.handle.clone();

        // Check if it's a dir or file first
        let is_dir = self.stat(path)?.kind == EntryKind::Directory;

        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                if is_dir {
                    self.session.remove_dir(remote_path).await
                } else {
                    self.session.remove_file(remote_path).await
                }
                .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                Ok("deleted".to_string())
            })
        })
    }

    fn read_limited(&self, path: &str, maximum_bytes: usize) -> Result<Vec<u8>, VfsError> {
        let normalized = Self::normalize_virtual_path(path)?;
        let remote_path = normalized.to_str().unwrap_or(".").to_string();
        let handle = self.handle.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let mut data = self
                    .session
                    .read(remote_path)
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                if data.len() > maximum_bytes {
                    data.truncate(maximum_bytes);
                }
                Ok(data)
            })
        })
    }

    fn write_file(&self, path: &str, content: &[u8]) -> Result<u64, VfsError> {
        let normalized = Self::normalize_virtual_path(path)?;
        let remote_path = normalized.to_str().unwrap_or(".").to_string();
        let handle = self.handle.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                self.session
                    .write(remote_path, content)
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                Ok(content.len() as u64)
            })
        })
    }

    fn chmod(&self, _path: &str, _mode: u32) -> Result<(), VfsError> {
        Err(VfsError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Not supported",
        )))
    }

    fn search(
        &self,
        _path: &str,
        _query: &str,
        _maximum_results: usize,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        Err(VfsError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Not supported",
        )))
    }
}
