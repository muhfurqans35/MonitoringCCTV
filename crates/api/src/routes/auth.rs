//! Login & sesi.
//!
//!   POST /api/auth/login        (publik)  {username, password} -> cookie sesi HttpOnly
//!   POST /api/auth/logout                 cabut sesi ini
//!   POST /api/auth/logout-all             cabut semua sesi akun ini (semua perangkat)
//!   GET  /api/auth/me                     {username, role, expires_at}
//!   GET  /api/auth/sessions               perangkat yang sedang login
//!   POST /api/auth/password               {current, new} -> sesi lain dicabut
//!   GET  /api/auth/check        (owner)   204 — dipakai nginx auth_request untuk /live/ (go2rtc)

use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::Instant;

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use crate::authcore;
use crate::security::{self, ApiMsg, Session};
use crate::state::AppState;

const BCRYPT_COST: u32 = 12;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub username: String,
    pub role: String,
    pub expires_at: i64,
}

/// Hash pembanding saat username tidak ada, agar waktu respons sama (tidak membocorkan
/// username mana yang terdaftar).
pub fn dummy_hash() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| bcrypt::hash(security::new_token(), BCRYPT_COST).unwrap_or_default())
}

/// `hash` None = akun tidak ada: tetap jalankan bcrypt terhadap hash palsu (waktu sama).
async fn verify_password(password: String, hash: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || match hash {
        Some(h) => bcrypt::verify(password, &h).unwrap_or(false),
        None => {
            let _ = bcrypt::verify(password, dummy_hash());
            false
        }
    })
    .await
    .unwrap_or(false)
}

fn minutes(d: std::time::Duration) -> u64 {
    d.as_secs().div_ceil(60).max(1)
}

fn user_agent(headers: &HeaderMap) -> String {
    headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

/// POST /api/auth/login
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(p): Json<LoginRequest>,
) -> Response {
    let ip = security::client_ip(&headers, Some(peer));
    let username = p.username.trim().to_string();
    if !security::request_same_origin(&headers) {
        return ApiMsg(StatusCode::FORBIDDEN, "asal permintaan tidak sah".into()).into_response();
    }
    if username.is_empty() || username.len() > 64 || p.password.is_empty() || p.password.len() > 256 {
        return ApiMsg(StatusCode::UNAUTHORIZED, "username atau password salah".into()).into_response();
    }
    let blocked = state.limiter.0.lock().ok().and_then(|l| l.blocked(&ip, &username, Instant::now()));
    if let Some(left) = blocked {
        security::audit(&state.db, &username, &ip, "login_blocked", "").await;
        let mut r = ApiMsg(
            StatusCode::TOO_MANY_REQUESTS,
            format!("Terlalu banyak percobaan gagal. Coba lagi dalam {} menit.", minutes(left)),
        )
        .into_response();
        if let Ok(v) = left.as_secs().to_string().parse() {
            r.headers_mut().insert(header::RETRY_AFTER, v);
        }
        return r;
    }

    let row: Option<(String, String)> = sqlx::query_as("SELECT password_hash, role FROM users WHERE username = ?")
        .bind(&username)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    let (hash, role) = match row {
        Some(r) => (Some(r.0), Some(r.1)),
        None => (None, None),
    };
    let ok = verify_password(p.password, hash).await && role.is_some();

    if !ok {
        let lock = state.limiter.0.lock().ok().and_then(|mut l| l.record_failure(&ip, &username, Instant::now()));
        let detail = lock.map(|d| format!("diblokir {} menit", minutes(d))).unwrap_or_default();
        security::audit(&state.db, &username, &ip, "login_fail", &detail).await;
        let msg = match lock {
            Some(d) => format!("Terlalu banyak percobaan gagal. Coba lagi dalam {} menit.", minutes(d)),
            None => "username atau password salah".into(),
        };
        let code = if lock.is_some() { StatusCode::TOO_MANY_REQUESTS } else { StatusCode::UNAUTHORIZED };
        return ApiMsg(code, msg).into_response();
    }
    let role = role.unwrap_or_default();
    if let Ok(mut l) = state.limiter.0.lock() {
        l.record_success(&ip, &username);
    }
    let token = match security::create_session(&state.db, &state.session_cfg, &username, &role, &ip, &user_agent(&headers)).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("gagal membuat sesi: {e}");
            return ApiMsg(StatusCode::INTERNAL_SERVER_ERROR, "gagal membuat sesi".into()).into_response();
        }
    };
    security::audit(&state.db, &username, &ip, "login_ok", "").await;
    let mut r = Json(MeResponse {
        username,
        role,
        expires_at: chrono::Utc::now().timestamp() + state.session_cfg.max_secs,
    })
    .into_response();
    r.headers_mut().insert(header::SET_COOKIE, security::session_cookie(&state.session_cfg, &headers, &token));
    r.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    r
}

