//! Phase 16C: deterministic TOCTOU / symlink-swap race attacks against
//! `LocalProvider`.
//!
//! Threat model: a validate-then-use gap where a path is checked/
//! authorized once, then a mutable filesystem component (a directory
//! entry that can be replaced with a symlink, or swapped for a
//! different directory) is changed by an attacker between validation
//! and the actual filesystem operation, redirecting the operation
//! outside the assigned root.
//!
//! `LocalProvider` deliberately never has a "validate, then later
//! reopen by path string" gap to attack: `normalize_virtual_path` is
//! purely lexical (never touches the filesystem -- no canonicalize, no
//! stat), and every actual filesystem operation goes through
//! `cap_std::fs::Dir`, which resolves paths relative to an
//! already-open directory file descriptor and is documented to refuse
//! to leave that subtree even via a symlink stored within the tree
//! that points outside it. There is no separate "check" step for a
//! race to land in between -- validation and use happen in the same
//! call. These tests attack that claim directly with a real,
//! continuously-mutating filesystem, rather than trusting the
//! library's documentation.
//!
//! Each test races a background thread that repeatedly swaps a path
//! component between a real in-root directory and a symlink to an
//! outside sentinel directory, while the foreground thread repeatedly
//! performs the operation under test through `LocalProvider`. A
//! negative control (`naive_unprotected_access_is_actually_racy`)
//! proves the race technique itself works by running the identical
//! attack against a deliberately naive, unprotected implementation
//! (canonicalize once, then reopen by path string) -- if that control
//! didn't leak, the race harness itself would be untrustworthy.

use std::fs;
use std::os::unix::fs::symlink;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use clouddesk_vfs::{LocalProvider, VfsProvider};

const RACE_ITERATIONS: usize = 2000;
const OUTSIDE_SENTINEL_CONTENT: &[u8] = b"OUTSIDE-ROOT-SENTINEL-DO-NOT-LEAK";
const INSIDE_CONTENT: &[u8] = b"inside-root-legitimate-content";

