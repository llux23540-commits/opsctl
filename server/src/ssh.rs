//! Server-side SSH executor (russh). Password auth (`ssh_pw` accounts) and
//! public-key auth (`ssh_key` accounts, private key in the vault-decrypted
//! secret). Non-interactive single command.

use std::sync::Arc;

use anyhow::anyhow;
use russh::client::{self, Config, Handler};
use russh::keys::ssh_key::PublicKey;
use russh::keys::{decode_secret_key, PrivateKeyWithHashAlg};
use russh::ChannelMsg;

/// Accept any host key (M1). TODO: pin/verify known_hosts.
struct AcceptAll;

impl Handler for AcceptAll {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _key: &PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

pub struct SshOutcome {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

async fn connect(host: &str, port: u16) -> anyhow::Result<client::Handle<AcceptAll>> {
    let config = Arc::new(Config::default());
    Ok(client::connect(config, (host, port), AcceptAll).await?)
}

/// Run one command over an authenticated handle, collecting output.
async fn exec(mut handle: client::Handle<AcceptAll>, command: &str) -> anyhow::Result<SshOutcome> {
    let mut channel = handle.channel_open_session().await?;
    channel.exec(true, command).await?;

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut exit_code: Option<i32> = None;

    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => exit_code = Some(exit_status as i32),
            ChannelMsg::Eof | ChannelMsg::Close => {}
            _ => {}
        }
    }

    Ok(SshOutcome {
        exit_code,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Password auth (`ssh_pw`).
pub async fn run_command(
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    command: &str,
) -> anyhow::Result<SshOutcome> {
    let mut handle = connect(host, port).await?;
    let auth = handle.authenticate_password(user, password).await?;
    if !auth.success() {
        anyhow::bail!("ssh 密码认证失败 {user}@{host}");
    }
    exec(handle, command).await
}

/// Public-key auth (`ssh_key`). `private_key` is an OpenSSH/PEM private key
/// (optionally encrypted with `passphrase`).
pub async fn run_command_key(
    host: &str,
    port: u16,
    user: &str,
    private_key: &str,
    passphrase: Option<&str>,
    command: &str,
) -> anyhow::Result<SshOutcome> {
    let key = decode_secret_key(private_key, passphrase)
        .map_err(|e| anyhow!("私钥解析失败:{e}"))?;
    let mut handle = connect(host, port).await?;
    let auth = handle
        .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), None))
        .await?;
    if !auth.success() {
        anyhow::bail!("ssh 公钥认证失败 {user}@{host}");
    }
    exec(handle, command).await
}
