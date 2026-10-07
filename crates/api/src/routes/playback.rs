//! Playback rekaman NVR langsung dari NVR (protokol DVRIP biner, crate `dvrip`).
//!
//!   GET /api/nvr/recordings?camera=gate_right&date=2026-10-06
//!       daftar rekaman sehari + event motion/alarm NVR (file bertanda [M]/[A])
//!   GET /api/nvr/clip?camera=gate_right&start=2026-10-06T07:56:23&seconds=30
//!       klip MP4 (H.264 asli NVR, di-remux ffmpeg tanpa encode ulang; disimpan sementara di
//!       NVR_CLIP_DIR, default data/nvr-clips, dihapus otomatis setelah 24 jam)
//!
//! Semua jam memakai JAM NVR (zona lokal, tanpa offset). Pastikan jam NVR sinkron (NTP).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use common::error::AppError;
use dvrip::{Nvr, NvrTime, RecFile};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Semaphore};

use crate::state::AppState;

const CLIP_MIN_S: u32 = 5;
const CLIP_MAX_S: u32 = 120;

fn nvr_client(state: &AppState) -> Nvr {
    Nvr::new(state.nvr.ip.clone(), state.nvr.dvrip_port, state.nvr.username.clone(), state.nvr.password.clone())
}

fn clip_dir() -> PathBuf {
    PathBuf::from(std::env::var("NVR_CLIP_DIR").unwrap_or_else(|_| "data/nvr-clips".to_string()))
}

/// Maks 2 sesi playback NVR bersamaan (NVR murah cepat kewalahan).
fn playback_slots() -> &'static Semaphore {
    static S: OnceLock<Semaphore> = OnceLock::new();
    S.get_or_init(|| Semaphore::new(2))
}

type ListCache = Mutex<HashMap<(String, String), (Instant, Vec<RecordingItem>)>>;
fn list_cache() -> &'static ListCache {
    static C: OnceLock<ListCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn camera(state: &AppState, id: &str) -> Result<(u32, String), PbError> {
    state
        .nvr
        .find_channel(id)
        .map(|c| (c.channel_number, c.label.clone()))
        .ok_or_else(|| PbError(StatusCode::NOT_FOUND, format!("kamera {id} tidak terdaftar di config/nvr.toml")))
}

fn nvr_err(e: impl std::fmt::Display) -> PbError {
    PbError::gw(format!("NVR: {e}"))
}

/// Error playback yang pesannya tetap sampai ke dashboard (AppError::Internal menyembunyikannya).
#[derive(Debug)]
pub struct PbError(StatusCode, String);

impl PbError {
    fn gw(msg: String) -> Self {
        tracing::warn!("playback: {msg}");
        PbError(StatusCode::BAD_GATEWAY, msg)
    }
}

impl From<AppError> for PbError {
    fn from(e: AppError) -> Self {
        let msg = e.to_string();
        PbError(e.into_response().status(), msg)
    }
}

impl IntoResponse for PbError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

// ------------------------------------------------------------------------------- daftar

#[derive(Debug, Deserialize)]
pub struct RecordingsQuery {
    camera: String,
    /// YYYY-MM-DD (jam NVR). Default: hari ini menurut jam server.
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordingItem {
    /// "2026-10-06T07:56:23" (jam NVR)
    pub begin: String,
    pub end: String,
    /// motion | alarm | regular | manual | other
    pub kind: String,
    pub seconds: i64,
    pub size_kb: u32,
}

#[derive(Debug, Serialize)]
pub struct RecordingsResponse {
    pub camera_id: String,
    pub label: String,
    pub channel: u32,
    pub date: String,
    pub recordings: Vec<RecordingItem>,
    pub motion_events: usize,
}

fn to_item(f: &RecFile) -> RecordingItem {
    RecordingItem {
        begin: f.begin.to_string(),
        end: f.end.to_string(),
        kind: f.kind_name().to_string(),
        seconds: f.end.secs_since(&f.begin),
        size_kb: f.size_kb,
    }
}

fn today_local() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// GET /api/nvr/recordings
pub async fn recordings(
    State(state): State<AppState>,
    Query(q): Query<RecordingsQuery>,
) -> Result<Json<RecordingsResponse>, PbError> {
    let (channel, label) = camera(&state, &q.camera)?;
    let date = q.date.filter(|d| !d.is_empty()).unwrap_or_else(today_local);
    let day = NvrTime::parse(&date).ok_or_else(|| PbError(StatusCode::BAD_REQUEST, "format tanggal harus YYYY-MM-DD".into()))?;
    let is_today = date == today_local();
    let ttl = Duration::from_secs(if is_today { 45 } else { 3600 });

    let key = (q.camera.clone(), date.clone());
    let cached = {
        let c = list_cache().lock().await;
        c.get(&key).filter(|(t, _)| t.elapsed() < ttl).map(|(_, v)| v.clone())
    };
    let items = match cached {
        Some(v) => v,
        None => {
            let nvr = nvr_client(&state);
            let files = tokio::task::spawn_blocking(move || nvr.find_files(channel, day.start_of_day(), day.end_of_day()))
                .await
                .map_err(|e| PbError::gw(format!("task NVR gagal: {e}")))?
                .map_err(nvr_err)?;
            let v: Vec<RecordingItem> = files.iter().map(to_item).collect();
            let mut c = list_cache().lock().await;
            c.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(3600));
            c.insert(key, (Instant::now(), v.clone()));
            v
        }
    };
    let motion_events = items.iter().filter(|r| r.kind == "motion" || r.kind == "alarm").count();
    Ok(Json(RecordingsResponse { camera_id: q.camera, label, channel, date, recordings: items, motion_events }))
}

