//! Sesi login di server + cookie HttpOnly.
//!
//! * Token sesi = 2 x UUIDv4 acak (±244 bit) di cookie `sk_session` (HttpOnly, SameSite=Strict,
//!   Secure bila HTTPS). Database hanya menyimpan SHA-256 token, bukan token aslinya.
//! * Sesi berakhir bila tidak dipakai `SESSION_IDLE_HOURS` (default 72 jam) atau setelah
//!   `SESSION_MAX_DAYS` (default 14 hari), dan bisa dicabut (logout / keluar dari semua perangkat /
//!   ganti password).
//! * IP klien: header X-Real-IP hanya dipercaya bila koneksi datang dari proxy lokal (127.0.0.1).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::authcore::{self, LoginLimiter, SESSION_COOKIE};

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub idle_secs: i64,
    pub max_secs: i64,
    /// None = otomatis (Secure bila permintaan datang lewat HTTPS menurut X-Forwarded-Proto).
    pub cookie_secure: Option<bool>,
}

impl SessionConfig {
    pub fn from_env() -> Self {
        let num = |k: &str, d: i64| std::env::var(k).ok().and_then(|v| v.trim().parse::<i64>().ok()).filter(|v| *v > 0).unwrap_or(d);
        let cookie_secure = match std::env::var("COOKIE_SECURE").unwrap_or_default().trim().to_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        };
        SessionConfig {
            idle_secs: num("SESSION_IDLE_HOURS", 72) * 3600,
            max_secs: num("SESSION_MAX_DAYS", 14) * 86400,
            cookie_secure,
        }
    }
}

#[derive(Clone, Default)]
pub struct Limiter(pub Arc<Mutex<LoginLimiter>>);

#[derive(Debug, Clone)]
pub struct Session {
    pub id_hash: String,
    pub username: String,
    pub role: String,
    pub expires_at: i64,
}

/// Balasan JSON {"error": pesan} dengan status tertentu (pesan tampil di dashboard).
pub struct ApiMsg(pub StatusCode, pub String);

impl IntoResponse for ApiMsg {
    fn into_response(self) -> Response {
        let mut r = (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response();
        r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        r
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn new_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// IP klien. Di belakang nginx (api hanya mendengar 127.0.0.1) nginx mengisi X-Real-IP;
/// nilai itu diabaikan bila koneksi tidak datang dari loopback (mencegah pemalsuan).
pub fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>) -> String {
    let from_proxy = peer.map(|p| p.ip().is_loopback() || trusted_proxies().contains(&p.ip())).unwrap_or(false);
    if from_proxy {
        if let Some(ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()).map(str::trim).filter(|s| !s.is_empty()) {
            return ip.chars().take(64).collect();
        }
    }
    peer.map(|p| p.ip().to_string()).unwrap_or_else(|| "?".into())
}

/// Proxy tambahan yang boleh mengisi X-Real-IP (Docker: IP container caddy), env
/// `TRUSTED_PROXIES=172.18.0.5,172.18.0.6`. Loopback selalu dipercaya.
fn trusted_proxies() -> &'static Vec<std::net::IpAddr> {
    static T: std::sync::OnceLock<Vec<std::net::IpAddr>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        std::env::var("TRUSTED_PROXIES")
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    })
}

fn header_str<'a>(headers: &'a HeaderMap, name: header::HeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Permintaan berasal dari halaman situs ini? (Origin, atau Referer bila Origin tidak ada.)
pub fn request_same_origin(headers: &HeaderMap) -> bool {
    let origin = header_str(headers, header::ORIGIN).or_else(|| header_str(headers, header::REFERER));
    authcore::same_origin(origin, header_str(headers, header::HOST))
}

fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(',').next().unwrap_or("").trim().eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

pub fn session_cookie(cfg: &SessionConfig, headers: &HeaderMap, token: &str) -> HeaderValue {
    let secure = cfg.cookie_secure.unwrap_or_else(|| is_https(headers));
    let v = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
        cfg.max_secs,
        if secure { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&v).unwrap_or_else(|_| HeaderValue::from_static("sk_session=; Path=/; Max-Age=0"))
}

