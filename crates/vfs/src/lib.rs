use std::{
    ffi::OsStr,
    io,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use cap_std::{
    ambient_authority,
    fs::{Dir, Metadata, MetadataExt},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod acl;
pub mod archive;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VfsEntry {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified_at: Option<i64>,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFeature {
    Read,
    Write,
    Trash,
    Ownership,
    Permissions,
    Acl,
    ResumableUpload,
}

pub trait VfsProvider {
    fn capabilities(&self) -> Vec<ProviderFeature>;
    fn list(&self, path: &str) -> Result<Vec<VfsEntry>, VfsError>;
    fn stat(&self, path: &str) -> Result<VfsEntry, VfsError>;
    fn create_directory(&self, path: &str) -> Result<(), VfsError>;
    fn rename(&self, from: &str, to: &str) -> Result<(), VfsError>;
    fn copy_file(&self, from: &str, to: &str) -> Result<u64, VfsError>;
    fn trash(&self, path: &str) -> Result<String, VfsError>;
    fn read_limited(&self, path: &str, maximum_bytes: usize) -> Result<Vec<u8>, VfsError>;
    fn write_file(&self, path: &str, content: &[u8]) -> Result<u64, VfsError>;
    fn chmod(&self, path: &str, mode: u32) -> Result<(), VfsError>;
    fn search(
        &self,
        path: &str,
        query: &str,
        maximum_results: usize,
    ) -> Result<Vec<VfsEntry>, VfsError>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum LocalFileOperation {
    List {
        path: String,
    },
    Stat {
        path: String,
    },
    CreateDirectory {
        path: String,
    },
    Rename {
        from: String,
        to: String,
    },
    CopyFile {
        from: String,
        to: String,
    },
    Trash {
        path: String,
    },
    ReadPreview {
        path: String,
        maximum_bytes: usize,
    },
    WriteFile {
        path: String,
        content: Vec<u8>,
    },
    Chmod {
        path: String,
        mode: u32,
    },
    Search {
        path: String,
        query: String,
        maximum_results: usize,
    },
    CreateArchive {
        /// Virtual paths of the files/directories to include. Preserved
        /// relative to the VFS root inside the archive (e.g. selecting
        /// `/docs` produces entries under `docs/...`).
        sources: Vec<String>,
        destination: String,
        format: archive::ArchiveFormat,
    },
    ExtractArchive {
        archive: String,
        destination: String,
        format: archive::ArchiveFormat,
    },
    ReadAcl {
        path: String,
    },
    SetAcl {
        path: String,
        entries: Vec<acl::AclEntry>,
    },
}

impl LocalFileOperation {
    #[must_use]
    pub const fn requires_write(&self) -> bool {
        matches!(
            self,
            Self::CreateDirectory { .. }
                | Self::Rename { .. }
                | Self::CopyFile { .. }
                | Self::Trash { .. }
                | Self::WriteFile { .. }
                | Self::Chmod { .. }
                | Self::CreateArchive { .. }
                | Self::ExtractArchive { .. }
                | Self::SetAcl { .. }
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum LocalFileResult {
    Entries {
        entries: Vec<VfsEntry>,
        capabilities: Vec<ProviderFeature>,
    },
    Entry {
        entry: VfsEntry,
    },
    Complete,
    Copied {
        bytes: u64,
    },
    Trashed {
        path: String,
    },
    Preview {
        bytes: Vec<u8>,
    },
    Written {
        bytes: u64,
    },
    SearchResults {
        entries: Vec<VfsEntry>,
    },
    ArchiveCreated {
        path: String,
        entries: u64,
        bytes: u64,
    },
    ArchiveExtracted {
        entries: u64,
        bytes: u64,
    },
    Acl {
        entries: Vec<acl::AclEntry>,
        supported: bool,
    },
}

pub fn execute_local(
    root: impl AsRef<Path>,
    writable: bool,
    operation: &LocalFileOperation,
) -> Result<LocalFileResult, VfsError> {
    let provider = LocalProvider::open(root, writable)?;
    match operation {
        LocalFileOperation::List { path } => Ok(LocalFileResult::Entries {
            entries: provider.list(path)?,
            capabilities: provider.capabilities(),
        }),
        LocalFileOperation::Stat { path } => Ok(LocalFileResult::Entry {
            entry: provider.stat(path)?,
        }),
        LocalFileOperation::CreateDirectory { path } => {
            provider.create_directory(path)?;
            Ok(LocalFileResult::Complete)
        }
        LocalFileOperation::Rename { from, to } => {
            provider.rename(from, to)?;
            Ok(LocalFileResult::Complete)
        }
        LocalFileOperation::CopyFile { from, to } => Ok(LocalFileResult::Copied {
            bytes: provider.copy_file(from, to)?,
        }),
        LocalFileOperation::Trash { path } => Ok(LocalFileResult::Trashed {
            path: provider.trash(path)?,
        }),
        LocalFileOperation::ReadPreview {
            path,
            maximum_bytes,
        } => Ok(LocalFileResult::Preview {
            // The typed helper protocol is deliberately capped at 64 KiB. JSON
            // encodes byte arrays verbosely, so previews stay below that frame.
            bytes: provider.read_limited(path, (*maximum_bytes).min(12 * 1024))?,
        }),
        LocalFileOperation::WriteFile { path, content } => Ok(LocalFileResult::Written {
            bytes: provider.write_file(path, content)?,
        }),
        LocalFileOperation::Chmod { path, mode } => {
            provider.chmod(path, *mode)?;
            Ok(LocalFileResult::Complete)
        }
        LocalFileOperation::Search {
            path,
            query,
            maximum_results,
        } => Ok(LocalFileResult::SearchResults {
            entries: provider.search(path, query, *maximum_results)?,
        }),
        LocalFileOperation::CreateArchive {
            sources,
            destination,
            format,
        } => {
            let outcome = archive::create_archive(&provider, sources, destination, *format)?;
            Ok(LocalFileResult::ArchiveCreated {
                path: canonical_virtual(destination),
                entries: outcome.entries,
                bytes: outcome.bytes,
            })
        }
        LocalFileOperation::ExtractArchive {
            archive,
            destination,
            format,
        } => {
            let outcome = archive::extract_archive(&provider, archive, destination, *format)?;
            Ok(LocalFileResult::ArchiveExtracted {
                entries: outcome.entries,
                bytes: outcome.bytes,
            })
        }
        LocalFileOperation::ReadAcl { path } => {
            let (entries, supported) = acl::read_acl(&provider, path)?;
            Ok(LocalFileResult::Acl { entries, supported })
        }
        LocalFileOperation::SetAcl { path, entries } => {
            acl::set_acl(&provider, path, entries)?;
            Ok(LocalFileResult::Complete)
        }
    }
}

pub struct LocalProvider {
    root_path: PathBuf,
    root: Dir,
    writable: bool,
}

impl LocalProvider {
    pub fn open(root: impl AsRef<Path>, writable: bool) -> Result<Self, VfsError> {
        let root_path = std::fs::canonicalize(root).map_err(VfsError::Io)?;
        let root = Dir::open_ambient_dir(&root_path, ambient_authority()).map_err(VfsError::Io)?;
        Ok(Self {
            root_path,
            root,
            writable,
        })
    }

    #[must_use]
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// The capability-sandboxed directory handle backing this provider.
    /// Crate-internal: archive create/extract need direct `Dir` access to
    /// stream file contents without buffering whole files in memory, but
    /// every path they touch still goes through
    /// [`normalize_virtual_path`] first, same as every other operation
    /// here — this accessor does not weaken the sandbox, `cap_std`'s `Dir`
    /// itself refuses to resolve outside `root_path` regardless of what
    /// path string it's given.
    pub(crate) fn dir(&self) -> &Dir {
        &self.root
    }

    pub(crate) fn is_writable(&self) -> bool {
        self.writable
    }

    fn require_write(&self) -> Result<(), VfsError> {
        if self.writable {
            Ok(())
        } else {
            Err(VfsError::ReadOnly)
        }
    }

    fn entry(&self, relative: &Path, display_path: &str) -> Result<VfsEntry, VfsError> {
        let metadata = self.root.symlink_metadata(relative).map_err(VfsError::Io)?;
        let name = relative
            .file_name()
            .unwrap_or_else(|| OsStr::new("/"))
            .to_str()
            .ok_or(VfsError::NonUtf8Name)?
            .to_owned();
        Ok(entry_from_metadata(
            name,
            display_path.to_owned(),
            &metadata,
        ))
    }
}

impl VfsProvider for LocalProvider {
    fn capabilities(&self) -> Vec<ProviderFeature> {
        let mut features = vec![ProviderFeature::Read];
        if self.writable {
            features.extend([
                ProviderFeature::Write,
                ProviderFeature::Trash,
                ProviderFeature::Ownership,
                ProviderFeature::Permissions,
                ProviderFeature::Acl,
                ProviderFeature::ResumableUpload,
            ]);
        }
        features
    }

    fn list(&self, path: &str) -> Result<Vec<VfsEntry>, VfsError> {
        let relative = normalize_virtual_path(path, true)?;
        let directory = self.root.open_dir(&relative).map_err(VfsError::Io)?;
        let mut entries = Vec::new();
        for item in directory.entries().map_err(VfsError::Io)? {
            let item = item.map_err(VfsError::Io)?;
            let name = item
                .file_name()
                .to_str()
                .ok_or(VfsError::NonUtf8Name)?
                .to_owned();
            let metadata = directory
                .symlink_metadata(Path::new(&name))
                .map_err(VfsError::Io)?;
            entries.push(entry_from_metadata(
                name.clone(),
                join_virtual(path, &name),
                &metadata,
            ));
        }
        entries.sort_by(|left, right| {
            let left_directory = left.kind == EntryKind::Directory;
            let right_directory = right.kind == EntryKind::Directory;
            right_directory
                .cmp(&left_directory)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(entries)
    }

    fn stat(&self, path: &str) -> Result<VfsEntry, VfsError> {
        let relative = normalize_virtual_path(path, false)?;
        self.entry(&relative, &canonical_virtual(path))
    }

    fn create_directory(&self, path: &str) -> Result<(), VfsError> {
        self.require_write()?;
        let relative = normalize_virtual_path(path, false)?;
        self.root.create_dir(relative).map_err(VfsError::Io)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), VfsError> {
        self.require_write()?;
        let from = normalize_virtual_path(from, false)?;
        let to = normalize_virtual_path(to, false)?;
        self.root.rename(from, &self.root, to).map_err(VfsError::Io)
    }

    fn copy_file(&self, from: &str, to: &str) -> Result<u64, VfsError> {
        self.require_write()?;
        let from = normalize_virtual_path(from, false)?;
        let to = normalize_virtual_path(to, false)?;
        self.root.copy(from, &self.root, to).map_err(VfsError::Io)
    }

    fn trash(&self, path: &str) -> Result<String, VfsError> {
        self.require_write()?;
        let relative = normalize_virtual_path(path, false)?;
        let name = relative
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or(VfsError::InvalidPath)?;
        let trash_directory = Path::new(".Trash").join("clouddesk");
        self.root
            .create_dir_all(&trash_directory)
            .map_err(VfsError::Io)?;
        let destination = trash_directory.join(format!("{}-{name}", unix_time()));
        self.root
            .rename(&relative, &self.root, &destination)
            .map_err(VfsError::Io)?;
        Ok(format!("/{}", destination.to_string_lossy()))
    }

    fn read_limited(&self, path: &str, maximum_bytes: usize) -> Result<Vec<u8>, VfsError> {
        let relative = normalize_virtual_path(path, false)?;
        let metadata = self
            .root
            .symlink_metadata(&relative)
            .map_err(VfsError::Io)?;
        if metadata.file_type().is_symlink() {
            return Err(VfsError::SymlinkReadDenied);
        }
        if metadata.len() > u64::try_from(maximum_bytes).unwrap_or(u64::MAX) {
            return Err(VfsError::TooLarge);
        }
        self.root.read(relative).map_err(VfsError::Io)
    }

    fn write_file(&self, path: &str, content: &[u8]) -> Result<u64, VfsError> {
        self.require_write()?;
        let relative = normalize_virtual_path(path, false)?;
        if let Some(parent) = relative.parent() {
            if !parent.as_os_str().is_empty() && parent != Path::new(".") {
                self.root.create_dir_all(parent).map_err(VfsError::Io)?;
            }
        }
        self.root.write(&relative, content).map_err(VfsError::Io)?;
        Ok(u64::try_from(content.len()).unwrap_or(0))
    }

    fn chmod(&self, path: &str, mode: u32) -> Result<(), VfsError> {
        use cap_std::fs::PermissionsExt;
        self.require_write()?;
        let relative = normalize_virtual_path(path, false)?;
        let permissions = cap_std::fs::Permissions::from_mode(mode & 0o7777);
        self.root
            .set_permissions(&relative, permissions)
            .map_err(VfsError::Io)?;
        Ok(())
    }

    fn search(
        &self,
        path: &str,
        query: &str,
        maximum_results: usize,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        let root_rel = normalize_virtual_path(path, true)?;
        let query_lower = query.to_lowercase();
        let max_results = maximum_results.clamp(1, 200);
        let mut results = Vec::new();
        let mut stack = vec![(root_rel, canonical_virtual(path))];

        while let Some((dir_rel, dir_virt)) = stack.pop() {
            let Ok(directory) = self.root.open_dir(&dir_rel) else {
                continue;
            };
            let Ok(entries) = directory.entries() else {
                continue;
            };
            for item in entries.flatten() {
                let Some(name) = item.file_name().to_str().map(ToOwned::to_owned) else {
                    continue;
                };
                let Ok(meta) = directory.symlink_metadata(Path::new(&name)) else {
                    continue;
                };
                let child_virt = join_virtual(&dir_virt, &name);
                let child_rel = if dir_rel == Path::new(".") {
                    PathBuf::from(&name)
                } else {
                    dir_rel.join(&name)
                };

                if name.to_lowercase().contains(&query_lower) {
                    results.push(entry_from_metadata(name, child_virt.clone(), &meta));
                    if results.len() >= max_results {
                        return Ok(results);
                    }
                }
                if meta.is_dir() && !meta.file_type().is_symlink() && stack.len() < 100 {
                    stack.push((child_rel, child_virt));
                }
            }
        }
        Ok(results)
    }
}

pub(crate) fn normalize_virtual_path(path: &str, allow_root: bool) -> Result<PathBuf, VfsError> {
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
        if allow_root {
            normalized.push(".");
        } else {
            return Err(VfsError::InvalidPath);
        }
    }
    Ok(normalized)
}

fn canonical_virtual(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        "/".to_owned()
    } else {
        format!("/{trimmed}")
    }
}

fn join_virtual(parent: &str, name: &str) -> String {
    let parent = canonical_virtual(parent);
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn entry_from_metadata(name: String, path: String, metadata: &Metadata) -> VfsEntry {
    let kind = if metadata.file_type().is_symlink() {
        EntryKind::Symlink
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else if metadata.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    VfsEntry {
        name,
        path,
        kind,
        size: metadata.len(),
        modified_at: metadata
            .modified()
            .ok()
            .and_then(|value| system_time(value.into_std())),
        mode: metadata.mode(),
        uid: metadata.uid(),
        gid: metadata.gid(),
    }
}

fn system_time(value: SystemTime) -> Option<i64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
}

fn unix_time() -> i64 {
    system_time(SystemTime::now()).unwrap_or(0)
}

#[derive(Debug, Error)]
pub enum VfsError {
    #[error("path traversal is not allowed")]
    Traversal,
    #[error("path is invalid")]
    InvalidPath,
    #[error("non-UTF-8 names are not supported by the web API")]
    NonUtf8Name,
    #[error("provider is read-only")]
    ReadOnly,
    #[error("direct reads through symbolic links are denied")]
    SymlinkReadDenied,
    #[error("file exceeds the requested size limit")]
    TooLarge,
    #[error("filesystem operation failed: {0}")]
    Io(#[source] io::Error),
    #[error("archive entry uses an unsafe path: {0}")]
    UnsafeArchiveEntry(String),
    #[error("archive entry is a symlink or hard link, which is not permitted")]
    ArchiveEntryLinkDenied,
    #[error("archive exceeds the configured extraction quota")]
    ArchiveQuotaExceeded,
    #[error("archive is malformed or unsupported: {0}")]
    InvalidArchive(String),
    #[error("no sources were selected for the archive")]
    EmptyArchiveSelection,
    #[error("operation is not supported on this filesystem")]
    Unsupported,
    #[error("invalid ACL entry: {0}")]
    InvalidAclEntry(String),
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use super::*;

    #[test]
    fn provider_lists_and_mutates_only_inside_its_capability_root() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("alpha.txt"), "alpha").unwrap();
        fs::create_dir(directory.path().join("documents")).unwrap();
        let provider = LocalProvider::open(directory.path(), true).unwrap();

        let entries = provider.list("/").unwrap();
        assert_eq!(entries[0].kind, EntryKind::Directory);
        provider.create_directory("/new").unwrap();
        provider.rename("/alpha.txt", "/renamed.txt").unwrap();
        provider.copy_file("/renamed.txt", "/copy.txt").unwrap();
        assert_eq!(provider.read_limited("/copy.txt", 32).unwrap(), b"alpha");
        assert!(directory.path().join("new").is_dir());
    }

    #[test]
    fn traversal_and_symlink_escape_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), "outside").unwrap();
        symlink(outside.path(), root.path().join("escape")).unwrap();
        let provider = LocalProvider::open(root.path(), true).unwrap();

        assert!(matches!(provider.list("/../"), Err(VfsError::Traversal)));
        assert!(provider.list("/escape").is_err());
        assert!(matches!(
            provider.read_limited("/escape/secret", 64),
            Err(VfsError::Io(_) | VfsError::SymlinkReadDenied)
        ));
    }

    #[test]
    fn read_only_roots_reject_all_mutations() {
        let root = tempfile::tempdir().unwrap();
        let provider = LocalProvider::open(root.path(), false).unwrap();
        assert!(matches!(
            provider.create_directory("/blocked"),
            Err(VfsError::ReadOnly)
        ));
        assert!(matches!(
            provider.write_file("/blocked.txt", b"data"),
            Err(VfsError::ReadOnly)
        ));
        assert!(matches!(
            provider.chmod("/blocked.txt", 0o644),
            Err(VfsError::ReadOnly)
        ));
    }

    #[test]
    fn write_file_chmod_and_search_operate_within_root() {
        let root = tempfile::tempdir().unwrap();
        let provider = LocalProvider::open(root.path(), true).unwrap();

        // Write file
        let written = provider
            .write_file("/sub/nested/file.txt", b"hello world")
            .unwrap();
        assert_eq!(written, 11);
        assert_eq!(
            provider.read_limited("/sub/nested/file.txt", 32).unwrap(),
            b"hello world"
        );

        // Chmod
        provider.chmod("/sub/nested/file.txt", 0o600).unwrap();
        let stat = provider.stat("/sub/nested/file.txt").unwrap();
        assert_eq!(stat.mode & 0o777, 0o600);

        // Search
        let results = provider.search("/", "file", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "file.txt");
        assert_eq!(results[0].path, "/sub/nested/file.txt");

        // Search for non-existent
        let empty = provider.search("/", "nonexistent", 10).unwrap();
        assert!(empty.is_empty());
    }
}
