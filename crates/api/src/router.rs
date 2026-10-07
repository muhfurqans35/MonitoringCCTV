use axum::http::{header, HeaderValue};
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::middleware_auth::{require_auth, require_owner};
use crate::routes::{alerts, auth, billing, cctv, detections, doorbell, history, playback, rooms, ws};
use crate::state::AppState;

/// Susunan route:
///   publik       : POST /api/auth/login, GET /ws (memeriksa sesi + Origin sendiri)
///   login apa pun: data kamar, akun sendiri (me/logout/password/sesi)
///   khusus owner : CCTV, playback NVR, deteksi, alert, tagihan, bel pintu, cek /live/
/// Tidak ada CORS: dashboard dilayani dari origin yang sama (nginx), jadi situs lain tidak
/// boleh memanggil API ini dari browser.
pub fn build_router(state: AppState) -> Router {
    let public_routes = Router::new()
        .route("/api/auth/login", post(auth::login))
        .route("/ws", get(ws::ws_handler));

    let user_routes = Router::new()
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/logout-all", post(auth::logout_all))
        .route("/api/auth/sessions", get(auth::sessions))
        .route("/api/auth/password", post(auth::change_password))
        .route("/api/rooms", get(rooms::list_rooms))
        .route("/api/rooms/live", get(rooms::live_snapshot))
        .route("/api/rooms/:code", get(rooms::get_room))
        .route("/api/rooms/:code/history/power", get(history::power_history))
        .route("/api/rooms/:code/history/occupancy", get(history::occupancy_history));

    let owner_routes = Router::new()
        .route("/api/auth/check", get(auth::check))
        .route("/api/alerts", get(alerts::list_alerts))
        .route("/api/alerts/:id/resolve", post(alerts::resolve_alert))
        .route("/api/billing/:room_code", get(billing::list_billing_for_room))
        .route("/api/billing/:room_code/generate", post(billing::generate_billing))
        .route("/api/cctv/channels", get(cctv::list_channels))
        .route("/api/cctv/live/:channel_id", get(cctv::live_view))
        .route("/api/cameras/:source/snapshot", get(cctv::camera_snapshot))
        .route("/api/cctv/detections", get(detections::list))
        .route("/api/cctv/detections/image", get(detections::image))
        .route("/api/nvr/recordings", get(playback::recordings))
        .route("/api/nvr/clip", get(playback::clip))
        .route("/api/doorbell/status", get(doorbell::status))
        .route("/api/doorbell/packages", get(doorbell::list_packages))
        .route("/api/doorbell/packages/:id/close", post(doorbell::close_package))
        .route("/api/doorbell/mode", post(doorbell::set_mode))
        .route("/api/doorbell/invites", post(doorbell::create_invite))
        .route("/api/doorbell/calibrate", post(doorbell::calibrate))
        .route("/api/doorbell/camera/snapshot", get(doorbell::camera_snapshot))
        .route_layer(middleware::from_fn(require_owner));

    let protected_routes = user_routes
        .merge(owner_routes)
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        // Data pribadi: jangan disimpan cache browser/proxy kecuali handler menentukan sendiri.
        .layer(SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL, HeaderValue::from_static("no-store")))
        .layer(SetResponseHeaderLayer::overriding(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
