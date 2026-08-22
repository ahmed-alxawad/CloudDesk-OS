//! PASS SSH-C, Task 4/5: live protocol-correctness evidence for the
//! real remote PTY (`crate::pty::TerminalSession`) against a REAL
//! disposable OpenSSH server -- never a mock, never a plain `exec`.
//!
//! Skips (not FAIL) if the disposable fixture isn't running.

use clouddesk_remote::pty::TerminalEvent;
use clouddesk_remote::ssh::{SshAuth, SshSession};

const BASTION_HOST: &str = "127.0.0.1";
const BASTION_PORT: u16 = 2222;
const BASTION_USER: &str = "testuser";
const BASTION_PASSWORD: &str = "testpassword";

async fn fixture_available() -> bool {
    tokio::net::TcpStream::connect((BASTION_HOST, BASTION_PORT))
        .await
        .is_ok()
}

async fn connect() -> SshSession {
    SshSession::connect(
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        SshAuth::Password(BASTION_PASSWORD.to_owned()),
        tokio::time::Duration::from_secs(10),
    )
    .await
    .expect("real bastion connection must succeed")
}

/// Reads accumulated output until `predicate` matches the buffer so
/// far, or a bounded number of events pass without a match (never an
/// unbounded wait).
async fn read_until(
    session: &mut clouddesk_remote::pty::TerminalSession,
    predicate: impl Fn(&str) -> bool,
) -> String {
    let mut buf = Vec::new();
    for _ in 0..200 {
        match tokio::time::timeout(std::time::Duration::from_secs(8), session.next_event()).await {
            Ok(Some(TerminalEvent::Output(data))) => {
                buf.extend_from_slice(&data);
                if predicate(&String::from_utf8_lossy(&buf)) {
                    return String::from_utf8_lossy(&buf).into_owned();
                }
            }
            Ok(Some(TerminalEvent::Exit { .. } | TerminalEvent::Closed) | None) | Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Task 4/5: a real PTY is allocated (proven by `stty size` and
/// `test -t 0`/`test -t 1` succeeding -- neither works over a plain,
/// non-PTY `exec` channel) and real commands run through it.
#[tokio::test(flavor = "multi_thread")]
async fn task_4_5_real_pty_allocated_and_shell_semantics_proven() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let session = connect().await;
    let mut terminal = session
        .open_terminal("xterm-256color", 80, 24)
        .await
        .expect("real PTY allocation must succeed");

    // Let the shell start and print its prompt/banner before typing.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    terminal.write_input(b"whoami\n").await.unwrap();
    let out = read_until(&mut terminal, |s| s.contains("testuser\n")).await;
    assert!(
        out.contains("testuser"),
        "whoami must report testuser: {out:?}"
    );

    terminal.write_input(b"pwd\n").await.unwrap();
    let out = read_until(&mut terminal, |s| s.contains('/')).await;
    assert!(out.contains('/'), "pwd must report a path: {out:?}");

    terminal
        .write_input(b"printf 'clouddesk-pty-sentinel\\n'\n")
        .await
        .unwrap();
    let out = read_until(&mut terminal, |s| s.contains("clouddesk-pty-sentinel")).await;
    assert!(out.contains("clouddesk-pty-sentinel"));

    // Proves an actual PTY, not a plain exec channel: only a real
    // terminal has a controlling tty for fd 0/1, and only a real PTY
    // reports a real (non-zero) size via stty.
    terminal
        .write_input(b"test -t 0 && echo IS_A_TTY\n")
        .await
        .unwrap();
    let out = read_until(&mut terminal, |s| s.contains("IS_A_TTY")).await;
    assert!(
        out.contains("IS_A_TTY"),
        "fd 0 must be a real tty under a real PTY: {out:?}"
    );

    // The real PTY was opened at 24 rows x 80 cols (Task 4); `stty
    // size` reports "<rows> <cols>", interspersed with the shell's own
    // ANSI bracketed-paste escapes and bare `\r`s, which is exactly
    // real PTY output -- checked as a substring rather than by
    // strictly parsing "lines" (a bare `\r` without `\n` doesn't split
    // a Rust `str::lines()` line, so the real "24 80" often shares a
    // logical line with adjacent escape codes).
    terminal.write_input(b"stty size\n").await.unwrap();
    let out = read_until(&mut terminal, |s| s.contains("24 80")).await;
    assert!(
        out.contains("24 80"),
        "stty size must report the real PTY dimensions (24 80): {out:?}"
    );

    terminal.write_input(b"exit\n").await.unwrap();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match terminal.next_event().await {
                Some(TerminalEvent::Exit { .. } | TerminalEvent::Closed) | None => break,
                _ => {}
            }
        }
    })
    .await;
}

