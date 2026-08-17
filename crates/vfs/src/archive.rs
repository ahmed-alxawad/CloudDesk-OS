//! Archive create/extract for [`crate::LocalProvider`].
//!
//! `GOAL.md` G3: "archive creation/extraction." Everything here goes
//! through the same `cap_std`-sandboxed [`Dir`] every other local
//! operation uses (via [`crate::LocalProvider::dir`]), so a bug in the
//! entry-name validation below is defense-in-depth backed by the sandbox
//! itself refusing to resolve outside the provider's root regardless of
//! what relative path string it's handed.
//!
//! Security posture (Zip Slip / Tar Slip and friends):
//! - every extracted entry name is validated by [`safe_entry_path`] before
//!   any filesystem call is made for it: no `..`, no absolute path, no
//!   backslash (rules out the classic Windows-drive-letter trick on a
//!   platform where `\` isn't a path separator and so wouldn't otherwise
//!   be caught by component parsing), no embedded NUL;
//! - symlink and hard-link entries are rejected outright, both on create
//!   (never followed/dereferenced into the archive) and on extract (never
//!   materialized) — the archive contents traverse only regular files and
//!   directories;
//! - extraction is bounded by an entry-count and a *decompressed-bytes*
//!   quota, the latter enforced by counting actual bytes copied out of
//!   the decompressor rather than trusting any size field the archive
//!   format declares, which is what actually stops a zip/tar bomb;
//! - anything written by a failed extraction attempt is cleaned up before
//!   the error is returned.

use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};

use crate::{normalize_virtual_path, LocalProvider, VfsError};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArchiveOutcome {
    pub entries: u64,
    pub bytes: u64,
}

/// Entries beyond this count abort the extraction.
const MAX_EXTRACT_ENTRIES: u64 = 100_000;
/// Total *decompressed* bytes beyond this abort the extraction — this is
/// the actual zip/tar-bomb defense, checked incrementally against bytes
/// really read out of the decompressor, not a declared header field.
const MAX_EXTRACT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

const COPY_BUFFER_SIZE: usize = 256 * 1024;

pub fn create_archive(
    provider: &LocalProvider,
    sources: &[String],
    destination: &str,
    format: ArchiveFormat,
) -> Result<ArchiveOutcome, VfsError> {
    if !provider.is_writable() {
        return Err(VfsError::ReadOnly);
    }
    if sources.is_empty() {
        return Err(VfsError::EmptyArchiveSelection);
    }
    let destination_relative = normalize_virtual_path(destination, false)?;
    if let Some(parent) = destination_relative.parent() {
        if !parent.as_os_str().is_empty() && parent != Path::new(".") {
            provider
                .dir()
                .create_dir_all(parent)
                .map_err(VfsError::Io)?;
        }
    }

    // Every authorized source is validated through the same
    // `normalize_virtual_path` every other operation uses before we ever
    // touch it, so a source path can't itself be a traversal/escape
    // attempt.
    let mut resolved_sources = Vec::with_capacity(sources.len());
    for source in sources {
        resolved_sources.push(normalize_virtual_path(source, false)?);
    }

    let outcome = match format {
        ArchiveFormat::Zip => create_zip(provider, &resolved_sources, &destination_relative)?,
        ArchiveFormat::TarGz => create_tar_gz(provider, &resolved_sources, &destination_relative)?,
    };
    Ok(outcome)
}

fn create_zip(
    provider: &LocalProvider,
    sources: &[PathBuf],
    destination: &Path,
) -> Result<ArchiveOutcome, VfsError> {
    let file = provider.dir().create(destination).map_err(VfsError::Io)?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut outcome = ArchiveOutcome::default();

    for source in sources {
        walk_source(provider.dir(), source, &mut |entry_name, kind, dir, rel| {
            match kind {
                EntryKind::Directory => {
                    writer
                        .add_directory(format!("{entry_name}/"), options)
                        .map_err(|error| zip_write_error(&error))?;
                }
                EntryKind::File => {
                    writer
                        .start_file(entry_name.clone(), options)
                        .map_err(|error| zip_write_error(&error))?;
                    let mut source_file = dir.open(rel).map_err(VfsError::Io)?;
                    outcome.bytes +=
                        io::copy(&mut source_file, &mut writer).map_err(VfsError::Io)?;
                }
                EntryKind::SkippedSymlink => return Ok(()),
            }
            outcome.entries += 1;
            Ok(())
        })?;
    }

    writer.finish().map_err(|error| zip_write_error(&error))?;
    Ok(outcome)
}

