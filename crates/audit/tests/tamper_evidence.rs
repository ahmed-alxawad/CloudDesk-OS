//! Phase 16C: fresh, executed audit tamper-evidence re-verification
//! (Parts 19-21).
//!
//! `crates/audit/src/lib.rs`'s own unit tests already prove: a normal
//! two-event chain verifies (`events_form_a_verifiable_hash_chain`),
//! ordinary SQL-level `UPDATE`/`DELETE` against `audit_events` is
//! rejected by a database trigger
//! (`audit_rows_cannot_be_updated_or_deleted`), and 24 concurrent
//! writers still produce one verifiable linear chain
//! (`concurrent_writers_produce_one_linear_chain`).
//!
//! What none of those prove: that tampering which *bypasses* the SQL
//! trigger entirely -- an attacker with raw filesystem access to the
//! `.db` file, editing bytes directly while the database is closed,
//! never issuing a single SQL statement -- is actually detected on the
//! next `verify_chain` call. A trigger that blocks `UPDATE`/`DELETE`
//! through the API is not the same claim as "tamper evident"; this
//! file closes that specific gap with real file-level byte edits
//! against a real on-disk `SQLite` database.

use clouddesk_audit::{append, verify_chain, AuditError, NewAuditEvent};
use serde_json::json;

