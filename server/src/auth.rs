//! Authentication: password hashing, device-bound JWT, sessions, login,
//! and the `AuthUser` request extractor (per-request verification).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::Json;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use opsctl_core::api::{Claims, LoginRequest, LoginResponse};
use opsctl_core::model::{Role, UserView, MAX_LOGIN_TTL_SECS};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::{now_secs, AppState};
use crate::store::SessionRow;

pub fn hash_password(pw: &str) -> anyhow::Result<String> {
    // 16 random bytes from a v4 UUID (getrandom-backed) as the salt.
    let salt = SaltString::encode_b64(Uuid::new_v4().as_bytes())
        .map_err(|e| anyhow::anyhow!("salt: {e}"))?;
    let hash = Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hash: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(hash: &str, pw: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(pw.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

fn parse_role(s: &str) -> Role {
    match s {
        "admin" => Role::Admin,
        "operator" => Role::Operator,
        _ => Role::Viewer,
    }
}

/// Best-effort client IP: first hop of `x-forwarded-for` (trustworthy only
/// behind a reverse proxy), else the TCP peer address, else empty. Never
/// rejects, so handlers stay usable when serve() lacks ConnectInfo.
#[derive(Debug, Clone)]
pub struct ClientIp(pub String);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _st: &S) -> Result<Self, Self::Rejection> {
        let ip = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                parts
                    .extensions
                    .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                    .map(|ci| ci.0.ip().to_string())
            })
            .unwrap_or_default();
        Ok(ClientIp(ip))
    }
}

/// Issue a device-bound JWT and register the session (one per user+device).
async fn issue_token(
    st: &AppState,
    user_id: &str,
    role: Role,
    device_id: &str,
    ttl_secs: i64,
    ip: &str,
) -> anyhow::Result<(String, i64)> {
    let now = now_secs();
    let ttl = ttl_secs.clamp(60, MAX_LOGIN_TTL_SECS);
    let exp = now + ttl;
    let sid = Uuid::new_v4().to_string();

    st.store
        .upsert_session(&SessionRow {
            sid: sid.clone(),
            user_id: user_id.to_string(),
            device_id: device_id.to_string(),
            created_at: now,
            last_seen: now,
            ip: ip.to_string(),
            revoked: 0,
        })
        .await?;

    let claims = Claims {
        sub: user_id.to_string(),
        did: device_id.to_string(),
        sid,
        role,
        iat: now,
        exp,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(st.jwt_secret.as_bytes()),
    )?;
    Ok((token, exp))
}

/// Issue token+session, audit, notify, and build the login response.
async fn complete_login(
    st: &AppState,
    user: &crate::store::UserRow,
    device_id: &str,
    ip: &str,
) -> Result<LoginResponse, AppError> {
    let role = parse_role(&user.role);
    let ttl = if user.login_ttl_secs > 0 { user.login_ttl_secs } else { st.default_ttl_secs };
    let (token, exp) = issue_token(st, &user.id, role, device_id, ttl, ip)
        .await
        .map_err(AppError::Internal)?;

    let _ = st.store.insert_audit(
        &Uuid::new_v4().to_string(), now_secs(), &user.id, &user.email,
        "login", device_id, "", "ok", "",
    ).await;
    let _ = st.store.push_notification(
        &user.id, "login", "新设备登录",
        &format!("设备 {device_id} 登录成功"), "/settings", now_secs(),
    ).await;

    Ok(LoginResponse {
        token,
        user: UserView {
            id: user.id.clone(),
            name: user.name.clone(),
            email: user.email.clone(),
            role,
            telegram_bound: user.telegram_chat_id.is_some(),
            login_ttl_secs: ttl,
        },
        expires_at: exp,
    })
}