fn create_tar_gz(
    provider: &LocalProvider,
    sources: &[PathBuf],
    destination: &Path,
) -> Result<ArchiveOutcome, VfsError> {
    let file = provider.dir().create(destination).map_err(VfsError::Io)?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let mut outcome = ArchiveOutcome::default();

    for source in sources {
        walk_source(provider.dir(), source, &mut |entry_name, kind, dir, rel| {
            match kind {
                EntryKind::Directory => {
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_size(0);
                    header.set_mode(0o755);
                    header.set_cksum();
                    builder
                        .append_data(&mut header, format!("{entry_name}/"), io::empty())
                        .map_err(VfsError::Io)?;
                }
                EntryKind::File => {
                    let mut source_file = dir.open(rel).map_err(VfsError::Io)?;
                    let metadata = dir.metadata(rel).map_err(VfsError::Io)?;
                    let mut header = tar::Header::new_gnu();
                    header.set_size(metadata.len());
                    header.set_mode(0o644);
                    header.set_cksum();
                    outcome.bytes += metadata.len();
                    builder
                        .append_data(&mut header, entry_name.clone(), &mut source_file)
                        .map_err(VfsError::Io)?;
                }
                EntryKind::SkippedSymlink => return Ok(()),
            }
            outcome.entries += 1;
            Ok(())
        })?;
    }

    builder
        .into_inner()
        .map_err(VfsError::Io)?
        .finish()
        .map_err(VfsError::Io)?;
    Ok(outcome)
}

enum EntryKind {
    File,
    Directory,
    /// A symlink was encountered and intentionally not followed/added —
    /// "do not follow unauthorized symlink targets."
    SkippedSymlink,
}

/// Callback invoked by [`walk_source`] for each file/directory/skipped
/// symlink found: entry name, kind, the sandboxed `Dir` to open it
/// through, and its path relative to that `Dir`.
type WalkVisitor<'a> = dyn FnMut(&String, EntryKind, &Dir, &Path) -> Result<(), VfsError> + 'a;

/// Walks `source` (a file or a directory tree) inside `dir`, calling
/// `visit` for each file/directory found, with an entry name preserving
/// the path relative to the VFS root (matching `source`'s own already-
/// normalized relative path). Symlinks are reported as
/// [`EntryKind::SkippedSymlink`] and never opened/followed.
fn walk_source(dir: &Dir, source: &Path, visit: &mut WalkVisitor<'_>) -> Result<(), VfsError> {
    let metadata = dir.symlink_metadata(source).map_err(VfsError::Io)?;
    let entry_name = source.to_string_lossy().replace('\\', "/");

    if metadata.file_type().is_symlink() {
        visit(&entry_name, EntryKind::SkippedSymlink, dir, source)?;
        return Ok(());
    }
    if metadata.is_dir() {
        visit(&entry_name, EntryKind::Directory, dir, source)?;
        let subdirectory = dir.open_dir(source).map_err(VfsError::Io)?;
        let mut children: Vec<_> = subdirectory
            .entries()
            .map_err(VfsError::Io)?
            .filter_map(Result::ok)
            .collect();
        children.sort_by_key(cap_std::fs::DirEntry::file_name);
        for child in children {
            let Some(name) = child.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            walk_source(dir, &source.join(&name), visit)?;
        }
        return Ok(());
    }
    visit(&entry_name, EntryKind::File, dir, source)
}

fn zip_write_error(error: &zip::result::ZipError) -> VfsError {
    VfsError::Io(io::Error::other(error.to_string()))
}

pub fn extract_archive(
    provider: &LocalProvider,
    archive_path: &str,
    destination: &str,
    format: ArchiveFormat,
) -> Result<ArchiveOutcome, VfsError> {
    if !provider.is_writable() {
        return Err(VfsError::ReadOnly);
    }
    let archive_relative = normalize_virtual_path(archive_path, false)?;
    let destination_relative = normalize_virtual_path(destination, true)?;
    provider
        .dir()
        .create_dir_all(&destination_relative)
        .map_err(VfsError::Io)?;

    let mut created = Vec::new();
    let result = match format {
        ArchiveFormat::Zip => extract_zip(
            provider.dir(),
            &archive_relative,
            &destination_relative,
            &mut created,
        ),
        ArchiveFormat::TarGz => extract_tar_gz(
            provider.dir(),
            &archive_relative,
            &destination_relative,
            &mut created,
        ),
    };

    if result.is_err() {
        cleanup_partial_extraction(provider.dir(), &created);
    }
    result
}

fn cleanup_partial_extraction(dir: &Dir, created: &[PathBuf]) {
    for path in created.iter().rev() {
        let _ = dir.remove_file(path);
    }
    for path in created.iter().rev() {
        let _ = dir.remove_dir(path);
    }
}

