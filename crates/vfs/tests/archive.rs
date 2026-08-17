//! Security-focused tests for archive create/extract
//! (`crates/vfs/src/archive.rs`). Uses only temporary fixtures.

use std::fs;
use std::io::Write;

use clouddesk_vfs::archive::{create_archive, extract_archive, ArchiveFormat};
use clouddesk_vfs::LocalProvider;

fn make_provider(dir: &std::path::Path, writable: bool) -> LocalProvider {
    LocalProvider::open(dir, writable).unwrap()
}

#[test]
fn zip_create_and_extract_round_trips_nested_directories() {
    let source_dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(source_dir.path().join("docs/sub")).unwrap();
    fs::write(source_dir.path().join("docs/report.txt"), b"report").unwrap();
    fs::write(source_dir.path().join("docs/sub/nested.txt"), b"nested").unwrap();
    fs::write(source_dir.path().join("top.txt"), b"top-level").unwrap();

    let provider = make_provider(source_dir.path(), true);
    let outcome = create_archive(
        &provider,
        &["docs".to_owned(), "top.txt".to_owned()],
        "archive.zip",
        ArchiveFormat::Zip,
    )
    .unwrap();
    assert!(
        outcome.entries >= 4,
        "expected at least 4 entries, got {}",
        outcome.entries
    );

    let dest_dir = tempfile::tempdir().unwrap();
    let dest_provider = make_provider(dest_dir.path(), true);
    fs::copy(
        source_dir.path().join("archive.zip"),
        dest_dir.path().join("archive.zip"),
    )
    .unwrap();
    let extracted =
        extract_archive(&dest_provider, "archive.zip", "out", ArchiveFormat::Zip).unwrap();
    assert!(extracted.entries >= 4);

    assert_eq!(
        fs::read(dest_dir.path().join("out/docs/report.txt")).unwrap(),
        b"report"
    );
    assert_eq!(
        fs::read(dest_dir.path().join("out/docs/sub/nested.txt")).unwrap(),
        b"nested"
    );
    assert_eq!(
        fs::read(dest_dir.path().join("out/top.txt")).unwrap(),
        b"top-level"
    );
}

#[test]
fn tar_gz_create_and_extract_round_trips_multiple_files() {
    let source_dir = tempfile::tempdir().unwrap();
    fs::write(source_dir.path().join("a.txt"), b"aaa").unwrap();
    fs::write(source_dir.path().join("b.txt"), b"bbb").unwrap();

    let provider = make_provider(source_dir.path(), true);
    create_archive(
        &provider,
        &["a.txt".to_owned(), "b.txt".to_owned()],
        "bundle.tar.gz",
        ArchiveFormat::TarGz,
    )
    .unwrap();

    let outcome = extract_archive(&provider, "bundle.tar.gz", "out", ArchiveFormat::TarGz).unwrap();
    assert_eq!(outcome.entries, 2);
    assert_eq!(
        fs::read(source_dir.path().join("out/a.txt")).unwrap(),
        b"aaa"
    );
    assert_eq!(
        fs::read(source_dir.path().join("out/b.txt")).unwrap(),
        b"bbb"
    );
}

#[test]
fn extract_rejects_zip_slip_traversal_entry() {
    let dest_dir = tempfile::tempdir().unwrap();
    let canary = dest_dir
        .path()
        .parent()
        .unwrap()
        .join("zip-slip-canary.txt");
    let _ = fs::remove_file(&canary);

    let archive_path = dest_dir.path().join("evil.zip");
    {
        let file = fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        // The zip *format* does not forbid this name — real-world Zip Slip
        // payloads rely on exactly this. Our extractor, not the writer, is
        // the thing that must refuse it.
        writer
            .start_file("../zip-slip-canary.txt", options)
            .unwrap();
        writer.write_all(b"escaped").unwrap();
        writer.finish().unwrap();
    }

    let provider = make_provider(dest_dir.path(), true);
    let result = extract_archive(&provider, "evil.zip", "out", ArchiveFormat::Zip);
    assert!(result.is_err(), "traversal entry must be rejected");
    assert!(
        !canary.exists(),
        "traversal entry must never be written outside the destination"
    );
}

#[test]
fn extract_rejects_absolute_path_entry() {
    let dest_dir = tempfile::tempdir().unwrap();
    let archive_path = dest_dir.path().join("evil.zip");
    {
        let file = fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("/etc/absolute-canary.txt", options)
            .unwrap();
        writer.write_all(b"escaped").unwrap();
        writer.finish().unwrap();
    }

    let provider = make_provider(dest_dir.path(), true);
    let result = extract_archive(&provider, "evil.zip", "out", ArchiveFormat::Zip);
    assert!(result.is_err(), "absolute-path entry must be rejected");
    assert!(!std::path::Path::new("/etc/absolute-canary.txt").exists());
}

