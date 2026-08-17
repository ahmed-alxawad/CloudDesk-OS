//! Tests for POSIX ACL read/edit (`crates/vfs/src/acl.rs`), run against a
//! real filesystem's real `getfacl`/`setfacl`. Uses only temporary
//! fixtures.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use clouddesk_vfs::acl::{read_acl, set_acl, AclEntry, AclQualifierKind};
use clouddesk_vfs::{LocalProvider, VfsError, VfsProvider};

fn make_provider(dir: &std::path::Path, writable: bool) -> LocalProvider {
    LocalProvider::open(dir, writable).unwrap()
}

fn has_acl_tools() -> bool {
    std::process::Command::new("getfacl")
        .arg("--version")
        .output()
        .is_ok()
}

#[test]
fn read_acl_reports_base_entries_for_a_plain_file() {
    if !has_acl_tools() {
        eprintln!("skipping: getfacl/setfacl not installed in this environment");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("plain.txt"), b"hello").unwrap();
    let provider = make_provider(dir.path(), true);

    let (entries, supported) = read_acl(&provider, "plain.txt").unwrap();
    assert!(supported);
    assert!(entries
        .iter()
        .any(|e| e.kind == AclQualifierKind::OwningUser));
    assert!(entries
        .iter()
        .any(|e| e.kind == AclQualifierKind::OwningGroup));
    assert!(entries.iter().any(|e| e.kind == AclQualifierKind::Other));
}

#[test]
fn set_acl_adds_a_named_user_entry_and_read_acl_reflects_it() {
    if !has_acl_tools() {
        eprintln!("skipping: getfacl/setfacl not installed in this environment");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("shared.txt"), b"data").unwrap();
    let provider = make_provider(dir.path(), true);

    // "nobody" exists on essentially every Linux system and is never the
    // acting test user, so this is a real cross-identity ACL grant.
    let new_entry = AclEntry {
        kind: AclQualifierKind::User,
        name: Some("nobody".to_owned()),
        read: true,
        write: false,
        execute: false,
    };
    set_acl(&provider, "shared.txt", std::slice::from_ref(&new_entry)).unwrap();

    let (entries, supported) = read_acl(&provider, "shared.txt").unwrap();
    assert!(supported);
    let found = entries
        .iter()
        .find(|e| e.kind == AclQualifierKind::User && e.name.as_deref() == Some("nobody"))
        .expect("newly added named-user entry must be visible on read-back");
    assert!(found.read);
    assert!(!found.write);
    assert!(!found.execute);

    // Modify: flip to read+write.
    let modified = AclEntry {
        read: true,
        write: true,
        execute: false,
        ..new_entry.clone()
    };
    set_acl(&provider, "shared.txt", &[modified]).unwrap();
    let (entries, _) = read_acl(&provider, "shared.txt").unwrap();
    let found = entries
        .iter()
        .find(|e| e.kind == AclQualifierKind::User && e.name.as_deref() == Some("nobody"))
        .unwrap();
    assert!(found.read && found.write);

    // Remove: setfacl -x via a direct removal call is a distinct code
    // path we don't expose yet; document that removal today means setting
    // the entry's permission bits to none, which is the safe subset this
    // implementation supports.
    let removed = AclEntry {
        read: false,
        write: false,
        execute: false,
        ..new_entry
    };
    set_acl(&provider, "shared.txt", &[removed]).unwrap();
    let (entries, _) = read_acl(&provider, "shared.txt").unwrap();
    let found = entries
        .iter()
        .find(|e| e.kind == AclQualifierKind::User && e.name.as_deref() == Some("nobody"))
        .unwrap();
    assert!(!found.read && !found.write && !found.execute);
}

#[test]
fn normal_chmod_still_works_alongside_acl_support() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("mode.txt"), b"data").unwrap();
    let provider = make_provider(dir.path(), true);
    provider.chmod("mode.txt", 0o640).unwrap();
    let metadata = fs::metadata(dir.path().join("mode.txt")).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0o640);
}

#[test]
fn read_acl_denied_outside_authorized_root() {
    let dir = tempfile::tempdir().unwrap();
    let provider = make_provider(dir.path(), true);
    let result = read_acl(&provider, "../../etc/passwd");
    assert!(matches!(result, Err(VfsError::Traversal)));
}

#[test]
fn set_acl_denied_without_write_capability() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("f.txt"), b"data").unwrap();
    let provider = make_provider(dir.path(), false);
    let entry = AclEntry {
        kind: AclQualifierKind::User,
        name: Some("nobody".to_owned()),
        read: true,
        write: false,
        execute: false,
    };
    let result = set_acl(&provider, "f.txt", &[entry]);
    assert!(matches!(result, Err(VfsError::ReadOnly)));
}

#[test]
fn set_acl_rejects_unsafe_qualifier_name() {
    if !has_acl_tools() {
        eprintln!("skipping: getfacl/setfacl not installed in this environment");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("f.txt"), b"data").unwrap();
    let provider = make_provider(dir.path(), true);
    let entry = AclEntry {
        kind: AclQualifierKind::User,
        name: Some("evil:x:0:0".to_owned()),
        read: true,
        write: false,
        execute: false,
    };
    let result = set_acl(&provider, "f.txt", &[entry]);
    assert!(
        matches!(result, Err(VfsError::InvalidAclEntry(_))),
        "a qualifier name containing setfacl spec delimiters must be rejected before it reaches setfacl"
    );
}