pub fn clear_cookie() -> HeaderValue {
    HeaderValue::from_static("sk_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| authcore::cookie_value(c, SESSION_COOKIE).map(str::to_string))
        .filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub async fn create_session(
    db: &SqlitePool,
    cfg: &SessionConfig,
    username: &str,
    role: &str,
    ip: &str,
    user_agent: &str,
) -> anyhow::Result<String> {
    let token = new_token();
    let t = now();
    sqlx::query(
        "INSERT INTO sessions (id_hash, username, role, created_at, last_seen, expires_at, ip, user_agent) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(hash_token(&token))
    .bind(username)
    .bind(role)
    .bind(t)
    .bind(t)
    .bind(t + cfg.max_secs)
    .bind(ip)
    .bind(user_agent.chars().take(200).collect::<String>())
    .execute(db)
    .await?;
    Ok(token)
}

/// Sesi aktif untuk token cookie, atau None (tidak ada / kedaluwarsa / dicabut).
pub async fn lookup(db: &SqlitePool, cfg: &SessionConfig, token: &str) -> Option<Session> {
    let id_hash = hash_token(token);
    let row: Option<(String, String, i64, i64)> =
        sqlx::query_as("SELECT username, role, last_seen, expires_at FROM sessions WHERE id_hash = ?")
            .bind(&id_hash)
            .fetch_optional(db)
            .await
            .ok()?;
    let (username, role, last_seen, expires_at) = row?;
    let t = now();
    if t >= expires_at || t - last_seen > cfg.idle_secs {
        let _ = sqlx::query("DELETE FROM sessions WHERE id_hash = ?").bind(&id_hash).execute(db).await;
        return None;
    }
    // Pengguna harus masih ada dengan peran yang sama (user dihapus / diturunkan = sesi mati).
    let current_role: Option<(String,)> = sqlx::query_as("SELECT role FROM users WHERE username = ?")
        .bind(&username)
        .fetch_optional(db)
        .await
        .ok()?;
    if current_role.map(|(r,)| r != role).unwrap_or(true) {
        let _ = sqlx::query("DELETE FROM sessions WHERE id_hash = ?").bind(&id_hash).execute(db).await;
        return None;
    }
    if t - last_seen > 300 {
        let _ = sqlx::query("UPDATE sessions SET last_seen = ? WHERE id_hash = ?").bind(t).bind(&id_hash).execute(db).await;
    }
    Some(Session { id_hash, username, role, expires_at })
}

pub async fn session_from_headers(db: &SqlitePool, cfg: &SessionConfig, headers: &HeaderMap) -> Option<Session> {
    let token = token_from_headers(headers)?;
    lookup(db, cfg, &token).await
}

pub async fn revoke(db: &SqlitePool, id_hash: &str) {
    let _ = sqlx::query("DELETE FROM sessions WHERE id_hash = ?").bind(id_hash).execute(db).await;
}

/// Cabut semua sesi pengguna, kecuali `keep` (id_hash sesi saat ini) bila diberikan.
pub async fn revoke_all(db: &SqlitePool, username: &str, keep: Option<&str>) -> u64 {
    sqlx::query("DELETE FROM sessions WHERE username = ? AND id_hash <> ?")
        .bind(username)
        .bind(keep.unwrap_or(""))
        .execute(db)
        .await
        .map(|r| r.rows_affected())
        .unwrap_or(0)
}

pub async fn audit(db: &SqlitePool, username: &str, ip: &str, event: &str, detail: &str) {
    tracing::info!(target: "audit", "{event} user={username} ip={ip} {detail}");
    let _ = sqlx::query("INSERT INTO auth_events (ts, username, ip, event, detail) VALUES (?, ?, ?, ?, ?)")
        .bind(now())
        .bind(username.chars().take(64).collect::<String>())
        .bind(ip)
        .bind(event)
        .bind(detail)
        .execute(db)
        .await;
}

/// Bersih-bersih berkala: sesi kedaluwarsa/menganggur, catatan audit > 180 hari, pembatas login.
pub async fn housekeeping(db: SqlitePool, cfg: SessionConfig, limiter: Limiter) {
    loop {
        let t = now();
        let _ = sqlx::query("DELETE FROM sessions WHERE expires_at <= ? OR last_seen < ?")
            .bind(t)
            .bind(t - cfg.idle_secs)
            .execute(&db)
            .await;
        let _ = sqlx::query("DELETE FROM auth_events WHERE ts < ?").bind(t - 180 * 86400).execute(&db).await;
        if let Ok(mut l) = limiter.0.lock() {
            l.prune(std::time::Instant::now());
        }
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_and_hash_shape() {
        let t = new_token();
        assert_eq!(t.len(), 64);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(t, new_token());
        assert_eq!(hash_token("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn real_ip_only_trusted_from_loopback() {
        let mut h = HeaderMap::new();
        h.insert("x-real-ip", HeaderValue::from_static("203.0.113.9"));
        assert_eq!(client_ip(&h, Some("127.0.0.1:5000".parse().unwrap())), "203.0.113.9");
        assert_eq!(client_ip(&h, Some("192.168.1.50:5000".parse().unwrap())), "192.168.1.50");
    }

    #[test]
    fn cookie_flags() {
        let cfg = SessionConfig { idle_secs: 10, max_secs: 100, cookie_secure: None };
        let mut h = HeaderMap::new();
        let v = session_cookie(&cfg, &h, "ab");
        assert!(v.to_str().unwrap().contains("HttpOnly; SameSite=Strict; Max-Age=100"));
        assert!(!v.to_str().unwrap().contains("Secure"));
        h.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert!(session_cookie(&cfg, &h, "ab").to_str().unwrap().ends_with("; Secure"));
    }
}
