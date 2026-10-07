use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::Json;
use common::error::AppError;
use serde::Serialize;

use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct ChannelInfo {
    pub id: String,
    pub label: String,
}

pub async fn list_channels(State(state): State<AppState>) -> Result<Json<Vec<ChannelInfo>>, AppError> {
    let channels = state.nvr.channels.iter().map(|c| ChannelInfo {
        id: c.id.clone(),
        label: c.label.clone(),
    }).collect();
    Ok(Json(channels))
}

#[derive(Debug, Serialize)]
pub struct PlayerResponse {
    pub player_path: String,
}

pub async fn live_view(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> Result<Json<PlayerResponse>, AppError> {
    state.nvr.find_channel(&channel_id)
        .ok_or_else(|| AppError::NotFound(format!("channel {channel_id} tidak terdaftar di config NVR")))?;

    Ok(Json(PlayerResponse {
        player_path: format!("/live/stream.html?src={channel_id}"),
    }))
}

pub async fn camera_snapshot(
    State(state): State<AppState>,
    Path(source): Path<String>,
) -> Result<(StatusCode, [(header::HeaderName, String); 1], Body), AppError> {
    let snapshot_url = state.nvr.build_snapshot_url_for_channel_id(&source)
        .ok_or_else(|| AppError::NotFound(format!("camera/source {source} tidak terdaftar di config NVR")))?;

    let client = reqwest::Client::new();
    let response = client
        .get(&snapshot_url)
        .basic_auth(&state.nvr.username, Some(&state.nvr.password))
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("gagal fetch snapshot dari NVR: {e}")))?;

    if !response.status().is_success() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "NVR mengembalikan status {} untuk snapshot {}",
            response.status(),
            snapshot_url
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("gagal membaca body snapshot NVR: {e}")))?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/jpeg".to_string())],
        Body::from(bytes.to_vec()),
    ))
}

