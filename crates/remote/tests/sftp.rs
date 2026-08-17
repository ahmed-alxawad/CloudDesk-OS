//! Regression tests for CLAUDE-NIGHTMARE-003 (SFTP `write_file` could never
//! create a new remote file) and CLAUDE-NIGHTMARE-004 (`SftpProvider::list`
//! broke on any non-chrooted SFTP server, which is the common case).
//!
//! Uses a minimal in-process SFTP server (no chroot: it deliberately fails
//! any absolute `/name` request, exactly like a real, non-chrooted OpenSSH
//! `sftp-server` run against a home directory that isn't `/`) so the tests
//! don't depend on Docker/real infrastructure.

use clouddesk_remote::ssh::{SshAuth, SshSession};
use clouddesk_vfs::VfsProvider;
use russh::keys::ssh_key::PrivateKey;
use russh::server::{Auth, Handler as SshHandler, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{FileAttributes, Handle, Name, Status, StatusCode};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;

#[derive(Clone, Default)]
struct MockServer;

impl russh::server::Server for MockServer {
    type Handler = MockSession;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
        MockSession::default()
    }
}

#[derive(Default)]
struct MockSession {
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl SshHandler for MockSession {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, _user: &str, _password: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.lock().await.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        let channel = self.channels.lock().await.remove(&channel_id).unwrap();
        session.channel_success(channel_id)?;
        russh_sftp::server::run(channel.into_stream(), NonChrootSftp::default()).await;
        Ok(())
    }
}

/// Simulates a real, non-chrooted OpenSSH `sftp-server`: the login lands in
/// a home directory that is NOT the filesystem root, so entries are
/// addressed relative to "." (the home dir) and an absolute "/name" request
/// deliberately fails — exactly like the real disposable OpenSSH fixture
/// this was first reproduced against.
#[derive(Default)]
struct NonChrootSftp {
    files: HashMap<String, Vec<u8>>,
    dir_listed: bool,
}

impl russh_sftp::server::Handler for NonChrootSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<russh_sftp::protocol::Version, Self::Error> {
        self.files
            .insert("existing.bin".to_owned(), b"seed".to_vec());
        Ok(russh_sftp::protocol::Version::new())
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        if path != "." {
            // Real behaviour outside a chroot: absolute/foreign paths do
            // not resolve to the home directory's contents.
            return Err(StatusCode::NoSuchFile);
        }
        self.dir_listed = false;
        Ok(Handle { id, handle: path })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        if handle == "." && !self.dir_listed {
            self.dir_listed = true;
            return Ok(Name {
                id,
                files: self
                    .files
                    .keys()
                    .map(|name| russh_sftp::protocol::File::new(name, FileAttributes::dummy()))
                    .collect(),
            });
        }
        Err(StatusCode::Eof)
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".to_owned(),
            language_tag: "en-US".to_owned(),
        })
    }

    async fn stat(
        &mut self,
        id: u32,
        path: String,
    ) -> Result<russh_sftp::protocol::Attrs, Self::Error> {
        if self.files.contains_key(&path) {
            Ok(russh_sftp::protocol::Attrs {
                id,
                attrs: FileAttributes::dummy(),
            })
        } else {
            Err(StatusCode::NoSuchFile)
        }
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: russh_sftp::protocol::OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        if pflags.contains(russh_sftp::protocol::OpenFlags::CREATE) {
            self.files.entry(filename.clone()).or_default();
        } else if !self.files.contains_key(&filename) {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(Handle {
            id,
            handle: filename,
        })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        _offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        self.files
            .entry(handle)
            .or_default()
            .extend_from_slice(&data);
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".to_owned(),
            language_tag: "en-US".to_owned(),
        })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<russh_sftp::protocol::Data, Self::Error> {
        let content = self.files.get(&handle).ok_or(StatusCode::NoSuchFile)?;
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        if offset >= content.len() {
            return Err(StatusCode::Eof);
        }
        let end = (offset + len as usize).min(content.len());
        Ok(russh_sftp::protocol::Data {
            id,
            data: content[offset..end].to_vec(),
        })
    }
}

// Fixed ed25519 test key (same one used by the sibling `ssh.rs` mock
// server) — avoids depending on a workspace-compatible CSPRNG API for a
// throwaway server identity.
const TEST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBswrGY1nEnqW9lIuvMn5oFfozsW7dJMC2oodSq2WFYowAAAJg7NuFaOzbh\nWgAAAAtzc2gtZWQyNTUxOQAAACBswrGY1nEnqW9lIuvMn5oFfozsW7dJMC2oodSq2WFYow\nAAAED8ijpQnhgmGHVQIr+UJ/ybod+6d22qAlbEH6ny0CwchmzCsZjWcSepb2Ui68yfmgV+\njOxbt0kwLaih1KrZYVijAAAADmNsb3VkZGVzay10ZXN0AQIDBAUGBw==\n-----END OPENSSH PRIVATE KEY-----\n";

async fn start_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let key = PrivateKey::from_openssh(TEST_KEY).unwrap();
        let config = russh::server::Config {
            auth_rejection_time: Duration::from_secs(1),
            auth_rejection_time_initial: Some(Duration::from_secs(0)),
            keys: vec![key],
            ..Default::default()
        };
        let mut server = MockServer;
        if let Ok((socket, _)) = listener.accept().await {
            let _ =
                russh::server::run_stream(Arc::new(config), socket, server.new_client(None)).await;
        }
    });
    port
}

async fn connect(port: u16) -> SshSession {
    SshSession::connect(
        "127.0.0.1",
        port,
        "testuser",
        SshAuth::Password("anything".into()),
        Duration::from_secs(5),
    )
    .await
    .expect("connect")
}

/// CLAUDE-NIGHTMARE-004: listing the root directory must succeed against a
/// non-chrooted SFTP server, and must never probe an absolute `/name` path
/// (which resolves outside the user's home on such a server).
#[tokio::test(flavor = "multi_thread")]
async fn list_root_succeeds_against_a_non_chrooted_sftp_server() {
    let port = start_server().await;
    let mut session = connect(port).await;
    let sftp = session.open_sftp_session().await.unwrap();
    let handle = tokio::runtime::Handle::current();
    let provider = clouddesk_remote::sftp::SftpProvider::new(sftp, handle);

    let entries = provider
        .list("/")
        .expect("listing the root must succeed even without a chroot");
    let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"existing.bin".to_owned()));
}

/// CLAUDE-NIGHTMARE-003: uploading a file that does not already exist on
/// the remote must succeed (create-or-overwrite), not fail with
/// "No such file".
#[tokio::test(flavor = "multi_thread")]
async fn write_file_creates_a_new_remote_file() {
    let port = start_server().await;
    let mut session = connect(port).await;
    let sftp = session.open_sftp_session().await.unwrap();
    let handle = tokio::runtime::Handle::current();
    let provider = clouddesk_remote::sftp::SftpProvider::new(sftp, handle);

    let content = b"brand new content that never existed on the remote".to_vec();
    provider
        .write_file("/brand-new.bin", &content)
        .expect("uploading a new file must succeed");

    let read_back = provider
        .read_limited("/brand-new.bin", content.len())
        .expect("the newly created file must be readable");
    assert_eq!(read_back, content);
}