/// `POST /login`. When OTP is enabled (settings flag), returns a pending step
/// instead of a token; otherwise completes the login directly.
pub async fn login(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<LoginRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.device_id.trim().is_empty() {
        return Err(AppError::BadRequest("device_id required".into()));
    }
    let user = st.store.get_user_by_name(&req.username).await.map_err(AppError::Internal)?
        .ok_or(AppError::Unauthorized)?;
    if !verify_password(&user.pass_hash, &req.password) {
        return Err(AppError::Unauthorized);
    }

    // Per-user TOTP: if the user enrolled 2FA, require the second step.
    if !user.totp_secret.is_empty() {
        let pending_id = Uuid::new_v4().to_string();
        let _ = st.store.set_setting(
            &format!("otp_pending:{pending_id}"),
            &format!("{}|{}", user.id, req.device_id),
        ).await;
        return Ok(Json(serde_json::json!({ "need_otp": true, "pending_id": pending_id })));
    }

    let resp = complete_login(&st, &user, &req.device_id, &ip).await?;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}

#[derive(serde::Deserialize)]
pub struct OtpRequest {
    pub pending_id: String,
    pub code: String,
}

/// `POST /login/otp` — second step: verify the RFC6238 TOTP code against the
/// user's vault-encrypted secret.
pub async fn login_otp(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<OtpRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = format!("otp_pending:{}", req.pending_id);
    let pending = st.store.get_setting(&key).await.map_err(AppError::Internal)?
        .filter(|s| !s.is_empty())
        .ok_or(AppError::Unauthorized)?;
    let (uid, device) = pending.split_once('|').ok_or(AppError::Unauthorized)?;
    let user = st.store.get_user_by_id(uid).await.map_err(AppError::Internal)?
        .ok_or(AppError::Unauthorized)?;
    let secret = st.vault.decrypt(&user.totp_secret).map_err(|_| AppError::Sealed)?;
    if !crate::totp::verify(&secret, &req.code, now_secs()) {
        return Err(AppError::BadRequest("口令错误".into()));
    }
    let resp = complete_login(&st, &user, device, &ip).await?;
    let _ = st.store.set_setting(&key, "").await;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}

#[derive(serde::Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub email: String,
}

/// `POST /register` — self-registration, only when the admin has opened it.
pub async fn register(
    State(st): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let open = st.store.get_setting("register_open").await.ok().flatten().as_deref() == Some("1");
    if !open {
        return Err(AppError::Forbidden);
    }
    if req.username.trim().is_empty() || req.password.len() < 6 {
        return Err(AppError::BadRequest("用户名必填,密码至少 6 位".into()));
    }
    if st.store.get_user_by_name(&req.username).await.map_err(AppError::Internal)?.is_some() {
        return Err(AppError::BadRequest("用户名已存在".into()));
    }
    let hash = hash_password(&req.password).map_err(AppError::Internal)?;
    st.store.create_user(
        &Uuid::new_v4().to_string(), req.username.trim(),
        &req.email, "viewer", &hash, st.default_ttl_secs,
    ).await.map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Authenticated principal, produced by verifying the JWT on each request.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: String,
    pub email: String,
    pub role: Role,
    pub sid: String,
    pub device_id: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        st: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Bearer token
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?
            .to_string();

        // Client must present its machine code so we can enforce did binding.
        let device_hdr = parts
            .headers
            .get("x-device-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let data = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(st.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| AppError::Unauthorized)?;
        let claims = data.claims;

        // Device binding: token's did must match the machine code presented now.
        if claims.did != device_hdr || device_hdr.is_empty() {
            return Err(AppError::Unauthorized);
        }

        // Session must still be the current, non-revoked one for this device.
        let sess = st
            .store
            .get_session(&claims.sid)
            .await
            .map_err(AppError::Internal)?
            .ok_or(AppError::Unauthorized)?;
        if sess.revoked != 0 || sess.user_id != claims.sub || sess.device_id != claims.did {
            return Err(AppError::Unauthorized);
        }

        // Activity tracking: refresh last_seen, throttled to once per second.
        let now = now_secs();
        if now - sess.last_seen >= 1 {
            let _ = st.store.touch_session(&claims.sid, now).await;
        }

        // Resolve email for audit attribution (best-effort).
        let email = st
            .store
            .get_user_by_id(&claims.sub)
            .await
            .ok()
            .flatten()
            .map(|u| u.email)
            .unwrap_or_default();

        Ok(AuthUser {
            user_id: claims.sub,
            email,
            role: claims.role,
            sid: claims.sid,
            device_id: claims.did,
        })
    }
}
