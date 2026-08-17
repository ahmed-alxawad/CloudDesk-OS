use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use clouddesk_linux::lookup_user;
use clouddesk_privilege::{
    GrantSigner, PrivdRequest, PrivilegedAction, SignedGrant, TerminalClientMessage,
    TerminalServerMessage, WorkerKind,
};
use clouddesk_vfs::LocalFileOperation;
use cloudesk_privd::PrivdConfig;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    process::Command,
    time::sleep,
};

const CHILD_SOCKET: &str = "CLOUDESK_ROOT_TEST_SOCKET";
const CHILD_GRANT: &str = "CLOUDESK_ROOT_TEST_GRANT";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::similar_names)]
async fn root_helper_launches_worker_as_the_mapped_linux_identity() {
    if rustix::process::geteuid().as_raw() != 0 {
        eprintln!("root boundary test skipped: run this test as root");
        return;
    }

    let identity = lookup_user("daemon")
        .unwrap()
        .expect("the standard daemon account is required");
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let socket_path = directory.path().join("privd.sock");
    let sessiond_path = binary_sibling("cloudesk-sessiond");
    assert!(
        sessiond_path.is_file(),
        "{} is missing",
        sessiond_path.display()
    );
    let key = [31_u8; 32];
    let helper_key = key;
    let helper_socket_path = socket_path.clone();
    let target_uid = identity.uid;
    let target_gid = identity.gid;
    let helper = tokio::spawn(async move {
        cloudesk_privd::run(
            PrivdConfig {
                socket_path: helper_socket_path,
                socket_gid: target_gid,
                allowed_peer_uid: target_uid,
                sessiond_path,
                setpriv_path: "/usr/bin/setpriv".into(),
            },
            &helper_key,
        )
        .await
    });

    for _ in 0..100 {
        if socket_path.exists() {
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    assert!(
        socket_path.exists(),
        "privileged helper socket was not created"
    );

    let now = unix_time();
    let signer = GrantSigner::new(&key).unwrap();
    let grant = signer
        .issue(
            "mapped-test-user",
            "mapped-test-session",
            PrivilegedAction::SpawnUserWorker {
                uid: identity.uid,
                gid: identity.gid,
                worker: WorkerKind::Terminal,
            },
            now,
            30,
        )
        .unwrap();
    assert_child(&invoke_child(&socket_path, &grant, identity.uid, identity.gid).await);

    let grant = signer
        .issue(
            "mapped-test-user",
            "mapped-test-session",
            PrivilegedAction::LocalFileOperation {
                uid: identity.uid,
                gid: identity.gid,
                root: identity.home.to_string_lossy().into_owned(),
                writable: false,
                operation: LocalFileOperation::List {
                    path: "/".to_owned(),
                },
            },
            now,
            30,
        )
        .unwrap();
    assert_child(&invoke_child(&socket_path, &grant, identity.uid, identity.gid).await);

    let grant = signer
        .issue(
            "mapped-test-user",
            "mapped-test-session",
            PrivilegedAction::OpenTerminalSession {
                uid: identity.uid,
                gid: identity.gid,
                rows: 24,
                cols: 80,
                shell: Some("/bin/sh".to_owned()),
            },
            now,
            30,
        )
        .unwrap();
    assert_child(&invoke_child(&socket_path, &grant, identity.uid, identity.gid).await);
    helper.abort();
}

async fn invoke_child(
    socket: &PathBuf,
    grant: &SignedGrant,
    uid: u32,
    gid: u32,
) -> std::process::Output {
    Command::new(env::current_exe().unwrap())
        .arg("--ignored")
        .arg("--exact")
        .arg("privileged_boundary_child")
        .env(CHILD_SOCKET, socket)
        .env(CHILD_GRANT, serde_json::to_string(grant).unwrap())
        .uid(uid)
        .gid(gid)
        .output()
        .await
        .unwrap()
}

fn assert_child(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "child failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "launched as a non-root subprocess by the root boundary test"]
async fn privileged_boundary_child() {
    let Some(socket) = env::var_os(CHILD_SOCKET) else {
        return;
    };
    let grant: SignedGrant = serde_json::from_str(&env::var(CHILD_GRANT).unwrap()).unwrap();
    let file_operation = matches!(
        &grant.claims.action,
        PrivilegedAction::LocalFileOperation { .. }
    );
    let terminal = matches!(
        &grant.claims.action,
        PrivilegedAction::OpenTerminalSession { .. }
    );
    let response = cloudesk_privd::request(&PathBuf::from(socket), &PrivdRequest { grant })
        .await
        .unwrap();
    assert!(response.accepted, "{}", response.message);
    let output = response.output.unwrap();
    if terminal {
        let socket_path = output["socket_path"].as_str().unwrap();
        let mut stream = UnixStream::connect(socket_path).await.unwrap();
        write_frame(
            &mut stream,
            &TerminalClientMessage::Data {
                data: b"printf CLOUDESK_PTY_OK; exit\n".to_vec(),
            },
        )
        .await;
        let mut terminal_output = Vec::new();
        loop {
            let message: TerminalServerMessage = read_frame(&mut stream).await;
            match message {
                TerminalServerMessage::Output { data } => terminal_output.extend(data),
                TerminalServerMessage::Exit { .. } => break,
                TerminalServerMessage::Error { message } => panic!("{message}"),
            }
        }
        assert!(String::from_utf8_lossy(&terminal_output).contains("CLOUDESK_PTY_OK"));
    } else if file_operation {
        assert_eq!(output["result"], "entries");
        assert!(output["entries"].is_array());
    } else {
        let identity: &Value = &output["identity"];
        assert_ne!(identity["effective_uid"], 0);
        assert_ne!(identity["effective_gid"], 0);
    }
}

async fn write_frame(stream: &mut UnixStream, message: &TerminalClientMessage) {
    let bytes = serde_json::to_vec(message).unwrap();
    stream
        .write_u32(u32::try_from(bytes.len()).unwrap())
        .await
        .unwrap();
    stream.write_all(&bytes).await.unwrap();
}

async fn read_frame(stream: &mut UnixStream) -> TerminalServerMessage {
    let length = stream.read_u32().await.unwrap() as usize;
    let mut bytes = vec![0_u8; length];
    stream.read_exact(&mut bytes).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn binary_sibling(name: &str) -> PathBuf {
    env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(name)
}

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}
