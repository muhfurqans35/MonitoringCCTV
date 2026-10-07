use axum::extract::{Path, Query, State};
use axum::Json;
use common::error::AppError;
use serde::Deserialize;
use serde_json::Value;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    /// contoh: "-24h", "-7d". Default 24 jam terakhir.
    #[serde(default = "default_range")]
    range: String,
}
fn default_range() -> String { "-24h".to_string() }

/// GET /api/rooms/:code/history/power?range=-24h
/// Ambil histori daya (watt) 1 kamar dari InfluxDB, dikonversi ke JSON sederhana
/// [{time, watt}, ...] supaya gampang dipakai chart di frontend (recharts-equivalent di Leptos).
pub async fn power_history(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<Value>>, AppError> {
    let flux = format!(
        r#"from(bucket: "{bucket}")
  |> range(start: {range})
  |> filter(fn: (r) => r._measurement == "power" and r.room_code == "{code}")
  |> filter(fn: (r) => r._field == "watt")
  |> keep(columns: ["_time", "_value"])"#,
        bucket = state.influx_bucket_name(),
        range = q.range,
        code = code,
    );

    let csv = state
        .influx
        .query_flux(&flux)
        .await
        .map_err(|e| AppError::Influx(e.to_string()))?;

    Ok(Json(parse_flux_csv_time_value(&csv, "watt")))
}

/// GET /api/rooms/:code/history/occupancy?range=-24h
pub async fn occupancy_history(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<Value>>, AppError> {
    let flux = format!(
        r#"from(bucket: "{bucket}")
  |> range(start: {range})
  |> filter(fn: (r) => r._measurement == "occupancy" and r.room_code == "{code}")
  |> filter(fn: (r) => r._field == "presence")
  |> keep(columns: ["_time", "_value"])"#,
        bucket = state.influx_bucket_name(),
        range = q.range,
        code = code,
    );

    let csv = state
        .influx
        .query_flux(&flux)
        .await
        .map_err(|e| AppError::Influx(e.to_string()))?;

    Ok(Json(parse_flux_csv_time_value(&csv, "presence")))
}

/// Parser CSV minimal hasil response InfluxDB Flux query. Format resmi InfluxDB
/// CSV agak kompleks (multi-table dengan header berulang), fungsi ini menangani
/// kasus umum: 1 tabel per response, ambil kolom `_time` dan `_value`.
fn parse_flux_csv_time_value(csv: &str, value_key: &str) -> Vec<Value> {
    let mut result = Vec::new();
    let mut lines = csv.lines();

    let header_line = match lines.find(|l| l.starts_with(",result,")) {
        Some(h) => h,
        None => return result,
    };
    let headers: Vec<&str> = header_line.split(',').collect();
    let time_idx = headers.iter().position(|h| *h == "_time");
    let value_idx = headers.iter().position(|h| *h == "_value");

    let (Some(ti), Some(vi)) = (time_idx, value_idx) else { return result };

    for line in lines {
        if line.trim().is_empty() { continue; }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() <= ti.max(vi) { continue; }
        result.push(serde_json::json!({
            "time": cols[ti],
            value_key: cols[vi],
        }));
    }

    result
}
