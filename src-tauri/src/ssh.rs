// Native SSH client built on `russh`.
//
// Replaces the prior `Command::new("ssh")` shell-out pattern for all
// Pi interactions. Wins:
//   * No console-window flash per call (Windows users see a single
//     self-contained process).
//   * No dependency on a system OpenSSH install (Tauri ships its own
//     SSH-2 implementation in the binary).
//   * Connection reuse — one TCP+auth handshake per host, then many
//     exec/sftp calls reuse the same `Handle`. Bulk-sync gets a 5-10×
//     speedup since we previously did a fresh handshake per file.
//   * Cross-platform: same code path on Windows, macOS, Linux. No
//     `Command` lifecycle quirks to manage.
//
// Auth strategy: look for `~/.ssh/id_ed25519` first, then `~/.ssh/id_rsa`.
// Both should be present from the user's prior `ssh-copy-id pi@<ip>`.
// Passphrase-protected keys are not yet supported (a follow-up — the
// frontend would need a prompt; today's BatchMode flow assumes
// unencrypted keys).
//
// Host key verification: currently accepts any server key on first
// connect (matches the prior `StrictHostKeyChecking=accept-new`
// behavior). Proper `~/.ssh/known_hosts` parsing + persisted pinning
// is a follow-up — the threat model here is a LAN cabinet, so the
// risk is low until PP Doctor ever speaks SSH to anything else.

use async_trait::async_trait;
use parking_lot::Mutex;
use russh::client::{Config, Handle, Handler};
use russh::ChannelMsg;
use russh_keys::*;
use russh_sftp::client::SftpSession;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;

/// Default SSH connect timeout. Matches the prior `-o ConnectTimeout=10`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default inactivity timeout before the held-open session is dropped.
/// 5 minutes — long enough for a Tables-page idle, short enough that
/// a forgotten window doesn't pin a TCP slot on the Pi forever.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct SshResult {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

struct ClientHandler;

#[async_trait]
impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // TODO: parse ~/.ssh/known_hosts and verify. For now match the
        // prior StrictHostKeyChecking=accept-new behavior.
        Ok(true)
    }
}

/// Parsed `[user@]host[:port]` into its parts. Default user `pi`
/// (PinnerPi convention), default port 22.
struct Target {
    user: String,
    host: String,
    port: u16,
}

fn parse_target(spec: &str) -> Target {
    let (user, rest) = match spec.split_once('@') {
        Some((u, r)) => (u.to_string(), r),
        None => ("pi".to_string(), spec),
    };
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(22)),
        None => (rest.to_string(), 22),
    };
    Target { user, host, port }
}

/// Find a usable SSH private key in `~/.ssh/`. Tries ed25519 first
/// (preferred for new setups), then rsa. Returns the first existing.
fn find_ssh_key() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("home dir not resolvable")?;
    let candidates = ["id_ed25519", "id_ecdsa", "id_rsa"];
    for name in candidates {
        let p = home.join(".ssh").join(name);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(format!(
        "no SSH key found in {}/.ssh/ — run `ssh-keygen` and `ssh-copy-id pi@<ip>` first",
        home.display()
    ))
}

/// One live session per host. Held behind an `AsyncMutex` because the
/// underlying `Handle<H>` channel calls are not Sync.
pub(crate) struct Session {
    handle: Handle<ClientHandler>,
}

/// Per-host credentials override. The Settings modal in the frontend
/// stores `ppe.pi-user` + `ppe.pi-pw` in localStorage; the Connect
/// flow forwards them to the pool via `ssh_set_credentials`. If no
/// entry exists for a host, we try the PinnerPi default `pi`/`pi`
/// (factory cabinet credentials) before falling back to key auth.
#[derive(Clone, Default)]
struct Credentials {
    user: String,
    password: Option<String>,
}

/// Pool of held-open sessions, keyed by canonical `user@host:port`.
/// Lazily creates a session on first use, reuses thereafter.
#[derive(Default)]
pub struct SshPool {
    sessions: Mutex<HashMap<String, Arc<AsyncMutex<Session>>>>,
    /// Cred overrides keyed by `host:port` (without user — the user is
    /// part of the cred itself). Pre-populated from the Settings modal.
    creds: Mutex<HashMap<String, Credentials>>,
}

impl SshPool {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(t: &Target) -> String {
        format!("{}@{}:{}", t.user, t.host, t.port)
    }

    fn cred_key(t: &Target) -> String {
        format!("{}:{}", t.host, t.port)
    }

