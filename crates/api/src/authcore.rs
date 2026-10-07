//! Logika keamanan murni (hanya std, diuji tanpa server): pembatas percobaan login, parser
//! cookie, cek asal permintaan (anti-CSRF / cross-site WebSocket), dan kebijakan password.

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const SESSION_COOKIE: &str = "sk_session";

/// Nilai cookie `name` dari header `Cookie` (bisa berisi banyak cookie dipisah `;`).
pub fn cookie_value<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    cookie_header.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k.trim() == name).then(|| v.trim()).filter(|v| !v.is_empty())
    })
}

/// Host (tanpa skema) dari header Origin/Referer, mis. "https://kost.example.com/x" -> "kost.example.com".
pub fn origin_host(origin: &str) -> Option<&str> {
    let rest = origin.split_once("://").map(|(_, r)| r)?;
    let host = rest.split('/').next()?;
    (!host.is_empty()).then_some(host)
}

/// Permintaan berasal dari halaman situs ini sendiri?
/// `origin`: header Origin (atau Referer); `host`: header Host yang diteruskan proxy.
/// Tanpa Origin (mis. curl, aplikasi native) dianggap sama-asal; cookie SameSite=Strict
/// sudah menghalangi browser situs lain mengirim cookie sesi.
pub fn same_origin(origin: Option<&str>, host: Option<&str>) -> bool {
    match (origin, host) {
        (None, _) => true,
        (Some("null"), _) => false,
        (Some(o), Some(h)) => origin_host(o).map(|oh| oh.eq_ignore_ascii_case(h)).unwrap_or(false),
        (Some(_), None) => false,
    }
}

/// Kebijakan password baru. Mengembalikan pesan kesalahan untuk pengguna bila ditolak.
pub fn check_password_policy(username: &str, password: &str) -> Result<(), &'static str> {
    let len = password.chars().count();
    if len < 10 {
        return Err("password minimal 10 karakter");
    }
    if password.len() > 72 {
        return Err("password maksimal 72 byte (batas bcrypt)");
    }
    let lower = password.to_lowercase();
    if lower.contains(&username.to_lowercase()) && !username.is_empty() {
        return Err("password tidak boleh memuat username");
    }
    const COMMON: [&str; 12] = [
        "1234567890", "password12", "qwertyuiop", "smartkost1", "admin12345", "0987654321",
        "iloveyou12", "password123", "1111111111", "kostkost12", "rahasia123", "bismillah1",
    ];
    if COMMON.iter().any(|c| lower == *c) {
        return Err("password terlalu umum");
    }
    let classes = [
        password.chars().any(|c| c.is_lowercase()),
        password.chars().any(|c| c.is_uppercase()),
        password.chars().any(|c| c.is_ascii_digit()),
        password.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if classes < 2 && len < 16 {
        return Err("gunakan campuran huruf dan angka/simbol (atau minimal 16 karakter)");
    }
    Ok(())
}

// ------------------------------------------------------------------ pembatas percobaan login

#[derive(Debug, Clone)]
struct Bucket {
    fails: u32,
    window_start: Instant,
    locked_until: Option<Instant>,
    lockouts: u32,
}

/// Pembatas percobaan login gagal (di memori):
///   * per (IP, username): 5 gagal dalam 15 menit -> kunci 15 menit, berlipat tiap kunci ulang (maks 24 jam)
///   * per IP            : 20 gagal dalam 15 menit -> kunci 1 jam (menebak banyak username)
/// Login berhasil menghapus hitungan (IP, username).
#[derive(Debug, Default)]
pub struct LoginLimiter {
    buckets: HashMap<String, Bucket>,
}

pub const WINDOW: Duration = Duration::from_secs(15 * 60);
const PAIR_LIMIT: u32 = 5;
const IP_LIMIT: u32 = 20;

impl LoginLimiter {
    fn pair_key(ip: &str, user: &str) -> String {
        format!("u:{ip}|{}", user.to_lowercase())
    }
    fn ip_key(ip: &str) -> String {
        format!("ip:{ip}")
    }

    /// Sisa waktu blokir bila (ip, user) sedang diblokir.
    pub fn blocked(&self, ip: &str, user: &str, now: Instant) -> Option<Duration> {
        [Self::pair_key(ip, user), Self::ip_key(ip)]
            .iter()
            .filter_map(|k| self.buckets.get(k)?.locked_until)
            .filter(|t| *t > now)
            .max()
            .map(|t| t - now)
    }