fn extract_zip(
    dir: &Dir,
    archive_relative: &Path,
    destination: &Path,
    created: &mut Vec<PathBuf>,
) -> Result<ArchiveOutcome, VfsError> {
    let file = dir.open(archive_relative).map_err(VfsError::Io)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| VfsError::InvalidArchive(error.to_string()))?;

    if u64::try_from(archive.len()).unwrap_or(u64::MAX) > MAX_EXTRACT_ENTRIES {
        return Err(VfsError::ArchiveQuotaExceeded);
    }

    let mut outcome = ArchiveOutcome::default();
    for index in 0..archive.len() {
        let mut zip_entry = archive
            .by_index(index)
            .map_err(|error| VfsError::InvalidArchive(error.to_string()))?;
        if zip_entry.is_symlink() {
            return Err(VfsError::ArchiveEntryLinkDenied);
        }
        let target = destination.join(safe_entry_path(zip_entry.name())?);

        if zip_entry.is_dir() {
            dir.create_dir_all(&target).map_err(VfsError::Io)?;
            created.push(target);
            outcome.entries += 1;
            continue;
        }

        if let Some(parent) = target.parent() {
            dir.create_dir_all(parent).map_err(VfsError::Io)?;
        }
        let mut out_file = dir.create(&target).map_err(VfsError::Io)?;
        created.push(target);
        let written = copy_with_quota(&mut zip_entry, &mut out_file, &mut outcome.bytes)?;
        let _ = written;
        outcome.entries += 1;
    }
    Ok(outcome)
}

fn extract_tar_gz(
    dir: &Dir,
    archive_relative: &Path,
    destination: &Path,
    created: &mut Vec<PathBuf>,
) -> Result<ArchiveOutcome, VfsError> {
    let file = dir.open(archive_relative).map_err(VfsError::Io)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut outcome = ArchiveOutcome::default();

    let entries = archive
        .entries()
        .map_err(|error| VfsError::InvalidArchive(error.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| VfsError::InvalidArchive(error.to_string()))?;
        outcome.entries += 1;
        if outcome.entries > MAX_EXTRACT_ENTRIES {
            return Err(VfsError::ArchiveQuotaExceeded);
        }

        let header_type = entry.header().entry_type();
        if header_type.is_symlink() || header_type.is_hard_link() {
            return Err(VfsError::ArchiveEntryLinkDenied);
        }

        let raw_path = entry
            .path()
            .map_err(|error| VfsError::InvalidArchive(error.to_string()))?;
        let raw_path_string = raw_path.to_string_lossy().into_owned();
        let target = destination.join(safe_entry_path(&raw_path_string)?);

        if header_type.is_dir() {
            dir.create_dir_all(&target).map_err(VfsError::Io)?;
            created.push(target);
            continue;
        }
        if !header_type.is_file() {
            // Device nodes, FIFOs, etc. — never materialize these.
            return Err(VfsError::ArchiveEntryLinkDenied);
        }

        if let Some(parent) = target.parent() {
            dir.create_dir_all(parent).map_err(VfsError::Io)?;
        }
        let mut out_file = dir.create(&target).map_err(VfsError::Io)?;
        created.push(target);
        copy_with_quota(&mut entry, &mut out_file, &mut outcome.bytes)?;
    }
    Ok(outcome)
}

/// Copies from `reader` to `writer`, tracking the running total against
/// [`MAX_EXTRACT_BYTES`] — this is checked against bytes actually read out
/// of the (possibly decompressing) reader, which is what makes it a real
/// zip/tar-bomb defense rather than trusting a declared size field.
fn copy_with_quota(
    reader: &mut impl Read,
    writer: &mut impl Write,
    running_total: &mut u64,
) -> Result<u64, VfsError> {
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    let mut copied = 0_u64;
    loop {
        let read = reader.read(&mut buffer).map_err(VfsError::Io)?;
        if read == 0 {
            break;
        }
        *running_total += read as u64;
        if *running_total > MAX_EXTRACT_BYTES {
            return Err(VfsError::ArchiveQuotaExceeded);
        }
        writer.write_all(&buffer[..read]).map_err(VfsError::Io)?;
        copied += read as u64;
    }
    Ok(copied)
}

/// Validates an archive entry name and returns it as a safe path relative
/// to the extraction destination. Rejects everything Zip Slip / Tar Slip
/// and the Windows-drive-letter trick rely on:
/// - absolute paths (leading `/`)
/// - `..` anywhere in the path
/// - backslashes (not a path separator on this platform, so a name like
///   `C:\Windows\System32\evil.dll` would otherwise parse as one opaque
///   "normal" component and slip past component-based checks entirely)
/// - a drive-letter prefix (`C:...`)
/// - embedded NUL bytes
/// - an empty result
fn safe_entry_path(raw: &str) -> Result<PathBuf, VfsError> {
    if raw.is_empty() || raw.as_bytes().contains(&0) {
        return Err(VfsError::UnsafeArchiveEntry(raw.to_owned()));
    }
    if raw.contains('\\') {
        return Err(VfsError::UnsafeArchiveEntry(raw.to_owned()));
    }
    let mut chars = raw.chars();
    if let (Some(first), Some(':')) = (chars.next(), chars.next()) {
        if first.is_ascii_alphabetic() {
            return Err(VfsError::UnsafeArchiveEntry(raw.to_owned()));
        }
    }

    let mut safe = PathBuf::new();
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(VfsError::UnsafeArchiveEntry(raw.to_owned()));
            }
        }
    }
    if safe.as_os_str().is_empty() {
        return Err(VfsError::UnsafeArchiveEntry(raw.to_owned()));
    }
    Ok(safe)
}
