use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

const API_BASE: &str = "/api";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    pub id: i64,
    pub code: String,
    pub floor: i64,
    pub tariff_per_kwh: f64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomLiveStatus {
    pub room_code: String,
    pub presence: bool,
    pub watt: f32,
    pub kwh_cumulative: f32,
    pub timestamp_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertEvent {
    pub id: i64,
    pub source: String,
    pub alert_type: String,
    pub severity: String,
    pub message: String,
    pub resolved: bool,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
struct LoginRequest<'a> {
    username: &'a str,
    password: &'a str,
}

/// Akun yang sedang login (sesi cookie HttpOnly dari server; tidak ada token di JavaScript).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Me {
    pub username: String,
    pub role: String,
    #[allow(dead_code)]
    pub expires_at: i64,
}

impl Me {
    pub fn is_owner(&self) -> bool {
        self.role == "owner"
    }
}

/// Sesi disimpan di cookie HttpOnly yang dikirim browser otomatis (same-origin), jadi tidak ada
/// header Authorization. Fungsi ini dipertahankan agar pemanggil lama tetap sama.
fn auth_header(req: gloo_net::http::RequestBuilder) -> gloo_net::http::RequestBuilder {
    req
}

/// Sesi habis/dicabut: muat ulang halaman -> /me gagal -> layar login.
fn session_expired() {
    if let Some(w) = web_sys::window() {
        let _ = w.location().reload();
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, String> {
    let resp = Request::get(&format!("{API_BASE}{path}")).send().await.map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(error_text(resp).await);
    }
    resp.json().await.map_err(|e| e.to_string())
}

async fn post_json(path: &str, body: Option<serde_json::Value>) -> Result<gloo_net::http::Response, String> {
    let req = Request::post(&format!("{API_BASE}{path}"));
    let resp = match body {
        Some(b) => req.json(&b).map_err(|e| e.to_string())?.send().await,
        None => req.send().await,
    }
    .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(error_text(resp).await);
    }
    Ok(resp)
}

