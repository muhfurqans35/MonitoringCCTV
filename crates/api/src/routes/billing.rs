use axum::extract::{Path, State};
use axum::Json;
use common::error::AppError;
use common::models::{BillingRecord, Room};

use crate::state::AppState;

/// GET /api/billing/:room_code - histori tagihan 1 kamar
pub async fn list_billing_for_room(
    State(state): State<AppState>,
    Path(room_code): Path<String>,
) -> Result<Json<Vec<BillingRecord>>, AppError> {
    let room = sqlx::query_as::<_, Room>("SELECT * FROM rooms WHERE code = ?")
        .bind(&room_code)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("kamar {room_code} tidak ditemukan")))?;

    let records = sqlx::query_as::<_, BillingRecord>(
        "SELECT * FROM billing_records WHERE room_id = ? ORDER BY period DESC",
    )
    .bind(room.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(records))
}

/// POST /api/billing/:room_code/generate?period=2026-07
/// Menghitung total kWh dari InfluxDB (delta kwh_cumulative awal-akhir periode)
/// dikali tarif per kamar, lalu simpan sebagai BillingRecord berstatus draft.
/// NOTE: implementasi agregasi Flux detail sengaja disederhanakan di sini —
/// lihat billing_service di alertengine/cron untuk kalkulasi penuh.
pub async fn generate_billing(
    State(state): State<AppState>,
    Path(room_code): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<BillingRecord>, AppError> {
    let period = params
        .get("period")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("query param 'period' wajib diisi, contoh: 2026-07".into()))?;

    let room = sqlx::query_as::<_, Room>("SELECT * FROM rooms WHERE code = ?")
        .bind(&room_code)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("kamar {room_code} tidak ditemukan")))?;

    // Placeholder kalkulasi kWh — pada implementasi penuh, query Flux untuk
    // ambil first() dan last() dari field kwh_cumulative dalam range 1 bulan,
    // lalu selisihkan. Di sini nilai kwh dummy 0 dulu supaya endpoint tetap berfungsi
    // end-to-end sebelum agregasi Flux final diisi.
    let total_kwh: f64 = 0.0;
    let total_amount = total_kwh * room.tariff_per_kwh;

    let record = sqlx::query_as::<_, BillingRecord>(
        "INSERT INTO billing_records (room_id, period, total_kwh, total_amount, status) \
         VALUES (?, ?, ?, ?, 'draft') \
         ON CONFLICT(room_id, period) DO UPDATE SET total_kwh = excluded.total_kwh, total_amount = excluded.total_amount \
         RETURNING *",
    )
    .bind(room.id)
    .bind(&period)
    .bind(total_kwh)
    .bind(total_amount)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(record))
}