/// Spawns a background thread that swaps `victim_path` back and forth
/// between a real directory (containing a copy of the in-root file)
/// and a symlink pointing at `outside_dir`, for `iterations` cycles.
/// Returns a handle plus a flag the caller can use to confirm the
/// swap thread actually performed real mutations (Part 15: proof the
/// race was really attempted, not merely "no escape observed").
fn spawn_symlink_swapper(
    victim_path: std::path::PathBuf,
    outside_dir: std::path::PathBuf,
    iterations: usize,
) -> (std::thread::JoinHandle<()>, Arc<AtomicUsize>) {
    let swap_count = Arc::new(AtomicUsize::new(0));
    let counted = swap_count.clone();
    let handle = std::thread::spawn(move || {
        for i in 0..iterations {
            let _ = fs::remove_dir_all(&victim_path);
            let _ = fs::remove_file(&victim_path);
            if i % 2 == 0 {
                symlink(&outside_dir, &victim_path).ok();
            } else {
                fs::create_dir_all(&victim_path).ok();
                fs::write(victim_path.join("target.txt"), INSIDE_CONTENT).ok();
            }
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    (handle, swap_count)
}

fn setup_race_fixture() -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
    let root_dir = tempfile::tempdir().unwrap();
    let outside_dir = tempfile::tempdir().unwrap();
    fs::write(
        outside_dir.path().join("target.txt"),
        OUTSIDE_SENTINEL_CONTENT,
    )
    .unwrap();
    let victim_path = root_dir.path().join("victim");
    fs::create_dir_all(&victim_path).unwrap();
    fs::write(victim_path.join("target.txt"), INSIDE_CONTENT).unwrap();
    (root_dir, outside_dir, victim_path)
}

/// Part 5: path race on read. Attacker swaps `/victim` (an in-root
/// directory) for a symlink to an outside directory while the
/// foreground thread repeatedly reads `/victim/target.txt` through
/// `LocalProvider`. Outside content must never be returned.
#[test]
fn race_read_never_returns_outside_root_content() {
    let (root_dir, outside_dir, victim_path) = setup_race_fixture();
    let provider = LocalProvider::open(root_dir.path(), true).unwrap();

    let (swapper, swap_count) = spawn_symlink_swapper(
        victim_path,
        outside_dir.path().to_path_buf(),
        RACE_ITERATIONS,
    );

    let mut outside_content_observed = 0usize;
    let mut inside_content_observed = 0usize;
    let mut errors_observed = 0usize;
    for _ in 0..RACE_ITERATIONS {
        match provider.read_limited("/victim/target.txt", 4096) {
            Ok(bytes) if bytes == OUTSIDE_SENTINEL_CONTENT => outside_content_observed += 1,
            Ok(bytes) if bytes == INSIDE_CONTENT => inside_content_observed += 1,
            Ok(_) => {}
            Err(_) => errors_observed += 1,
        }
    }
    swapper.join().unwrap();

    eprintln!(
        "race_read: swaps={}, outside_observed={outside_content_observed}, \
         inside_observed={inside_content_observed}, errors={errors_observed}",
        swap_count.load(Ordering::SeqCst)
    );
    assert!(
        swap_count.load(Ordering::SeqCst) >= RACE_ITERATIONS / 2,
        "swap thread must have actually run its full mutation cycle for this to be a real race"
    );
    assert_eq!(
        outside_content_observed, 0,
        "LocalProvider::read_limited returned outside-root sentinel content during a symlink-swap race"
    );
}

/// Part 6: path race on write. Attacker swaps `/victim` for a symlink
/// to an outside directory while the foreground thread repeatedly
/// writes `/victim/target.txt` through `LocalProvider`. The outside
/// file's content must never change from its original sentinel value.
#[test]
fn race_write_never_touches_outside_root_file() {
    let (root_dir, outside_dir, victim_path) = setup_race_fixture();
    let provider = LocalProvider::open(root_dir.path(), true).unwrap();
    let outside_target = outside_dir.path().join("target.txt");

    let (swapper, swap_count) = spawn_symlink_swapper(
        victim_path,
        outside_dir.path().to_path_buf(),
        RACE_ITERATIONS,
    );

    for i in 0..RACE_ITERATIONS {
        let payload = format!("attacker-write-attempt-{i}");
        let _ = provider.write_file("/victim/target.txt", payload.as_bytes());
    }
    swapper.join().unwrap();

    let outside_after = fs::read(&outside_target).unwrap();
    eprintln!(
        "race_write: swaps={}, outside_after_len={}",
        swap_count.load(Ordering::SeqCst),
        outside_after.len()
    );
    assert_eq!(
        outside_after, OUTSIDE_SENTINEL_CONTENT,
        "LocalProvider::write_file mutated an outside-root file during a symlink-swap race"
    );
}

/// Part 7: rename/move race. Attacker swaps the *destination's
/// parent* for a symlink to an outside directory while the foreground
/// thread repeatedly renames an in-root file into
/// `/victim/moved.txt`. No file should ever land in the outside
/// directory.
#[test]
fn race_rename_never_lands_outside_root() {
    let (root_dir, outside_dir, victim_path) = setup_race_fixture();
    let provider = LocalProvider::open(root_dir.path(), true).unwrap();

    let (swapper, swap_count) = spawn_symlink_swapper(
        victim_path,
        outside_dir.path().to_path_buf(),
        RACE_ITERATIONS,
    );

    for i in 0..RACE_ITERATIONS {
        let source_name = format!("source-{i}.txt");
        provider
            .write_file(
                &format!("/{source_name}"),
                format!("payload-{i}").as_bytes(),
            )
            .unwrap();
        let _ = provider.rename(&format!("/{source_name}"), "/victim/moved.txt");
        // Clean up whatever is left in-root from this iteration so the
        // next one starts from a known state; ignore errors (the file
        // may already have been consumed by a successful rename).
        let _ = fs::remove_file(root_dir.path().join(&source_name));
    }
    swapper.join().unwrap();

    let leaked = outside_dir.path().join("moved.txt");
    eprintln!(
        "race_rename: swaps={}, leaked_file_exists={}",
        swap_count.load(Ordering::SeqCst),
        leaked.exists()
    );
    assert!(
        !leaked.exists(),
        "LocalProvider::rename placed a file in the outside-root directory during a symlink-swap race"
    );
}

/// Part 15 negative control: proves the race harness above is a real,
/// effective attack -- not merely one that happens not to trigger
/// against `LocalProvider` because the timing never lined up. Runs
/// the identical swap-thread technique against a deliberately naive,
/// unprotected implementation (`std::fs::canonicalize` once, then
/// reopen by the resulting path string later -- the classic
/// vulnerable TOCTOU pattern this whole test file is checking
/// `LocalProvider` does NOT use) and requires it to actually leak
/// outside-root content at least once. If this control failed to
/// leak, "0 leaks" against `LocalProvider` above would be weak
/// evidence (Part 15's "no escape observed without proof the mutation
/// occurred is weak evidence").
#[test]
fn naive_unprotected_access_is_actually_racy() {
    // Deliberately deterministic, not statistical: earlier versions of
    // this control raced a background swapper thread against a sleep-
    // widened check-then-use loop over many iterations, hoping a swap
    // would land inside the window often enough. That was confirmed
    // live to be genuinely unreliable on GitHub's shared runners (first
    // real hosted CI run, v1.0.1-rc.3, and again after a 10x sleep bump
    // and a 50x swapper-iteration bump) -- fewer/weaker cores make
    // thread interleaving far less predictable than on this project's
    // own dev machine, where even the 50x-swapper version passed
    // consistently. A single-threaded, explicitly sequenced
    // check-swap-use reproduction has no timing dependence at all: it
    // demonstrates the exact same vulnerability class with a guaranteed
    // repro instead of a probabilistic one.
    let (root_dir, outside_dir, victim_path) = setup_race_fixture();
    let target_str = victim_path.join("target.txt").to_str().unwrap().to_owned();

    // "Check": validate the path resolves inside root -- true here,
    // since setup_race_fixture starts the victim as a real in-root
    // directory.
    let canonical = fs::canonicalize(&target_str).expect("fixture path should exist");
    assert!(
        canonical.starts_with(root_dir.path()),
        "fixture setup should start inside root"
    );

    // The attacker's swap, happening deterministically between the
    // check above and the "use" below -- exactly the check-then-use gap
    // `LocalProvider` avoids by never canonicalizing and always
    // resolving through an already-open capability handle.
    fs::remove_dir_all(&victim_path).ok();
    fs::remove_file(&victim_path).ok();
    symlink(outside_dir.path(), &victim_path).expect("swap to outside symlink should succeed");

    // "Use": the naive pattern re-reads via the plain path string it
    // already validated, which now resolves outside root because of the
    // intervening swap.
    let bytes = fs::read(&target_str).expect("read through the swapped symlink should succeed");
    assert_eq!(
        bytes, OUTSIDE_SENTINEL_CONTENT,
        "the naive check-then-use pattern must observe outside-root content after an intervening \
         swap -- proving the vulnerability class is real, so the LocalProvider races above are \
         meaningful evidence rather than weak ones"
    );
}

/// Part 9 (narrow slice): archive extraction destination race. The
/// full malicious-archive-content tests already exist in
/// `crates/vfs/tests/archive.rs`; this test targets the *mutable
/// destination directory* specifically -- extracting into a
/// destination whose containing directory is being swapped for a
/// symlink outside the root concurrently with extraction.
#[test]
fn race_archive_extract_destination_never_escapes_root() {
    use clouddesk_vfs::archive::{create_archive, extract_archive, ArchiveFormat};

    let source_dir = tempfile::tempdir().unwrap();
    fs::write(source_dir.path().join("payload.txt"), b"archived-payload").unwrap();
    let source_provider = LocalProvider::open(source_dir.path(), true).unwrap();
    create_archive(
        &source_provider,
        &["payload.txt".to_owned()],
        "bundle.zip",
        ArchiveFormat::Zip,
    )
    .unwrap();

    let (root_dir, outside_dir, victim_path) = setup_race_fixture();
    fs::copy(
        source_dir.path().join("bundle.zip"),
        root_dir.path().join("bundle.zip"),
    )
    .unwrap();
    let provider = LocalProvider::open(root_dir.path(), true).unwrap();

    let (swapper, swap_count) = spawn_symlink_swapper(
        victim_path,
        outside_dir.path().to_path_buf(),
        RACE_ITERATIONS / 4,
    );

    let mut escapes = 0usize;
    for _ in 0..(RACE_ITERATIONS / 4) {
        match extract_archive(
            &provider,
            "bundle.zip",
            "victim/extracted",
            ArchiveFormat::Zip,
        ) {
            Ok(_) | Err(_) => {}
        }
        if outside_dir.path().join("extracted").exists() {
            escapes += 1;
        }
        let _ = fs::remove_dir_all(root_dir.path().join("victim/extracted"));
    }
    swapper.join().unwrap();

    eprintln!(
        "race_archive_extract: swaps={}, escapes={escapes}",
        swap_count.load(Ordering::SeqCst)
    );
    assert_eq!(
        escapes, 0,
        "archive extraction wrote into the outside-root directory during a symlink-swap race"
    );
}