async fn file_backed_pool() -> (tempfile::TempDir, sqlx::SqlitePool, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("audit.db");
    let url = format!("sqlite://{}", db_path.display());
    let pool = clouddesk_db::connect(&url, 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    (directory, pool, db_path)
}

async fn checkpoint_and_close(pool: sqlx::SqlitePool) {
    sqlx::query("PRAGMA wal_checkpoint(FULL)")
        .execute(&pool)
        .await
        .ok();
    pool.close().await;
}

fn event(action: &str, sentinel_metadata: &str) -> NewAuditEvent {
    NewAuditEvent {
        timestamp: 1_700_000_000,
        user_id: Some("user-1".to_owned()),
        role_snapshot: vec!["Administrator".to_owned()],
        session_id_hash: Some("session-hash".to_owned()),
        source_ip: "127.0.0.1".to_owned(),
        user_agent: "test".to_owned(),
        action: action.to_owned(),
        resource_type: "session".to_owned(),
        resource_id: None,
        path: None,
        remote_target: None,
        result: "success".to_owned(),
        metadata: json!({ "note": sentinel_metadata }),
    }
}

/// Part 19: tamper independently with historical record content --
/// bypassing the SQL trigger entirely by editing raw file bytes while
/// the database is closed -- and require `verify_chain` to detect it.
/// Uses a same-length in-place substring replacement so the edit does
/// not corrupt `SQLite`'s own page/cell length encoding (a different-
/// length edit would just produce a corrupt file, which would prove
/// nothing about hash-chain tamper detection specifically).
#[tokio::test]
async fn file_level_tamper_bypassing_sql_triggers_is_detected() {
    let (_directory, pool, db_path) = file_backed_pool().await;

    append(&pool, &event("auth.login", "AAAAAAAAAAAAAAAA"))
        .await
        .unwrap();
    append(&pool, &event("auth.logout", "BBBBBBBBBBBBBBBB"))
        .await
        .unwrap();
    verify_chain(&pool)
        .await
        .expect("untampered chain must verify");

    checkpoint_and_close(pool).await;

    let mut raw = std::fs::read(&db_path).unwrap();
    let needle = b"AAAAAAAAAAAAAAAA";
    let replacement = b"ZZZZZZZZZZZZZZZZ";
    let offset = raw
        .windows(needle.len())
        .position(|window| window == needle)
        .expect(
            "sentinel metadata must be found as raw bytes in the SQLite file -- \
                 confirms this really is a file-level edit, not an API call",
        );
    raw[offset..offset + needle.len()].copy_from_slice(replacement);
    std::fs::write(&db_path, raw).unwrap();

    let url = format!("sqlite://{}", db_path.display());
    let reopened = clouddesk_db::connect(&url, 1).await.unwrap();
    let result = verify_chain(&reopened).await;
    assert!(
        matches!(result, Err(AuditError::InvalidHash(_))),
        "verify_chain must detect a historical record mutated at the file level, \
         bypassing SQL triggers entirely -- got {result:?}"
    );
}

/// Part 19 (deletion): remove a row directly from the file's b-tree
/// (via a fresh connection with triggers *disabled for this session
/// only*, simulating an attacker who has found a way to bypass
/// enforcement locally -- e.g. `PRAGMA writable_schema`+schema edit in
/// a real attack) and require the resulting broken chain to be
/// detected, not merely that the row is gone.
#[tokio::test]
async fn record_deletion_breaks_the_chain_detectably() {
    let (_directory, pool, db_path) = file_backed_pool().await;

    append(&pool, &event("auth.login", "first")).await.unwrap();
    append(&pool, &event("auth.logout", "second"))
        .await
        .unwrap();
    append(&pool, &event("auth.login", "third")).await.unwrap();
    verify_chain(&pool).await.unwrap();
    checkpoint_and_close(pool).await;

    // Open a raw connection and disable the enforcement this specific
    // session would otherwise be subject to, then delete the middle
    // row directly -- modeling an attacker who has found some local
    // bypass of the trigger (compromised superuser tooling, a bug in
    // the trigger itself, direct binary patching), not going through
    // this crate's own `append`/`verify_chain` API at all.
    let url = format!("sqlite://{}", db_path.display());
    let raw_pool = clouddesk_db::connect(&url, 1).await.unwrap();
    sqlx::query("DROP TRIGGER IF EXISTS audit_events_no_update")
        .execute(&raw_pool)
        .await
        .ok();
    sqlx::query("DROP TRIGGER IF EXISTS audit_events_no_delete")
        .execute(&raw_pool)
        .await
        .ok();
    let deleted = sqlx::query("DELETE FROM audit_events WHERE resource_id IS NULL AND id = 2")
        .execute(&raw_pool)
        .await
        .unwrap();
    assert_eq!(
        deleted.rows_affected(),
        1,
        "the deletion itself must have actually happened for this to be a real test of \
         detection, not an accidental no-op"
    );
    raw_pool.close().await;

    let verify_pool = clouddesk_db::connect(&url, 1).await.unwrap();
    let result = verify_chain(&verify_pool).await;
    assert!(
        matches!(
            result,
            Err(AuditError::BrokenChain(_) | AuditError::InvalidHash(_))
        ),
        "verify_chain must detect a deleted historical record (broken hash-chain linkage) -- \
         got {result:?}"
    );
}

/// Part 21: audit secret negative control, in two parts.
///
/// Part A proves the detection technique itself is sound (Part 15's
/// principle applied to log-scanning, not just races): construct an
/// event that -- like a hypothetical buggy call site -- embeds a
/// unique secret sentinel directly in `metadata`, and confirm the
/// sentinel *is* found in the raw file. If this didn't detect the
/// sentinel, "0 leaks" anywhere else would be meaningless (the scan
/// technique itself would be unproven).
///
/// Part B checks what the real production call sites in
/// `services/clouddeskd/src` actually do: none of them may construct
/// `NewAuditEvent`/`.metadata` from a variable whose name indicates a
/// raw secret/password/token/private-key value (as opposed to a
/// label, id, or other non-secret identifier). This is a static
/// check, not a live one -- reconciled here explicitly rather than
/// left as an unstated assumption, since Phase 16A's own static sweep
/// already covered this ground for the whole workspace (clean) and
/// this test exists to make that finding auditable inside the crate
/// that actually owns the audit-log contract, not merely asserted in
/// a document.
#[tokio::test]
async fn audit_events_never_store_the_secret_value_itself() {
    // Part A: detection technique proof.
    let (_directory, pool, db_path) = file_backed_pool().await;
    let secret_sentinel = format!("SENTINEL-SECRET-{}", std::process::id());
    let buggy_event = NewAuditEvent {
        timestamp: 1_700_000_100,
        user_id: Some("user-1".to_owned()),
        role_snapshot: vec!["User".to_owned()],
        session_id_hash: Some("session-hash".to_owned()),
        source_ip: "127.0.0.1".to_owned(),
        user_agent: "test".to_owned(),
        action: "vault.secret.create".to_owned(),
        resource_type: "vault_secret".to_owned(),
        resource_id: Some("secret-record-42".to_owned()),
        path: None,
        remote_target: None,
        result: "success".to_owned(),
        // Deliberately embeds the sentinel, modeling exactly the bug
        // this test exists to catch -- proving the scan below would
        // catch it if a real call site ever did this.
        metadata: json!({ "leaked_value": secret_sentinel }),
    };
    append(&pool, &buggy_event).await.unwrap();
    checkpoint_and_close(pool).await;
    let raw = std::fs::read(&db_path).unwrap();
    let sentinel_found = raw
        .windows(secret_sentinel.len())
        .any(|window| window == secret_sentinel.as_bytes());
    assert!(
        sentinel_found,
        "detection-technique proof failed: a deliberately embedded secret sentinel was not \
         found in the raw file, so this scan technique cannot be trusted to catch a real leak"
    );

    // Part B: real production call sites, scanned from this crate's
    // own test so the claim is checked mechanically, not merely
    // asserted in documentation.
    let clouddeskd_src =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../services/clouddeskd/src");
    assert!(
        clouddeskd_src.is_dir(),
        "expected to find services/clouddeskd/src relative to crates/audit -- repo layout changed?"
    );
    let forbidden_variable_fragments = [
        "password",
        "passphrase",
        "secret_value",
        "private_key",
        "raw_token",
        "plaintext",
    ];
    let mut suspicious_lines = Vec::new();
    for entry in walk_rs_files(&clouddeskd_src) {
        let contents = std::fs::read_to_string(&entry).unwrap();
        for (line_no, line) in contents.lines().enumerate() {
            if !line.contains("metadata") && !line.contains("NewAuditEvent") {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            for fragment in forbidden_variable_fragments {
                if lower.contains(fragment) {
                    suspicious_lines.push(format!(
                        "{}:{}: {}",
                        entry.display(),
                        line_no + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        suspicious_lines.is_empty(),
        "found audit-metadata-adjacent lines referencing secret-shaped variable names in \
         services/clouddeskd/src: {suspicious_lines:#?}"
    );
}

fn walk_rs_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files
}