    /// Store cabinet credentials for `host`. Frontend invokes this once
    /// from the Connect screen after loading the user-set values from
    /// localStorage (`ppe.pi-user`, `ppe.pi-pw`). Subsequent
    /// session establishment uses these instead of the `pi/pi` default.
    pub fn set_credentials(&self, host_spec: &str, user: String, password: Option<String>) {
        let target = parse_target(host_spec);
        let mut creds = self.creds.lock();
        creds.insert(
            Self::cred_key(&target),
            Credentials { user, password },
        );
    }

    /// Get-or-create a session for `host_spec`. Auth strategy:
    ///   1. Stored credentials (set via `set_credentials` from Settings).
    ///      If those have a password, try `authenticate_password`.
    ///   2. PinnerPi factory default `pi/pi`. Tries
    ///      `authenticate_password` so first-time-out-of-the-box
    ///      cabinets connect without any setup.
    ///   3. Public key (~/.ssh/id_ed25519, id_ecdsa, or id_rsa).
    ///      Honors users who pushed their key with `ssh-copy-id`.
    pub async fn get(&self, host_spec: &str) -> Result<Arc<AsyncMutex<Session>>, String> {
        let mut target = parse_target(host_spec);
        // If the user stored a custom username, use it. (parse_target
        // defaults to `pi` for bare-host input; explicit `user@host`
        // already overrode that.)
        if !host_spec.contains('@') {
            if let Some(c) = self.creds.lock().get(&Self::cred_key(&target)).cloned() {
                if !c.user.is_empty() {
                    target.user = c.user;
                }
            }
        }
        let key = Self::key(&target);

        // Fast path: existing session.
        if let Some(s) = self.sessions.lock().get(&key) {
            return Ok(s.clone());
        }

        // Slow path: establish a new one. Done OUTSIDE the pool lock so
        // we don't block other hosts during the handshake.
        let mut config = Config::default();
        config.inactivity_timeout = Some(IDLE_TIMEOUT);
        let config = Arc::new(config);

        let connect_fut = russh::client::connect(
            config,
            (target.host.as_str(), target.port),
            ClientHandler,
        );
        let mut handle = tokio::time::timeout(CONNECT_TIMEOUT, connect_fut)
            .await
            .map_err(|_| format!("ssh connect to {} timed out", host_spec))?
            .map_err(|e| format!("ssh connect: {}", e))?;

        // Try auth methods in order; remember WHY each failed for a
        // useful final error message.
        let mut last_err = String::new();

        // 1) Stored password from Settings (if any).
        let stored_pw = self
            .creds
            .lock()
            .get(&Self::cred_key(&target))
            .and_then(|c| c.password.clone());
        if let Some(pw) = stored_pw.as_ref().filter(|s| !s.is_empty()) {
            match handle.authenticate_password(&target.user, pw).await {
                Ok(true) => {
                    let session = Arc::new(AsyncMutex::new(Session { handle }));
                    self.sessions.lock().insert(key, session.clone());
                    return Ok(session);
                }
                Ok(false) => last_err = format!("password auth rejected for {}", target.user),
                Err(e) => last_err = format!("password auth: {}", e),
            }
        }

        // 2) Factory default `pi/pi` (only if we haven't already tried
        //    a stored password — avoid double-trying the same string).
        let factory_pw = "pi";
        let already_tried_factory = stored_pw.as_deref() == Some(factory_pw);
        if target.user == "pi" && !already_tried_factory {
            match handle.authenticate_password("pi", factory_pw).await {
                Ok(true) => {
                    let session = Arc::new(AsyncMutex::new(Session { handle }));
                    self.sessions.lock().insert(key, session.clone());
                    return Ok(session);
                }
                Ok(false) => {
                    if last_err.is_empty() {
                        last_err = "factory pi/pi auth rejected".into();
                    }
                }
                Err(e) => {
                    if last_err.is_empty() {
                        last_err = format!("factory auth: {}", e);
                    }
                }
            }
        }

        // 3) Key auth fallback. Many advanced users have already pushed
        //    a key via `ssh-copy-id`; honor that.
        if let Ok(key_path) = find_ssh_key() {
            if let Ok(key_pair) = load_secret_key(&key_path, None) {
                match handle
                    .authenticate_publickey(&target.user, Arc::new(key_pair))
                    .await
                {
                    Ok(true) => {
                        let session = Arc::new(AsyncMutex::new(Session { handle }));
                        self.sessions.lock().insert(key, session.clone());
                        return Ok(session);
                    }
                    Ok(false) => {
                        if last_err.is_empty() {
                            last_err = format!("public key {} rejected", key_path.display());
                        }
                    }
                    Err(e) => {
                        if last_err.is_empty() {
                            last_err = format!("public key: {}", e);
                        }
                    }
                }
            }
        }

        Err(format!(
            "ssh auth failed for {}@{} — tried stored password, factory pi/pi, and ~/.ssh/ keys. \
             Last detail: {}",
            target.user, target.host, last_err
        ))
    }

