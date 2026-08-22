//! PASS SSH-C: a real remote SSH PTY -- `SSH_MSG_CHANNEL_REQUEST`
//! `pty-req` + `shell` on a real SSH channel, over the exact same
//! authenticated `SshSession` every other feature (SFTP, SCP, advanced
//! auth) already uses. Never a local shell, never a one-shot `exec`,
//! never a second SSH stack.

use anyhow::Result;
use russh::{client::Msg, Channel, ChannelMsg, Sig};

/// An event surfaced from the remote PTY, translated from the
/// underlying SSH channel messages into the small vocabulary the
/// terminal WebSocket bridge actually needs.
pub enum TerminalEvent {
    /// Bytes the remote shell/PTY produced (stdout and stderr are not
    /// distinguished once a PTY is allocated -- exactly like a real
    /// terminal).
    Output(Vec<u8>),
    /// The remote shell exited. `code` is `None` when the remote
    /// closed without reporting a numeric status (e.g. killed by a
    /// signal) -- never fabricated as `0`.
    Exit { code: Option<u32> },
    /// The channel/connection ended without an explicit exit status
    /// (e.g. the transport died).
    Closed,
}

/// A live remote PTY -- owns the whole underlying `SshSession`
/// (including any `ProxyJump` bastion hop) for exactly as long as the
/// terminal is open, so the connection can never be torn down out
/// from under an active shell.
pub struct TerminalSession {
    _connection: crate::ssh::SshSession,
    channel: Channel<Msg>,
}

impl TerminalSession {
    pub(crate) async fn open(
        mut connection: crate::ssh::SshSession,
        term: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Self> {
        let channel = connection.handle_mut().channel_open_session().await?;
        // Task 4: a real PTY request, not a plain exec. Terminal modes
        // left empty (defaults) -- v1 doesn't need custom termios bits.
        channel
            .request_pty(true, term, u32::from(cols), u32::from(rows), 0, 0, &[])
            .await?;
        channel.request_shell(true).await?;
        Ok(Self {
            _connection: connection,
            channel,
        })
    }

    /// Sends raw bytes as terminal input -- including control bytes
    /// like `0x03` (Ctrl-C), exactly as a real terminal client does:
    /// through the normal input path, never a separate SSH protocol
    /// signal request (Task 13).
    pub async fn write_input(&mut self, data: &[u8]) -> Result<()> {
        self.channel.data(data).await?;
        Ok(())
    }

    /// Task 12: a real `window-change` channel request -- the remote
    /// PTY's actual dimensions change, not merely client-side display
    /// state.
    pub async fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.channel
            .window_change(u32::from(cols), u32::from(rows), 0, 0)
            .await?;
        Ok(())
    }

    /// Waits for the next channel event, translated to the bridge's
    /// vocabulary. Returns `None` only when the channel is fully
    /// exhausted (no more events will ever arrive).
    pub async fn next_event(&mut self) -> Option<TerminalEvent> {
        loop {
            match self.channel.wait().await? {
                ChannelMsg::Data { data } => return Some(TerminalEvent::Output(data.to_vec())),
                ChannelMsg::ExtendedData { data, .. } => {
                    return Some(TerminalEvent::Output(data.to_vec()))
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    return Some(TerminalEvent::Exit {
                        code: Some(exit_status),
                    })
                }
                ChannelMsg::ExitSignal { .. } => return Some(TerminalEvent::Exit { code: None }),
                ChannelMsg::Eof | ChannelMsg::Close => return Some(TerminalEvent::Closed),
                _ => {}
            }
        }
    }

    /// A best-effort SIGINT via the SSH protocol-level signal request
    /// (RFC 4254 6.9) -- not the mechanism Task 13 requires (which is
    /// the literal `0x03` byte through the normal input path, since
    /// that's what a real terminal client does and what real remote
    /// shells' line disciplines actually act on), but kept available
    /// for a caller that wants both.
    pub async fn send_sigint(&mut self) -> Result<()> {
        self.channel.signal(Sig::INT).await?;
        Ok(())
    }

    pub async fn close(&mut self) -> Result<()> {
        let _ = self.channel.eof().await;
        self.channel.close().await?;
        Ok(())
    }
}
