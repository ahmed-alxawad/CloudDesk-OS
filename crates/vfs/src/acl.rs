//! POSIX ACL read/edit for [`crate::LocalProvider`].
//!
//! `GOAL.md` G3/G12: "ACL viewing/editing when authorized" /
//! "`CloudDesk` must respect... ACLs." Shells out to the standard
//! `getfacl`/`setfacl` tools (present on every mainstream Linux
//! distribution `CloudDesk` targets) with a fixed argv — never through a
//! shell, never with unvalidated data concatenated into a command string.
//!
//! Security posture:
//! - the target is opened through the same `cap_std`-sandboxed [`Dir`]
//!   every other local operation uses, then the *exact* filesystem path
//!   that fd refers to is recovered in-process via
//!   `readlink("/proc/self/fd/<n>")` (`/proc/self/fd` here means our own
//!   process — a child process spawned by `Command` does not inherit
//!   arbitrary open fds by default, so pointing `getfacl`/`setfacl`
//!   directly at `/proc/self/fd/<n>` and letting *them* resolve it does
//!   not work). That resolved path — not the caller-supplied virtual
//!   path string — is what's handed to the external tool, so a symlink
//!   anywhere in the caller's path can't cause the external tool to
//!   operate on something other than what the sandbox actually opened.
//!   There remains a conventional (small) TOCTOU window between this
//!   `readlink` and the external process's own `open`, same as any tool
//!   that takes a path argument;
//! - `cap_std::fs::Dir::open` itself refuses to resolve outside its own
//!   root even through a symlink, so a symlink-escape attempt never
//!   produces an fd (or therefore a resolved path) to hand to the ACL
//!   tools in the first place;
//! - named-user/named-group qualifiers are validated against a
//!   conservative Linux-identifier charset before being placed in the
//!   `setfacl` argument, so a crafted qualifier can't confuse `setfacl`'s
//!   own comma/colon-delimited spec parser into applying a different
//!   entry than intended (not a shell-injection risk — no shell is
//!   involved — but a parser-confusion risk worth closing regardless);
//! - a filesystem without ACL support (or a missing `getfacl`/`setfacl`
//!   binary) is reported as `supported: false` / [`VfsError::Unsupported`]
//!   rather than silently no-op'd or faked.

use std::os::unix::io::AsRawFd;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::{normalize_virtual_path, LocalProvider, VfsError};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AclQualifierKind {
    OwningUser,
    User,
    OwningGroup,
    Group,
    Mask,
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AclEntry {
    pub kind: AclQualifierKind,
    /// Present only for `User`/`Group` entries — the named user or group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

/// Opens `relative` through the sandboxed `Dir` and resolves the exact
/// real filesystem path that open refers to, via an in-process
/// `readlink("/proc/self/fd/<n>")` — see the module-level security-posture
/// note for why this is done here rather than handing `/proc/self/fd/<n>`
/// to the child process directly.
fn resolve_real_path(
    provider: &LocalProvider,
    relative: &std::path::Path,
) -> Result<std::path::PathBuf, VfsError> {
    let handle = provider.dir().open(relative).map_err(VfsError::Io)?;
    let fd_link = format!("/proc/self/fd/{}", handle.as_raw_fd());
    std::fs::read_link(&fd_link).map_err(VfsError::Io)
}

pub fn read_acl(provider: &LocalProvider, path: &str) -> Result<(Vec<AclEntry>, bool), VfsError> {
    let relative = normalize_virtual_path(path, false)?;
    let real_path = resolve_real_path(provider, &relative)?;

    let output = match Command::new("getfacl")
        .arg("--omit-header")
        .arg(&real_path)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), false));
        }
        Err(error) => return Err(VfsError::Io(error)),
    };
    if !output.status.success() {
        // A filesystem without ACL support is the expected reason
        // `getfacl` fails here (the target was already opened
        // successfully, so it exists and is reachable).
        return Ok((Vec::new(), false));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok((parse_getfacl_output(&stdout), true))
}