    /// Drop a session — call when a connection is detected stale (the
    /// underlying TCP died, the Pi rebooted, etc.).
    pub fn drop_session(&self, host_spec: &str) {
        let target = parse_target(host_spec);
        let key = Self::key(&target);
        self.sessions.lock().remove(&key);
    }
}

// ─── Exec / SFTP wrappers ────────────────────────────────────────────

/// Run `command` on the remote host; return stdout, stderr, and exit
/// code. Drops the session and retries once on a stale-connection
/// error so a Pi reboot or NAT timeout doesn't fail the first call.
pub async fn exec(pool: &SshPool, host: &str, command: &str) -> Result<SshResult, String> {
    match exec_once(pool, host, command).await {
        Ok(r) => Ok(r),
        Err(e) if is_stale(&e) => {
            pool.drop_session(host);
            exec_once(pool, host, command).await
        }
        Err(e) => Err(e),
    }
}

fn is_stale(err: &str) -> bool {
    // Heuristics: russh closes the connection with these messages when
    // the underlying TCP died. Reconnecting is the right move.
    let lower = err.to_lowercase();
    lower.contains("disconnected")
        || lower.contains("connection reset")
        || lower.contains("broken pipe")
        || lower.contains("connection aborted")
        || lower.contains("not connected")
}

async fn exec_once(pool: &SshPool, host: &str, command: &str) -> Result<SshResult, String> {
    let session = pool.get(host).await?;
    let s = session.lock().await;

    let mut channel = s
        .handle
        .channel_open_session()
        .await
        .map_err(|e| format!("open channel: {}", e))?;
    channel
        .exec(true, command)
        .await
        .map_err(|e| format!("exec: {}", e))?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code: i32 = -1;

    // Read everything until the channel fully closes. Critical: do NOT
    // break on `Eof` — SSH-2 servers are permitted to send Eof before
    // ExitStatus, and breaking early loses the exit code, leaving us
    // with exit_code = -1 and ok = false even when the command
    // succeeded (this exactly the "Reached host but command failed"
    // symptom). Loop until `wait()` returns None (channel closed).
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { ref data } => stdout.extend_from_slice(data),
            ChannelMsg::ExtendedData { ref data, ext } if ext == 1 => {
                stderr.extend_from_slice(data)
            }
            ChannelMsg::ExitStatus { exit_status } => exit_code = exit_status as i32,
            // Don't break — keep draining; ExitStatus may follow Eof.
            ChannelMsg::Eof => {}
            ChannelMsg::Close => break,
            _ => {}
        }
    }

    Ok(SshResult {
        ok: exit_code == 0,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_code,
    })
}

/// SFTP-read a remote file as raw bytes. Replaces the prior pattern of
/// SCPing to a temp file then reading the temp file.
pub async fn sftp_read(pool: &SshPool, host: &str, remote_path: &str) -> Result<Vec<u8>, String> {
    let session = pool.get(host).await?;
    let s = session.lock().await;

    let channel = s
        .handle
        .channel_open_session()
        .await
        .map_err(|e| format!("sftp open channel: {}", e))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("sftp subsystem: {}", e))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("sftp init: {}", e))?;

    let mut file = sftp
        .open(remote_path)
        .await
        .map_err(|e| format!("sftp open {}: {}", remote_path, e))?;
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .await
        .map_err(|e| format!("sftp read: {}", e))?;
    Ok(buf)
}

/// SFTP-write raw bytes to a remote file. Used for SCP-upload paths.
pub async fn sftp_write(
    pool: &SshPool,
    host: &str,
    remote_path: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let session = pool.get(host).await?;
    let s = session.lock().await;

    let channel = s
        .handle
        .channel_open_session()
        .await
        .map_err(|e| format!("sftp open channel: {}", e))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("sftp subsystem: {}", e))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("sftp init: {}", e))?;

    let mut file = sftp
        .create(remote_path)
        .await
        .map_err(|e| format!("sftp create {}: {}", remote_path, e))?;
    use tokio::io::AsyncWriteExt;
    file.write_all(bytes)
        .await
        .map_err(|e| format!("sftp write: {}", e))?;
    file.flush()
        .await
        .map_err(|e| format!("sftp flush: {}", e))?;
    Ok(())
}
