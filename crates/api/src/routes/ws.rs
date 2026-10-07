use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use proto::telemetry::StreamRoomRequest;

use crate::security;
use crate::state::AppState;

/// GET /ws - upgrade ke WebSocket, lalu stream update occupancy/power semua kamar
/// (room_code kosong = semua kamar) langsung dari ingest service via gRPC stream,
/// diteruskan sebagai JSON ke browser. Dipakai frontend Leptos untuk update realtime
/// tanpa polling REST.
///
/// Keamanan: butuh sesi login (cookie) dan Origin harus situs ini sendiri — browser selalu
/// mengirim Origin pada WebSocket, jadi halaman situs lain tidak bisa ikut membaca data kamar.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !security::request_same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(token) = security::token_from_headers(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if security::lookup(&state.db, &state.session_cfg, &token).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state, token))
}

async fn handle_socket(mut socket: WebSocket, mut state: AppState, token: String) {
    let stream_result = state
        .telemetry_client
        .stream_room_telemetry(StreamRoomRequest { room_code: String::new() })
        .await;

    let mut stream = match stream_result {
        Ok(resp) => resp.into_inner(),
        Err(e) => {
            tracing::warn!("gagal subscribe stream telemetry: {e}");
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    // Ping berkala: menjaga koneksi tetap hidup di balik proxy (nginx / Cloudflare).
    let mut ping = tokio::time::interval(std::time::Duration::from_secs(30));
    ping.tick().await;
    let mut ticks: u32 = 0;
    loop {
        tokio::select! {
            _ = ping.tick() => {
                // Sesi dicabut (logout / ganti password) atau kedaluwarsa -> tutup koneksi.
                ticks += 1;
                if ticks % 10 == 0 && security::lookup(&state.db, &state.session_cfg, &token).await.is_none() {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
                if socket.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            // Data baru dari ingest -> forward ke client sebagai JSON
            item = stream.message() => {
                match item {
                    Ok(Some(data)) => {
                        let json = serde_json::json!({
                            "room_code": data.room_code,
                            "presence": data.presence,
                            "distance_cm": data.distance_cm,
                            "voltage": data.voltage,
                            "current": data.current,
                            "watt": data.watt,
                            "kwh_cumulative": data.kwh_cumulative,
                            "timestamp_unix": data.timestamp_unix,
                        });
                        if socket.send(Message::Text(json.to_string())).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("grpc stream error: {e}");
                        break;
                    }
                }
            }
            // Client disconnect / kirim pesan (diabaikan, hanya untuk deteksi close)
            msg = socket.recv() => {
                if msg.is_none() {
                    break;
                }
            }
        }
    }
}