/// GET /api/auth/me
pub async fn me(Extension(s): Extension<Session>) -> Json<MeResponse> {
    Json(MeResponse { username: s.username, role: s.role, expires_at: s.expires_at })
}

fn cleared(status: StatusCode) -> Response {
    let mut r = status.into_response();
    r.headers_mut().insert(header::SET_COOKIE, security::clear_cookie());
    r
}

/// POST /api/auth/logout
pub async fn logout(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(s): Extension<Session>,
) -> Response {
    security::revoke(&state.db, &s.id_hash).await;
    security::audit(&state.db, &s.username, &security::client_ip(&headers, Some(peer)), "logout", "").await;
    cleared(StatusCode::NO_CONTENT)
}

/// POST /api/auth/logout-all
pub async fn logout_all(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(s): Extension<Session>,
) -> Response {
    let n = security::revoke_all(&state.db, &s.username, None).await;
    security::audit(&state.db, &s.username, &security::client_ip(&headers, Some(peer)), "logout_all", &format!("{n} sesi")).await;
    cleared(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub struct SessionItem {
    pub created_at: i64,
    pub last_seen: i64,
    pub ip: String,
    pub user_agent: String,
    pub current: bool,
}

/// GET /api/auth/sessions
pub async fn sessions(State(state): State<AppState>, Extension(s): Extension<Session>) -> Json<Vec<SessionItem>> {
    let rows: Vec<(String, i64, i64, String, String)> = sqlx::query_as(
        "SELECT id_hash, created_at, last_seen, ip, user_agent FROM sessions WHERE username = ? ORDER BY last_seen DESC LIMIT 50",
    )
    .bind(&s.username)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    Json(
        rows.into_iter()
            .map(|(h, created_at, last_seen, ip, user_agent)| SessionItem { current: h == s.id_hash, created_at, last_seen, ip, user_agent })
            .collect(),
    )
}

#[derive(Debug, Deserialize)]
pub struct PasswordRequest {
    pub current: String,
    pub new: String,
}

/// POST /api/auth/password — ganti password; semua sesi lain dicabut.
pub async fn change_password(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(s): Extension<Session>,
    Json(p): Json<PasswordRequest>,
) -> Response {
    let ip = security::client_ip(&headers, Some(peer));
    if let Some(left) = state.limiter.0.lock().ok().and_then(|l| l.blocked(&ip, &s.username, Instant::now())) {
        return ApiMsg(StatusCode::TOO_MANY_REQUESTS, format!("Terlalu banyak percobaan. Coba lagi dalam {} menit.", minutes(left)))
            .into_response();
    }
    let hash: Option<(String,)> = sqlx::query_as("SELECT password_hash FROM users WHERE username = ?")
        .bind(&s.username)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    let Some((hash,)) = hash else {
        return ApiMsg(StatusCode::UNAUTHORIZED, "akun tidak ditemukan".into()).into_response();
    };
    if !verify_password(p.current, Some(hash)).await {
        if let Ok(mut l) = state.limiter.0.lock() {
            l.record_failure(&ip, &s.username, Instant::now());
        }
        security::audit(&state.db, &s.username, &ip, "password_fail", "").await;
        return ApiMsg(StatusCode::BAD_REQUEST, "password lama salah".into()).into_response();
    }
    if let Err(msg) = authcore::check_password_policy(&s.username, &p.new) {
        return ApiMsg(StatusCode::BAD_REQUEST, msg.into()).into_response();
    }
    let new_hash = match tokio::task::spawn_blocking(move || bcrypt::hash(p.new, BCRYPT_COST)).await {
        Ok(Ok(h)) => h,
        _ => return ApiMsg(StatusCode::INTERNAL_SERVER_ERROR, "gagal menyimpan password".into()).into_response(),
    };
    if sqlx::query("UPDATE users SET password_hash = ? WHERE username = ?")
        .bind(&new_hash)
        .bind(&s.username)
        .execute(&state.db)
        .await
        .is_err()
    {
        return ApiMsg(StatusCode::INTERNAL_SERVER_ERROR, "gagal menyimpan password".into()).into_response();
    }
    let n = security::revoke_all(&state.db, &s.username, Some(&s.id_hash)).await;
    security::audit(&state.db, &s.username, &ip, "password_changed", &format!("{n} sesi lain dicabut")).await;
    StatusCode::NO_CONTENT.into_response()
}

/// GET /api/auth/check — 204 bila sesi pemilik valid (middleware sudah memeriksa).
pub async fn check() -> StatusCode {
    StatusCode::NO_CONTENT
}
