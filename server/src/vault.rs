//! Credential vault: encrypt system-user secrets at rest with a key derived
//! from an unseal passphrase. The key lives only in memory; the server starts
//! sealed unless a passphrase is provided (env or `POST /api/vault/unseal`).

use std::sync::RwLock;

use anyhow::{anyhow, Result};
use argon2::Argon2;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use uuid::Uuid;

use crate::store::Store;

/// 12 random bytes (from a v4 UUID, getrandom-backed) for an AEAD nonce.
fn random_nonce() -> [u8; 12] {
    let mut n = [0u8; 12];
    n.copy_from_slice(&Uuid::new_v4().as_bytes()[..12]);
    n
}

const CIPHER_PREFIX: &str = "v1:";
const CHECK_PLAINTEXT: &str = "opsctl-vault-ok";

/// In-memory unseal state. `None` key = sealed.
#[derive(Default)]
pub struct Vault {
    key: RwLock<Option<[u8; 32]>>,
}

impl Vault {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_sealed(&self) -> bool {
        self.key.read().unwrap().is_none()
    }

    pub fn seal(&self) {
        *self.key.write().unwrap() = None;
    }

    /// Derive the key from `passphrase` (argon2 + per-deployment salt stored in
    /// settings). First unseal establishes the salt + a verification token;
    /// later unseals verify the passphrase against that token. Returns the
    /// derived key so callers can also run the plaintext→ciphertext migration.
    pub async fn unseal(&self, passphrase: &str, store: &Store) -> Result<()> {
        if passphrase.is_empty() {
            return Err(anyhow!("passphrase required"));
        }
        // salt (create on first unseal)
        let salt = match store.get_setting("vault_salt").await? {
            Some(s) if !s.is_empty() => B64.decode(s).map_err(|_| anyhow!("bad salt"))?,
            _ => {
                let s = *Uuid::new_v4().as_bytes(); // 16 random bytes
                store.set_setting("vault_salt", &B64.encode(s)).await?;
                s.to_vec()
            }
        };
        let key = derive_key(passphrase, &salt)?;

        // verify or establish the check token
        match store.get_setting("vault_check").await? {
            Some(c) if !c.is_empty() => {
                let plain = decrypt_with(&key, &c).map_err(|_| anyhow!("passphrase mismatch"))?;
                if plain != CHECK_PLAINTEXT {
                    return Err(anyhow!("passphrase mismatch"));
                }
            }
            _ => {
                let token = encrypt_with(&key, CHECK_PLAINTEXT)?;
                store.set_setting("vault_check", &token).await?;
            }
        }

        *self.key.write().unwrap() = Some(key);
        Ok(())
    }

    /// Encrypt plaintext → `v1:<base64(nonce||ciphertext)>`. Errors if sealed.
    pub fn encrypt(&self, plain: &str) -> Result<String> {
        let guard = self.key.read().unwrap();
        let key = guard.as_ref().ok_or_else(|| anyhow!("vault sealed"))?;
        encrypt_with(key, plain)
    }

    /// Deterministic encrypt: nonce = SHA256(key || plaintext)[..12], so the same
    /// plaintext always yields the same ciphertext. Used for git export so
    /// re-syncing unchanged content produces an identical file (no commit churn).
    /// A nonce only repeats for identical plaintext under the same key, which is
    /// safe for ChaCha20-Poly1305.
    pub fn encrypt_stable(&self, plain: &str) -> Result<String> {
        use sha2::{Digest, Sha256};
        let guard = self.key.read().unwrap();
        let key = guard.as_ref().ok_or_else(|| anyhow!("vault sealed"))?;
        let mut h = Sha256::new();
        h.update(key);
        h.update(plain.as_bytes());
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&h.finalize()[..12]);
        let cipher = ChaCha20Poly1305::new_from_slice(key).map_err(|_| anyhow!("bad key"))?;
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce), plain.as_bytes())
            .map_err(|_| anyhow!("encrypt failed"))?;
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ct);
        Ok(format!("{CIPHER_PREFIX}{}", B64.encode(blob)))
    }

    /// Decrypt a `v1:` value; non-prefixed input is treated as legacy plaintext
    /// and returned as-is (so pre-encryption rows keep working). Errors if a
    /// `v1:` value is present but the vault is sealed.
    pub fn decrypt(&self, value: &str) -> Result<String> {
        if !value.starts_with(CIPHER_PREFIX) {
            return Ok(value.to_string());
        }
        let guard = self.key.read().unwrap();
        let key = guard.as_ref().ok_or_else(|| anyhow!("vault sealed"))?;
        decrypt_with(key, value)
    }

    /// Encrypt any not-yet-encrypted, non-empty secrets in place (run after unseal).
    pub async fn migrate_plaintext(&self, store: &Store) -> Result<usize> {
        let rows = store.list_plaintext_secrets(CIPHER_PREFIX).await?;
        let mut n = 0;
        for (id, secret) in rows {
            let enc = self.encrypt(&secret)?;
            store.update_system_user_secret(&id, &enc).await?;
            n += 1;
        }
        Ok(n)
    }
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("kdf: {e}"))?;
    Ok(key)
}

fn encrypt_with(key: &[u8; 32], plain: &str) -> Result<String> {
    let cipher = ChaCha20Poly1305::new_from_slice(key).map_err(|_| anyhow!("bad key"))?;
    let nonce = random_nonce();
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plain.as_bytes())
        .map_err(|_| anyhow!("encrypt failed"))?;
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ct);
    Ok(format!("{CIPHER_PREFIX}{}", B64.encode(blob)))
}

fn decrypt_with(key: &[u8; 32], value: &str) -> Result<String> {
    let b64 = value.strip_prefix(CIPHER_PREFIX).ok_or_else(|| anyhow!("not a v1 value"))?;
    let blob = B64.decode(b64).map_err(|_| anyhow!("bad base64"))?;
    if blob.len() < 12 {
        return Err(anyhow!("truncated ciphertext"));
    }
    let (nonce, ct) = blob.split_at(12);
    let cipher = ChaCha20Poly1305::new_from_slice(key).map_err(|_| anyhow!("bad key"))?;
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| anyhow!("decrypt failed"))?;
    String::from_utf8(plain).map_err(|_| anyhow!("invalid utf8"))
}