#[test]
fn extract_rejects_windows_drive_letter_entry() {
    let dest_dir = tempfile::tempdir().unwrap();
    let archive_path = dest_dir.path().join("evil.zip");
    {
        let file = fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("C:\\Windows\\System32\\evil.dll", options)
            .unwrap();
        writer.write_all(b"escaped").unwrap();
        writer.finish().unwrap();
    }

    let provider = make_provider(dest_dir.path(), true);
    let result = extract_archive(&provider, "evil.zip", "out", ArchiveFormat::Zip);
    assert!(
        result.is_err(),
        "drive-letter/backslash entry must be rejected"
    );
}

#[test]
fn extract_rejects_symlink_entry() {
    let dest_dir = tempfile::tempdir().unwrap();
    let archive_path = dest_dir.path().join("evil.tar.gz");
    {
        let file = fs::File::create(&archive_path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_cksum();
        builder
            .append_link(&mut header, "evil-link", "/etc/passwd")
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
    }

    let provider = make_provider(dest_dir.path(), true);
    let result = extract_archive(&provider, "evil.tar.gz", "out", ArchiveFormat::TarGz);
    assert!(result.is_err(), "symlink entries must be rejected");
    assert!(!dest_dir.path().join("out/evil-link").exists());
}

#[test]
fn extract_partial_failure_cleans_up_already_written_entries() {
    let dest_dir = tempfile::tempdir().unwrap();
    let archive_path = dest_dir.path().join("mixed.zip");
    {
        let file = fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("first-ok.txt", options).unwrap();
        writer.write_all(b"ok").unwrap();
        // Second entry is malicious and must abort the whole extraction.
        writer.start_file("../escape.txt", options).unwrap();
        writer.write_all(b"nope").unwrap();
        writer.finish().unwrap();
    }

    let provider = make_provider(dest_dir.path(), true);
    let result = extract_archive(&provider, "mixed.zip", "out", ArchiveFormat::Zip);
    assert!(result.is_err());
    assert!(
        !dest_dir.path().join("out/first-ok.txt").exists(),
        "the successfully-written entry from a failed extraction must be cleaned up"
    );
}

#[test]
fn create_archive_denied_without_write_capability() {
    let source_dir = tempfile::tempdir().unwrap();
    fs::write(source_dir.path().join("a.txt"), b"a").unwrap();
    let provider = make_provider(source_dir.path(), false);
    let result = create_archive(
        &provider,
        &["a.txt".to_owned()],
        "out.zip",
        ArchiveFormat::Zip,
    );
    assert!(matches!(result, Err(clouddesk_vfs::VfsError::ReadOnly)));
}

#[test]
fn extract_archive_denied_without_write_capability() {
    let dir = tempfile::tempdir().unwrap();
    let archive_path = dir.path().join("a.zip");
    {
        let file = fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("a.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"a").unwrap();
        writer.finish().unwrap();
    }
    let provider = make_provider(dir.path(), false);
    let result = extract_archive(&provider, "a.zip", "out", ArchiveFormat::Zip);
    assert!(matches!(result, Err(clouddesk_vfs::VfsError::ReadOnly)));
}

#[test]
fn create_archive_does_not_follow_symlinked_source() {
    use std::os::unix::fs::symlink;

    let source_dir = tempfile::tempdir().unwrap();
    let secret_dir = tempfile::tempdir().unwrap();
    fs::write(secret_dir.path().join("secret.txt"), b"top secret").unwrap();
    symlink(secret_dir.path(), source_dir.path().join("link-to-secret")).unwrap();

    let provider = make_provider(source_dir.path(), true);
    // Creating an archive that selects the symlink must succeed (the
    // symlink itself is simply skipped, per policy) but must never read
    // through it into the linked-to directory.
    let outcome = create_archive(
        &provider,
        &["link-to-secret".to_owned()],
        "out.zip",
        ArchiveFormat::Zip,
    )
    .unwrap();
    assert_eq!(
        outcome.entries, 0,
        "a bare symlink source contributes no entries"
    );

    let dest_dir = tempfile::tempdir().unwrap();
    fs::copy(
        source_dir.path().join("out.zip"),
        dest_dir.path().join("out.zip"),
    )
    .unwrap();
    let dest_provider = make_provider(dest_dir.path(), true);
    let extracted =
        extract_archive(&dest_provider, "out.zip", "extracted", ArchiveFormat::Zip).unwrap();
    assert_eq!(extracted.entries, 0);
    assert!(!dest_dir.path().join("extracted/secret.txt").exists());
}
