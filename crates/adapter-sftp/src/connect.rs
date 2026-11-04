//! Opening an SFTP session: TCP, SSH handshake with host-key trust, sign-in,
//! then the `sftp` subsystem.
//!
//! Host keys are trusted when `~/.ssh/known_hosts` or the app's own file
//! knows them. A new key needs the user's explicit OK (`HostTrust::Accept`
//! with that fingerprint), which is then remembered in the app's file. A key
//! that differs from a remembered one is always refused.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use application::ports::{ConnectError, Secret};
use domain::{HostKey, HostTrust, ServerAccount, ServerAuth, VfsError};
use russh::client::{self, AuthResult, Handle};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate, known_hosts};
use russh_sftp::client::SftpSession;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(600);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const SFTP_SUBSYSTEM: &str = "sftp";
/// OpenSSH drops excess unauthenticated connections (`MaxStartups`), so a
/// failed handshake is retried a few times before giving up.
const HANDSHAKE_ATTEMPTS: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(400);
const SSH_FOLDER: &str = ".ssh";
const USER_KNOWN_HOSTS: &str = "known_hosts";
const HOME_VARIABLE: &str = "HOME";
const HOME_PREFIX: &str = "~/";
/// RSA private keys aren't signed with locally (see ADR 0021); the agent can.
const KEY_FILE_PROBLEM: &str =
    "the key file couldn't be used. RSA keys work through the SSH agent: run ssh-add and sign in with the agent";

/// What the handshake decided about the server's key.
#[derive(Debug, Clone)]
enum Verdict {
    Trusted,
    Unknown(HostKey),
    Changed(HostKey),
}

pub(crate) struct TrustingHandler {
    host: String,
    port: u16,
    trust: HostTrust,
    app_known_hosts: PathBuf,
    verdict: Arc<Mutex<Option<Verdict>>>,
}

fn host_key(key: &PublicKey) -> HostKey {
    HostKey {
        algorithm: key.algorithm().as_str().to_owned(),
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
    }
}

impl TrustingHandler {
    fn judge(&self, key: &PublicKey) -> Verdict {
        let presented = host_key(key);
        let mut files = vec![self.app_known_hosts.clone()];
        files.extend(user_known_hosts());
        let mut known = false;
        for file in files.iter().filter(|file| file.exists()) {
            match known_hosts::check_known_hosts_path(&self.host, self.port, key, file) {
                Ok(true) => known = true,
                Ok(false) => {}
                Err(_) => return Verdict::Changed(presented),
            }
        }
        if known {
            return Verdict::Trusted;
        }
        match &self.trust {
            HostTrust::Accept(accepted) if *accepted == presented => {
                match known_hosts::learn_known_hosts_path(&self.host, self.port, key, &self.app_known_hosts) {
                    Ok(()) => Verdict::Trusted,
                    // Trusted for this session even if it can't be remembered.
                    Err(_) => Verdict::Trusted,
                }
            }
            _ => Verdict::Unknown(presented),
        }
    }
}

fn user_known_hosts() -> Option<PathBuf> {
    std::env::var_os(HOME_VARIABLE).map(|home| PathBuf::from(home).join(SSH_FOLDER).join(USER_KNOWN_HOSTS))
}

impl client::Handler for TrustingHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server_key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = server_key else {
            // Host certificates need a CA setup we don't offer yet.
            return Ok(false);
        };
        let verdict = self.judge(key);
        let trusted = matches!(verdict, Verdict::Trusted);
        *self.verdict.lock().unwrap_or_else(PoisonError::into_inner) = Some(verdict);
        Ok(trusted)
    }
}

/// A signed-in SSH connection and its SFTP session.
pub(crate) struct Session {
    pub sftp: SftpSession,
    /// Kept so the connection stays open as long as the session is used.
    pub _ssh: Handle<TrustingHandler>,
}

