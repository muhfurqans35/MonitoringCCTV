use axum::extract::State;
use axum::Json;
use common::error::AppError;
use common::models::AlertEvent;

use crate::state::AppState;

/// GET /api/alerts - daftar alert terbaru (limit 100), dipakai AlertFeed di frontend.
pub async fn list_alerts(State(state): State<AppState>) -> Result<Json<Vec<AlertEvent>>, AppError> {
    let alerts = sqlx::query_as::<_, AlertEvent>(
        "SELECT id, source, alert_type, severity, message, resolved, created_at \
         FROM alert_events ORDER BY created_at DESC LIMIT 100",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(alerts))
}

/// POST /api/alerts/:id/resolve - tandai alert sudah ditangani
pub async fn resolve_alert(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    sqlx::query("UPDATE alert_events SET resolved = 1 WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
