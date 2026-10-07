use axum::extract::{Path, State};
use axum::Json;
use common::error::AppError;
use common::models::Room;
use proto::telemetry::LiveSnapshotRequest;
use serde::Serialize;

use crate::state::AppState;

/// GET /api/rooms - daftar seluruh kamar (data relational dari SQLite)
pub async fn list_rooms(State(state): State<AppState>) -> Result<Json<Vec<Room>>, AppError> {
    let rooms = sqlx::query_as::<_, Room>("SELECT * FROM rooms ORDER BY code")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rooms))
}

/// GET /api/rooms/:code - detail 1 kamar
pub async fn get_room(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Json<Room>, AppError> {
    let room = sqlx::query_as::<_, Room>("SELECT * FROM rooms WHERE code = ?")
        .bind(&code)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("kamar {code} tidak ditemukan")))?;
    Ok(Json(room))
}

#[derive(Debug, Serialize)]
pub struct RoomLiveStatus {
    pub room_code: String,
    pub presence: bool,
    pub watt: f32,
    pub kwh_cumulative: f32,
    pub timestamp_unix: i64,
}

/// GET /api/rooms/live - snapshot occupancy & daya seluruh kamar saat ini,
/// diambil langsung dari ingest service via gRPC (bukan query InfluxDB,
/// supaya latensi rendah untuk data "sekarang").
pub async fn live_snapshot(
    State(mut state): State<AppState>,
) -> Result<Json<Vec<RoomLiveStatus>>, AppError> {
    let resp = state
        .telemetry_client
        .get_live_snapshot(LiveSnapshotRequest {})
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("gagal ambil snapshot: {e}")))?;

    let rooms = resp
        .into_inner()
        .rooms
        .into_iter()
        .map(|r| RoomLiveStatus {
            room_code: r.room_code,
            presence: r.presence,
            watt: r.watt,
            kwh_cumulative: r.kwh_cumulative,
            timestamp_unix: r.timestamp_unix,
        })
        .collect();

    Ok(Json(rooms))
}