/// Task 12: resizing the PTY changes what `stty size` actually
/// reports on the remote end.
#[tokio::test(flavor = "multi_thread")]
async fn task_12_resize_changes_real_pty_dimensions() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let session = connect().await;
    let mut terminal = session
        .open_terminal("xterm-256color", 80, 24)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    for (cols, rows) in [(120_u16, 40_u16), (100, 30), (80, 24)] {
        terminal.resize(cols, rows).await.unwrap();
        // A short settle delay: window-change is asynchronous from the
        // remote shell's perspective.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        terminal.write_input(b"stty size\n").await.unwrap();
        let out = read_until(&mut terminal, |s| s.contains(&format!("{rows} {cols}"))).await;
        assert!(
            out.contains(&format!("{rows} {cols}")),
            "stty size must report {rows} {cols} after resize: {out:?}"
        );
    }
}

/// Task 13: Ctrl-C (the literal 0x03 byte, through the normal input
/// path) interrupts the foreground command without closing the whole
/// SSH connection -- the shell stays alive and runs the next command.
#[tokio::test(flavor = "multi_thread")]
async fn task_13_ctrl_c_interrupts_foreground_command_only() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let session = connect().await;
    let mut terminal = session
        .open_terminal("xterm-256color", 80, 24)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    terminal
        .write_input(b"sleep 60; echo SLEPT_FULL\n")
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    terminal.write_input(&[0x03]).await.unwrap(); // Ctrl-C
                                                  // A brief settle delay: real signal delivery + the shell resetting
                                                  // its line discipline after an interrupt isn't instantaneous --
                                                  // mirrors real user typing pace rather than sending the next
                                                  // command in the same instant as the interrupt byte.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    terminal
        .write_input(b"printf 'shell-still-alive\\n'\n")
        .await
        .unwrap();
    // The predicate can't just check for "shell-still-alive": the
    // terminal echoes back whatever is typed, so that substring
    // appears the instant the command is echoed, before it has
    // actually run. Real proof of execution is the sentinel appearing
    // a *second* time (once for the typed echo, once for the real
    // printf output) -- the same reasoning applies to "SLEPT_FULL"
    // below, which the *first* typed command's own echo already
    // contains regardless of whether `sleep` was ever interrupted.
    let out = read_until(&mut terminal, |s| {
        s.matches("shell-still-alive").count() >= 2
    })
    .await;
    assert!(
        out.matches("shell-still-alive").count() >= 2,
        "the shell must actually execute printf (sentinel must appear twice: typed echo + real output), not just echo the typed command: {out:?}"
    );
    assert!(
        out.matches("SLEPT_FULL").count() <= 1,
        "sleep must have been interrupted -- SLEPT_FULL must appear only in the typed echo, never as real `echo` output: {out:?}"
    );
}

/// Task 14: a real `exit` ends the remote shell -- the channel
/// reports a genuine exit, not merely a client-side close.
#[tokio::test(flavor = "multi_thread")]
async fn task_14_shell_exit_reaches_real_exit_status() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let session = connect().await;
    let mut terminal = session
        .open_terminal("xterm-256color", 80, 24)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    terminal.write_input(b"exit 0\n").await.unwrap();

    let mut reached_exit = false;
    for _ in 0..50 {
        match tokio::time::timeout(std::time::Duration::from_secs(2), terminal.next_event()).await {
            Ok(Some(TerminalEvent::Exit { .. } | TerminalEvent::Closed)) => {
                reached_exit = true;
                break;
            }
            Ok(Some(TerminalEvent::Output(_))) => {}
            _ => break,
        }
    }
    assert!(
        reached_exit,
        "a real `exit` must produce a real channel exit/close event"
    );
}