pub async fn login(username: &str, password: &str) -> Result<Me, String> {
    let resp = Request::post(&format!("{API_BASE}/auth/login"))
        .json(&LoginRequest { username, password })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|_| "server tidak bisa dihubungi".to_string())?;
    if !resp.ok() {
        let status = resp.status();
        let msg = resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string));
        return Err(msg.unwrap_or_else(|| match status {
            429 => "terlalu banyak percobaan, tunggu sebentar".into(),
            _ => "username atau password salah".into(),
        }));
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// Akun saat ini, atau Err bila belum login / sesi habis (tanpa memuat ulang halaman).
pub async fn fetch_me() -> Result<Me, String> {
    let resp = Request::get(&format!("{API_BASE}/auth/me")).send().await.map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

pub async fn logout() {
    let _ = post_json("/auth/logout", None).await;
}

pub async fn logout_all() -> Result<(), String> {
    post_json("/auth/logout-all", None).await.map(|_| ())
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SessionItem {
    pub created_at: i64,
    pub last_seen: i64,
    pub ip: String,
    pub user_agent: String,
    pub current: bool,
}

pub async fn fetch_sessions() -> Result<Vec<SessionItem>, String> {
    get_json("/auth/sessions").await
}

pub async fn change_password(current: &str, new: &str) -> Result<(), String> {
    post_json("/auth/password", Some(serde_json::json!({ "current": current, "new": new }))).await.map(|_| ())
}

pub async fn fetch_rooms() -> Result<Vec<Room>, String> {
    get_json("/rooms").await
}

#[allow(dead_code)]
pub async fn fetch_live_snapshot() -> Result<Vec<RoomLiveStatus>, String> {
    get_json("/rooms/live").await
}

pub async fn fetch_alerts() -> Result<Vec<AlertEvent>, String> {
    get_json("/alerts").await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CctvChannel {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Deserialize)]
struct PlayerResponse {
    player_path: String,
}

pub async fn fetch_cctv_channels() -> Result<Vec<CctvChannel>, String> {
    get_json("/cctv/channels").await
}

pub async fn fetch_live_player_path(channel_id: &str) -> Result<String, String> {
    let req = auth_header(Request::get(&format!("{API_BASE}/cctv/live/{channel_id}")));
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err("gagal buka live view, cek apakah channel valid".into());
    }
    let body: PlayerResponse = resp.json().await.map_err(|e| e.to_string())?;
    Ok(body.player_path)
}

// ---------------------------------------------------------------------------
// Modul doorbell (paket kost)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Package {
    pub id: i64,
    pub room_code: String,
    pub status: String,
    pub verified: bool,
    pub late: bool,
    pub note: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoorbellStatus {
    pub door_id: String,
    pub mode: String,
    pub modes: Vec<String>,
    pub node_online: bool,
    pub node_ip: String,
    pub config_in_sync: bool,
    pub calibrated: bool,
    pub waiting: i64,
    pub verifying: i64,
    // Field kamera: `default` supaya dashboard tetap jalan dengan API versi lama.
    #[serde(default)]
    pub camera_ok: bool,
    #[serde(default)]
    pub camera_sensor: String,
    #[serde(default)]
    pub camera_error: String,
    #[serde(default)]
    pub stream_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActionResult {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InviteResult {
    pub ok: bool,
    pub code: String,
    pub message: String,
    pub ttl_hours: i32,
}

#[derive(Serialize)]
struct ModeBody<'a> {
    mode: &'a str,
}

#[derive(Serialize)]
struct InviteBody<'a> {
    room_code: &'a str,
}

fn http_error(status: u16) -> String {
    match status {
        401 => {
            session_expired();
            "sesi habis, silakan login ulang".to_string()
        }
        403 => "hanya pemilik yang boleh melakukan aksi ini".to_string(),
        s => format!("permintaan gagal (HTTP {s})"),
    }
}

pub async fn fetch_doorbell_status() -> Result<DoorbellStatus, String> {
    let resp = auth_header(Request::get(&format!("{API_BASE}/doorbell/status")))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(http_error(resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

pub async fn fetch_packages() -> Result<Vec<Package>, String> {
    let resp = auth_header(Request::get(&format!("{API_BASE}/doorbell/packages")))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(http_error(resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

pub async fn close_package(id: i64) -> Result<ActionResult, String> {
    let resp = auth_header(Request::post(&format!("{API_BASE}/doorbell/packages/{id}/close")))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(http_error(resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

pub async fn set_doorbell_mode(mode: &str) -> Result<ActionResult, String> {
    let resp = auth_header(Request::post(&format!("{API_BASE}/doorbell/mode")))
        .json(&ModeBody { mode })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(http_error(resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

pub async fn create_invite(room_code: &str) -> Result<InviteResult, String> {
    let resp = auth_header(Request::post(&format!("{API_BASE}/doorbell/invites")))
        .json(&InviteBody { room_code })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(http_error(resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

pub async fn calibrate_doorbell() -> Result<ActionResult, String> {
    let resp = auth_header(Request::post(&format!("{API_BASE}/doorbell/calibrate")))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(http_error(resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// Foto terbaru kamera pintu sebagai data URL (`data:image/jpeg;base64,...`) siap dipasang
/// di `<img src>`. Lewat fetch (bukan `<img src=/api/...>` langsung) karena endpoint butuh
/// header Authorization JWT yang tidak bisa dikirim oleh tag `<img>`.
pub async fn fetch_door_snapshot() -> Result<String, String> {
    let resp = auth_header(Request::get(&format!("{API_BASE}/doorbell/camera/snapshot")))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == 503 {
        #[derive(Deserialize)]
        struct ErrBody {
            error: String,
        }
        let msg = resp.json::<ErrBody>().await.map(|b| b.error).unwrap_or_else(|_| "kamera tidak merespons".into());
        return Err(msg);
    }
    if resp.status() == 500 {
        return Err("service doorbell tidak terjangkau dari api (cek DOORBELL_GRPC_ADDR / service doorbell berjalan)".into());
    }
    if !resp.ok() {
        return Err(http_error(resp.status()));
    }
    let bytes = resp.binary().await.map_err(|e| e.to_string())?;
    Ok(format!("data:image/jpeg;base64,{}", base64_encode(&bytes)))
}

// ---------------------------------------------------------------------------
// Galeri deteksi CCTV (hasil service vision di data/vision)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BoxDet {
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub bbox: [f32; 4],
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DetectionItem {
    pub image: String,
    pub camera_id: String,
    #[serde(default)]
    pub camera_label: String,
    pub label: String,
    #[serde(default)]
    pub confidence: f32,
    pub timestamp: String,
    #[serde(default)]
    pub image_width: u32,
    #[serde(default)]
    pub image_height: u32,
    #[serde(default)]
    pub detections: Vec<BoxDet>,
}

pub async fn fetch_detections(limit: usize, camera: Option<&str>) -> Result<Vec<DetectionItem>, String> {
    let mut url = format!("{API_BASE}/cctv/detections?limit={limit}");
    if let Some(c) = camera.filter(|c| !c.is_empty()) {
        url.push_str(&format!("&camera={c}"));
    }
    let resp = auth_header(Request::get(&url)).send().await.map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(http_error(resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// Gambar deteksi sebagai data URL (fetch dengan JWT; tag <img> tidak bisa mengirim header).
pub async fn fetch_detection_image(path: &str) -> Result<String, String> {
    let enc: String = path
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    let resp = auth_header(Request::get(&format!("{API_BASE}/cctv/detections/image?path={enc}")))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(http_error(resp.status()));
    }
    let bytes = resp.binary().await.map_err(|e| e.to_string())?;
    Ok(format!("data:image/jpeg;base64,{}", base64_encode(&bytes)))
}

// ------------------------------------------------------------------ playback NVR

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NvrRecording {
    /// Jam NVR, "2026-10-06T07:56:23"
    pub begin: String,
    pub end: String,
    /// motion | alarm | regular | manual | other
    pub kind: String,
    pub seconds: i64,
    pub size_kb: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NvrRecordings {
    pub camera_id: String,
    pub label: String,
    pub channel: u32,
    pub date: String,
    pub recordings: Vec<NvrRecording>,
    pub motion_events: usize,
}

/// Pesan error JSON dari API ({"error": "..."}) bila ada, selain itu kode HTTP.
async fn error_text(resp: gloo_net::http::Response) -> String {
    let status = resp.status();
    if status == 401 {
        return http_error(401);
    }
    match resp.json::<serde_json::Value>().await {
        Ok(v) => v.get("error").and_then(|e| e.as_str()).map(|s| s.to_string()).unwrap_or_else(|| http_error(status)),
        Err(_) => http_error(status),
    }
}

pub async fn fetch_nvr_recordings(camera: &str, date: &str) -> Result<NvrRecordings, String> {
    let resp = auth_header(Request::get(&format!("{API_BASE}/nvr/recordings?camera={camera}&date={date}")))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(error_text(resp).await);
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// Semua deteksi vision satu hari (untuk ditandai di timeline playback).
pub async fn fetch_detections_day(camera: &str, date: &str) -> Result<Vec<DetectionItem>, String> {
    let resp = auth_header(Request::get(&format!("{API_BASE}/cctv/detections?camera={camera}&date={date}&limit=1000")))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(error_text(resp).await);
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// Klip rekaman NVR sebagai blob URL (`blob:...`) untuk <video>. Panggil `revoke_blob_url`
/// saat klip diganti agar memori browser dilepas.
pub async fn fetch_nvr_clip(camera: &str, start: &str, seconds: u32) -> Result<String, String> {
    let resp = auth_header(Request::get(&format!("{API_BASE}/nvr/clip?camera={camera}&start={start}&seconds={seconds}")))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(error_text(resp).await);
    }
    let bytes = resp.binary().await.map_err(|e| e.to_string())?;
    let arr = js_sys::Uint8Array::from(bytes.as_slice());
    let parts = js_sys::Array::new();
    parts.push(&arr);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("video/mp4");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(|_| "gagal membuat video".to_string())?;
    web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| "gagal membuat video".to_string())
}

pub fn revoke_blob_url(url: &str) {
    let _ = web_sys::Url::revoke_object_url(url);
}

pub async fn resolve_alert(id: i64) -> Result<(), String> {
    let resp = auth_header(Request::post(&format!("{API_BASE}/alerts/{id}/resolve")))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(http_error(resp.status()));
    }
    Ok(())
}

/// Base64 standar (RFC 4648) tanpa dependensi tambahan.
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0xff, 0xd8, 0xff]), "/9j/");
    }
}