pub(crate) async fn open(
    account: &ServerAccount,
    secret: Option<Secret>,
    trust: HostTrust,
    app_known_hosts: &Path,
) -> Result<Session, ConnectError> {
    let mut attempt = 1;
    loop {
        match open_once(account, secret.clone(), trust.clone(), app_known_hosts).await {
            Err(ConnectError::Failed(VfsError::Transient { .. })) if attempt < HANDSHAKE_ATTEMPTS => {
                tokio::time::sleep(RETRY_DELAY * attempt).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}

async fn open_once(
    account: &ServerAccount,
    secret: Option<Secret>,
    trust: HostTrust,
    app_known_hosts: &Path,
) -> Result<Session, ConnectError> {
    let port = account.port_or_default();
    let verdict = Arc::new(Mutex::new(None));
    let handler = TrustingHandler {
        host: account.host.clone(),
        port,
        trust,
        app_known_hosts: app_known_hosts.to_path_buf(),
        verdict: Arc::clone(&verdict),
    };
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(INACTIVITY_TIMEOUT),
        keepalive_interval: Some(KEEPALIVE_INTERVAL),
        ..client::Config::default()
    });
    let connecting = client::connect(config, (account.host.as_str(), port), handler);
    let handshake = tokio::time::timeout(CONNECT_TIMEOUT, connecting)
        .await
        .map_err(|_| ConnectError::Failed(VfsError::Transient { retry_after: None }))?;
    let mut ssh = match handshake {
        Ok(ssh) => ssh,
        Err(error) => {
            return Err(match verdict.lock().unwrap_or_else(PoisonError::into_inner).take() {
                Some(Verdict::Unknown(key)) => ConnectError::UnknownHost(key),
                Some(Verdict::Changed(key)) => ConnectError::HostKeyChanged(key),
                _ => ConnectError::Failed(ssh_error(&error)),
            });
        }
    };
    sign_in(&mut ssh, account, secret).await?;
    let channel = ssh.channel_open_session().await.map_err(|e| ConnectError::Failed(ssh_error(&e)))?;
    channel.request_subsystem(true, SFTP_SUBSYSTEM).await.map_err(|e| ConnectError::Failed(ssh_error(&e)))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| ConnectError::Failed(crate::errors::from_sftp(&e)))?;
    Ok(Session { sftp, _ssh: ssh })
}

async fn sign_in(
    ssh: &mut Handle<TrustingHandler>,
    account: &ServerAccount,
    secret: Option<Secret>,
) -> Result<(), ConnectError> {
    let user = account.username.clone();
    let failed = |e: &russh::Error| ConnectError::Failed(ssh_error(e));
    let result = match &account.auth {
        ServerAuth::Password => {
            let password = secret.map(|s| s.expose().to_owned()).unwrap_or_default();
            ssh.authenticate_password(user, password).await.map_err(|e| failed(&e))?
        }
        ServerAuth::KeyFile { path } => {
            let passphrase = secret.as_ref().map(Secret::expose);
            let key = russh::keys::load_secret_key(expand_home(path), passphrase)
                .map_err(|e| ConnectError::Failed(VfsError::Io { message: format!("{KEY_FILE_PROBLEM} ({e})") }))?;
            let hash = ssh.best_supported_rsa_hash().await.map_err(|e| failed(&e))?.flatten();
            ssh.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(|e| failed(&e))?
        }
        ServerAuth::Agent => sign_in_with_agent(ssh, &user).await?,
        ServerAuth::Anonymous => ssh.authenticate_none(user).await.map_err(|e| failed(&e))?,
    };
    match result {
        AuthResult::Success => Ok(()),
        AuthResult::Failure { .. } => Err(ConnectError::Failed(VfsError::PermissionDenied)),
    }
}

/// Tries each key the running agent holds, in order.
async fn sign_in_with_agent(ssh: &mut Handle<TrustingHandler>, user: &str) -> Result<AuthResult, ConnectError> {
    let no_agent =
        |e: &dyn std::fmt::Display| ConnectError::Failed(VfsError::Io { message: format!("the SSH agent: {e}") });
    let mut agent = russh::keys::agent::client::AgentClient::connect_env().await.map_err(|e| no_agent(&e))?;
    let identities = agent.request_identities().await.map_err(|e| no_agent(&e))?;
    let hash = ssh.best_supported_rsa_hash().await.map_err(|e| ConnectError::Failed(ssh_error(&e)))?.flatten();
    for identity in identities {
        let key = identity.public_key().into_owned();
        let result = ssh.authenticate_publickey_with(user, key, hash, &mut agent).await.map_err(|e| no_agent(&e))?;
        if result.success() {
            return Ok(result);
        }
    }
    Err(ConnectError::Failed(VfsError::PermissionDenied))
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix(HOME_PREFIX), std::env::var_os(HOME_VARIABLE)) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

pub(crate) fn ssh_error(error: &russh::Error) -> VfsError {
    match error {
        russh::Error::IO(io) if io.kind() == std::io::ErrorKind::PermissionDenied => VfsError::PermissionDenied,
        russh::Error::IO(_) | russh::Error::ConnectionTimeout | russh::Error::Disconnect | russh::Error::HUP => {
            VfsError::Transient { retry_after: None }
        }
        other => VfsError::Io { message: other.to_string() },
    }
}