fn parse_getfacl_output(text: &str) -> Vec<AclEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.splitn(3, ':');
        let (Some(tag), Some(qualifier), Some(perms)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Some((read, write, execute)) = parse_permissions(perms) else {
            continue;
        };
        let entry = match tag {
            "user" if qualifier.is_empty() => AclEntry {
                kind: AclQualifierKind::OwningUser,
                name: None,
                read,
                write,
                execute,
            },
            "user" => AclEntry {
                kind: AclQualifierKind::User,
                name: Some(qualifier.to_owned()),
                read,
                write,
                execute,
            },
            "group" if qualifier.is_empty() => AclEntry {
                kind: AclQualifierKind::OwningGroup,
                name: None,
                read,
                write,
                execute,
            },
            "group" => AclEntry {
                kind: AclQualifierKind::Group,
                name: Some(qualifier.to_owned()),
                read,
                write,
                execute,
            },
            "mask" => AclEntry {
                kind: AclQualifierKind::Mask,
                name: None,
                read,
                write,
                execute,
            },
            "other" => AclEntry {
                kind: AclQualifierKind::Other,
                name: None,
                read,
                write,
                execute,
            },
            _ => continue,
        };
        entries.push(entry);
    }
    entries
}

fn parse_permissions(field: &str) -> Option<(bool, bool, bool)> {
    let bytes = field.as_bytes();
    if bytes.len() != 3 {
        return None;
    }
    Some((bytes[0] == b'r', bytes[1] == b'w', bytes[2] == b'x'))
}

/// Linux user/group names: letters, digits, underscore, hyphen, dot —
/// deliberately conservative, and specifically excludes `:` and `,`,
/// which are `setfacl` spec-string delimiters.
fn valid_qualifier_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn format_entry_spec(entry: &AclEntry) -> Result<String, VfsError> {
    let perms = format!(
        "{}{}{}",
        if entry.read { 'r' } else { '-' },
        if entry.write { 'w' } else { '-' },
        if entry.execute { 'x' } else { '-' }
    );
    let spec = match entry.kind {
        AclQualifierKind::OwningUser => format!("u::{perms}"),
        AclQualifierKind::OwningGroup => format!("g::{perms}"),
        AclQualifierKind::Mask => format!("m::{perms}"),
        AclQualifierKind::Other => format!("o::{perms}"),
        AclQualifierKind::User | AclQualifierKind::Group => {
            let name = entry.name.as_deref().ok_or_else(|| {
                VfsError::InvalidAclEntry("ACL entry missing qualifier name".into())
            })?;
            if !valid_qualifier_name(name) {
                return Err(VfsError::InvalidAclEntry(format!(
                    "unsafe ACL qualifier name: {name}"
                )));
            }
            let tag = if matches!(entry.kind, AclQualifierKind::User) {
                'u'
            } else {
                'g'
            };
            format!("{tag}:{name}:{perms}")
        }
    };
    Ok(spec)
}

pub fn set_acl(provider: &LocalProvider, path: &str, entries: &[AclEntry]) -> Result<(), VfsError> {
    if !provider.is_writable() {
        return Err(VfsError::ReadOnly);
    }
    if entries.is_empty() {
        return Err(VfsError::InvalidAclEntry("no ACL entries supplied".into()));
    }
    let relative = normalize_virtual_path(path, false)?;
    let real_path = resolve_real_path(provider, &relative)?;

    let mut specs = Vec::with_capacity(entries.len());
    for entry in entries {
        specs.push(format_entry_spec(entry)?);
    }
    let modify_arg = specs.join(",");

    let output = Command::new("setfacl")
        .arg("-m")
        .arg(&modify_arg)
        .arg(&real_path)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(VfsError::Unsupported);
        }
        Err(error) => return Err(VfsError::Io(error)),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.to_lowercase().contains("not supported")
            || stderr.to_lowercase().contains("operation not supported")
        {
            return Err(VfsError::Unsupported);
        }
        return Err(VfsError::Io(std::io::Error::other(stderr.into_owned())));
    }
    Ok(())
}