// ------------------------------------------------------------------------------- klip

#[derive(Debug, Deserialize)]
pub struct ClipQuery {
    camera: String,
    /// "2026-10-06T07:56:23" (jam NVR)
    start: String,
    #[serde(default)]
    seconds: Option<u32>,
}

/// GET /api/nvr/clip -> video/mp4
pub async fn clip(State(state): State<AppState>, Query(q): Query<ClipQuery>) -> Result<Response, PbError> {
    let (channel, _) = camera(&state, &q.camera)?;
    let start = NvrTime::parse(&q.start)
        .ok_or_else(|| PbError(StatusCode::BAD_REQUEST, "start harus berformat YYYY-MM-DDTHH:MM:SS".into()))?;
    let seconds = q.seconds.unwrap_or(30).clamp(CLIP_MIN_S, CLIP_MAX_S);

    let dir = clip_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| PbError::gw(format!("gagal membuat {}: {e}", dir.display())))?;
    let stem = format!(
        "{}_{:04}{:02}{:02}_{:02}{:02}{:02}_{seconds}",
        q.camera.replace(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-', "_"),
        start.year,
        start.month,
        start.day,
        start.hour,
        start.minute,
        start.second
    );
    let mp4 = dir.join(format!("{stem}.mp4"));

    if !mp4.exists() {
        let _permit = playback_slots()
            .acquire()
            .await
            .map_err(|_| PbError::gw(format!("antrean playback ditutup")))?;
        if !mp4.exists() {
            let nvr = nvr_client(&state);
            let t0 = Instant::now();
            let clip = tokio::task::spawn_blocking(move || nvr.clip(channel, start, seconds))
                .await
                .map_err(|e| PbError::gw(format!("task NVR gagal: {e}")))?
                .map_err(|e| PbError(StatusCode::NOT_FOUND, format!("rekaman tidak bisa diambil: {e}")))?;
            tracing::info!(
                "playback {} {start} {seconds}s: {} frame {}x{} @{} fps dalam {:.1}s",
                q.camera,
                clip.frames,
                clip.width,
                clip.height,
                clip.fps,
                t0.elapsed().as_secs_f32()
            );
            remux(&dir, &stem, &clip.h264, clip.fps.max(1)).await?;
        }
        purge_old(&dir).await;
    }

    let bytes = tokio::fs::read(&mp4)
        .await
        .map_err(|e| PbError::gw(format!("gagal membaca klip: {e}")))?;
    let mut resp = (StatusCode::OK, Body::from(bytes)).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    Ok(resp)
}

/// H.264 mentah -> MP4 (faststart) dengan ffmpeg, tanpa encode ulang (ringan untuk Pi).
async fn remux(dir: &std::path::Path, stem: &str, h264: &[u8], fps: u32) -> Result<(), PbError> {
    // Nama sementara unik: dua permintaan klip yang sama tidak saling menimpa (rename terakhir menang).
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let raw = dir.join(format!("{stem}.{n}.h264"));
    let tmp = dir.join(format!("{stem}.{n}.part"));
    let out = dir.join(format!("{stem}.mp4"));
    tokio::fs::write(&raw, h264)
        .await
        .map_err(|e| PbError::gw(format!("gagal menulis {}: {e}", raw.display())))?;
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "h264", "-framerate", &fps.to_string(), "-i"])
        .arg(&raw)
        .args(["-c", "copy", "-movflags", "+faststart", "-f", "mp4"])
        .arg(&tmp)
        .status()
        .await;
    let _ = tokio::fs::remove_file(&raw).await;
    match status {
        Ok(s) if s.success() => tokio::fs::rename(&tmp, &out)
            .await
            .map_err(|e| PbError::gw(format!("gagal menyimpan klip: {e}"))),
        Ok(s) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            Err(PbError::gw(format!("ffmpeg gagal mengubah klip ke MP4 ({s})")))
        }
        Err(e) => Err(PbError::gw(format!(
            "ffmpeg tidak bisa dijalankan ({e}). Pasang dengan: sudo apt install ffmpeg"
        ))),
    }
}

/// Hapus klip > 24 jam dan sisakan maks 200 file terbaru.
async fn purge_old(dir: &std::path::Path) {
    let dir = dir.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || {
        let mut files: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(&dir)
            .map(|d| {
                d.flatten()
                    .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                    .collect()
            })
            .unwrap_or_default();
        // Sisa file sementara (.h264/.part) dari proses yang terhenti: hapus bila > 1 jam.
        let (mut files, temps): (Vec<_>, Vec<_>) =
            files.drain(..).partition(|(_, p)| p.extension().map(|x| x == "mp4").unwrap_or(false));
        for (t, p) in temps {
            if t.elapsed().map(|e| e > Duration::from_secs(3600)).unwrap_or(false) {
                let _ = std::fs::remove_file(p);
            }
        }
        files.sort_by(|a, b| b.0.cmp(&a.0));
        let day = Duration::from_secs(24 * 3600);
        for (i, (t, p)) in files.iter().enumerate() {
            let old = t.elapsed().map(|e| e > day).unwrap_or(false);
            if old || i >= 200 {
                let _ = std::fs::remove_file(p);
            }
        }
    })
    .await;
}