    /// Catat kegagalan; mengembalikan durasi blokir bila percobaan ini memicu blokir.
    pub fn record_failure(&mut self, ip: &str, user: &str, now: Instant) -> Option<Duration> {
        let mut triggered = None;
        for (key, limit) in [(Self::pair_key(ip, user), PAIR_LIMIT), (Self::ip_key(ip), IP_LIMIT)] {
            let b = self.buckets.entry(key.clone()).or_insert(Bucket {
                fails: 0,
                window_start: now,
                locked_until: None,
                lockouts: 0,
            });
            if now.duration_since(b.window_start) > WINDOW {
                b.fails = 0;
                b.window_start = now;
            }
            b.fails += 1;
            if b.fails >= limit {
                let base = if key.starts_with("ip:") { 3600 } else { 15 * 60 };
                let secs = (base as u64).saturating_mul(1u64 << b.lockouts.min(6)).min(24 * 3600);
                let d = Duration::from_secs(secs);
                b.locked_until = Some(now + d);
                b.lockouts += 1;
                b.fails = 0;
                b.window_start = now;
                triggered = Some(triggered.map_or(d, |t: Duration| t.max(d)));
            }
        }
        triggered
    }

    pub fn record_success(&mut self, ip: &str, user: &str) {
        self.buckets.remove(&Self::pair_key(ip, user));
    }

    /// Buang catatan lama agar memori tidak tumbuh.
    pub fn prune(&mut self, now: Instant) {
        self.buckets.retain(|_, b| {
            b.locked_until.map(|t| t > now).unwrap_or(false) || now.duration_since(b.window_start) <= WINDOW * 4
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cookies() {
        assert_eq!(cookie_value("a=1; sk_session=abc ; b=2", SESSION_COOKIE), Some("abc"));
        assert_eq!(cookie_value("sk_session=", SESSION_COOKIE), None);
        assert_eq!(cookie_value("x_sk_session=1", SESSION_COOKIE), None);
        assert_eq!(cookie_value("", SESSION_COOKIE), None);
    }

    #[test]
    fn origin_checks() {
        assert!(same_origin(Some("https://kost.example.com"), Some("kost.example.com")));
        assert!(same_origin(Some("http://192.168.1.10"), Some("192.168.1.10")));
        assert!(same_origin(Some("http://192.168.1.10:8080/x"), Some("192.168.1.10:8080")));
        assert!(!same_origin(Some("https://evil.example"), Some("kost.example.com")));
        assert!(!same_origin(Some("null"), Some("kost.example.com")));
        assert!(!same_origin(Some("https://kost.example.com.evil.io"), Some("kost.example.com")));
        assert!(same_origin(None, Some("kost.example.com")));
    }

    #[test]
    fn password_policy() {
        assert!(check_password_policy("admin", "pendek1").is_err());
        assert!(check_password_policy("admin", "admin-Kost-2026").is_err());
        assert!(check_password_policy("admin", "1234567890").is_err());
        assert!(check_password_policy("admin", "hanyahurufkecil").is_err());
        assert!(check_password_policy("admin", "Pintu-Hijau-7").is_ok());
        assert!(check_password_policy("admin", "kalimat sandi yang panjang sekali").is_ok());
    }

    #[test]
    fn limiter_blocks_after_five_and_escalates() {
        let mut l = LoginLimiter::default();
        let t0 = Instant::now();
        for i in 0..4 {
            assert!(l.record_failure("1.2.3.4", "admin", t0 + Duration::from_secs(i)).is_none());
        }
        let d = l.record_failure("1.2.3.4", "admin", t0 + Duration::from_secs(5)).unwrap();
        assert_eq!(d, Duration::from_secs(900));
        assert!(l.blocked("1.2.3.4", "Admin", t0 + Duration::from_secs(10)).is_some());
        // IP lain tidak terdampak
        assert!(l.blocked("5.6.7.8", "admin", t0 + Duration::from_secs(10)).is_none());
        // setelah blokir habis, 5 gagal lagi -> blokir 2x lebih lama
        let t1 = t0 + Duration::from_secs(1000);
        assert!(l.blocked("1.2.3.4", "admin", t1).is_none());
        let mut last = None;
        for i in 0..5 {
            last = l.record_failure("1.2.3.4", "admin", t1 + Duration::from_secs(i));
        }
        assert_eq!(last.unwrap(), Duration::from_secs(1800));
        // sukses menghapus hitungan pasangan
        l.record_success("1.2.3.4", "admin");
        l.prune(t1 + Duration::from_secs(10_000));
    }

    #[test]
    fn limiter_blocks_ip_guessing_many_usernames() {
        let mut l = LoginLimiter::default();
        let t0 = Instant::now();
        let mut hit = None;
        for i in 0..20 {
            hit = l.record_failure("9.9.9.9", &format!("user{i}"), t0);
        }
        assert_eq!(hit.unwrap(), Duration::from_secs(3600));
        assert!(l.blocked("9.9.9.9", "siapa-saja", t0).is_some());
    }
}
