//! Modul doorbell (semua rute khusus role `owner`): status node, daftar paket, dan aksi (mode, kode pendaftaran,
//! tutup paket, kalibrasi). Data dibaca dari SQLite; aksi diteruskan ke service `doorbell` lewat gRPC.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use common::auth::Claims;
use common::error::AppError;
use proto::doorbell::{
    CalibrateRequest, ClosePackageRequest, CreateInviteRequest, SetModeRequest, SnapshotRequest, StatusRequest,
};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Aksi yang mengubah keadaan hanya untuk pemilik (role "owner"), bukan penghuni.
fn require_owner(claims: &Claims) -> Result<(), AppError> {
    if claims.role == "owner" {
        Ok(())
    } else {
        Err(AppError::Forbidden("hanya pemilik yang boleh melakukan aksi ini".into()))
    }
}

fn grpc_err(e: tonic::Status) -> AppError {
    AppError::Internal(anyhow::anyhow!("service doorbell tidak terjangkau: {e}"))
}

#[derive(Debug, Serialize)]
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
    pub camera_ok: bool,
    pub camera_sensor: String,
    pub camera_error: String,
    /// URL MJPEG langsung ke node (hanya bisa dibuka dari LAN yang sama, butuh ?token=).
    pub stream_url: String,
}

/// GET /api/doorbell/status
pub async fn status(
    State(mut state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<DoorbellStatus>, AppError> {
    require_owner(&claims)?;
    let s = state.doorbell_client.get_status(StatusRequest {}).await.map_err(grpc_err)?.into_inner();
    Ok(Json(DoorbellStatus {
        config_in_sync: s.node_cfg_ver == s.hub_cfg_ver,
        door_id: s.door_id,
        mode: s.mode,
        modes: s.modes,
        node_online: s.node_online,
        node_ip: s.node_ip,
        calibrated: s.calibrated,
        waiting: s.waiting,
        verifying: s.verifying,
        camera_ok: s.camera_ok,
        camera_sensor: s.camera_sensor,
        camera_error: s.camera_error,
        stream_url: s.stream_url,
    }))
}

/// GET /api/doorbell/camera/snapshot — JPEG penuh terbaru dari kamera node pintu (live view).
/// Diambil lewat service doorbell (yang tahu IP + token node), sehingga browser tidak perlu
/// satu jaringan dengan ESP32 dan token perangkat tidak pernah sampai ke browser.
/// Gagal -> 503 JSON {"error": "<alasan>"} supaya dashboard bisa menampilkan penyebabnya.
pub async fn camera_snapshot(
    State(mut state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Response, AppError> {
    require_owner(&claims)?;
    let r = state.doorbell_client.snapshot(SnapshotRequest {}).await.map_err(grpc_err)?.into_inner();
    if !r.ok {
        return Ok((StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({ "error": r.message }))).into_response());
    }
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/jpeg".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::HeaderName::from_static("x-captured-at-ms"), r.captured_at_ms.to_string()),
        ],
        r.jpeg,
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
pub struct PackagesQuery {
    /// "waiting" (default: perlu tindakan) atau "all" (100 terakhir).
    #[serde(default)]
    scope: String,
}

#[derive(Debug, sqlx::FromRow)]
struct PackageRowDb {
    id: i64,
    room_code: Option<String>,
    status: String,
    verified: i64,
    late: i64,
    note: Option<String>,
    created_at: String,
}

#[derive(Debug, Serialize)]
pub struct PackageRow {
    pub id: i64,
    pub room_code: String,
    pub status: String,
    pub verified: bool,
    pub late: bool,
    pub note: String,
    /// UTC, format "YYYY-MM-DD HH:MM:SS".
    pub created_at: String,
}

/// GET /api/doorbell/packages?scope=waiting|all
/// Foto sengaja tidak disajikan lewat API: foto hanya dikirim ke penghuni & pengelola via Telegram.
pub async fn list_packages(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<PackagesQuery>,
) -> Result<Json<Vec<PackageRow>>, AppError> {
    // Daftar paket memuat kode kamar semua penghuni: hanya untuk pemilik.
    require_owner(&claims)?;
    let filter = if q.scope == "all" { "" } else { "AND status IN ('waiting','verifying','disputed')" };
    let sql = format!(
        "SELECT id, room_code, status, verified, late, note, datetime(ts, 'unixepoch') AS created_at \
         FROM doorbell_submissions WHERE purpose = 'paket' {filter} ORDER BY ts DESC LIMIT 100"
    );
    let rows: Vec<PackageRowDb> = sqlx::query_as(&sql).fetch_all(&state.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| PackageRow {
                id: r.id,
                room_code: r.room_code.unwrap_or_default(),
                status: r.status,
                verified: r.verified == 1,
                late: r.late == 1,
                note: r.note.unwrap_or_default(),
                created_at: r.created_at,
            })
            .collect(),
    ))
}

#[derive(Debug, Serialize)]
pub struct ActionResult {
    pub ok: bool,
    pub message: String,
}

/// POST /api/doorbell/packages/:id/close — tandai paket sudah diambil (pemilik).
pub async fn close_package(
    State(mut state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<Json<ActionResult>, AppError> {
    require_owner(&claims)?;
    let r = state.doorbell_client.close_package(ClosePackageRequest { id }).await.map_err(grpc_err)?.into_inner();
    Ok(Json(ActionResult { ok: r.ok, message: r.message }))
}

#[derive(Debug, Deserialize)]
pub struct SetModeBody {
    pub mode: String,
}

/// POST /api/doorbell/mode {"mode": "sibuk"}
pub async fn set_mode(
    State(mut state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<SetModeBody>,
) -> Result<Json<ActionResult>, AppError> {
    require_owner(&claims)?;
    let r = state.doorbell_client.set_mode(SetModeRequest { mode: body.mode }).await.map_err(grpc_err)?.into_inner();
    Ok(Json(ActionResult { ok: r.ok, message: r.message }))
}

#[derive(Debug, Deserialize)]
pub struct InviteBody {
    pub room_code: String,
}

#[derive(Debug, Serialize)]
pub struct InviteResult {
    pub ok: bool,
    pub code: String,
    pub message: String,
    pub ttl_hours: i32,
}

/// POST /api/doorbell/invites {"room_code": "K01"} — kode sekali pakai agar penghuni mendaftar
/// (opt-in) ke bot Telegram: penghuni mengirim `/start KODE`.
pub async fn create_invite(
    State(mut state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<InviteBody>,
) -> Result<Json<InviteResult>, AppError> {
    require_owner(&claims)?;
    let r = state
        .doorbell_client
        .create_invite(CreateInviteRequest { room_code: body.room_code })
        .await
        .map_err(grpc_err)?
        .into_inner();
    Ok(Json(InviteResult { ok: r.ok, code: r.code, message: r.message, ttl_hours: r.ttl_hours }))
}

/// POST /api/doorbell/calibrate — simpan foto zona KOSONG saat ini sebagai referensi.
pub async fn calibrate(
    State(mut state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ActionResult>, AppError> {
    require_owner(&claims)?;
    let r = state.doorbell_client.calibrate(CalibrateRequest {}).await.map_err(grpc_err)?.into_inner();
    Ok(Json(ActionResult { ok: r.ok, message: r.message }))
}
