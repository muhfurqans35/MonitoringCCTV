use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use common::auth::Claims;
use common::error::AppError;

use crate::security::{self, Session};
use crate::state::AppState;

/// Wajib login: sesi dari cookie HttpOnly `sk_session` (lihat security.rs).
/// Permintaan yang mengubah data (selain GET/HEAD) juga harus berasal dari situs ini sendiri
/// (cek Origin/Referer) sebagai lapis kedua anti-CSRF di samping SameSite=Strict.
/// Claims tetap dimasukkan ke extensions agar handler lama (doorbell) bisa memeriksa peran.
pub async fn require_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Result<Response, AppError> {
    let session = security::session_from_headers(&state.db, &state.session_cfg, req.headers())
        .await
        .ok_or(AppError::Unauthorized)?;
    if !matches!(*req.method(), Method::GET | Method::HEAD) && !security::request_same_origin(req.headers()) {
        return Err(AppError::Forbidden("asal permintaan tidak sah".into()));
    }
    req.extensions_mut().insert(Claims {
        sub: session.username.clone(),
        role: session.role.clone(),
        exp: session.expires_at.max(0) as usize,
    });
    req.extensions_mut().insert(session);
    Ok(next.run(req).await)
}

/// Khusus pemilik (CCTV, playback NVR, deteksi, alert, bel pintu). Dipasang DI DALAM require_auth.
pub async fn require_owner(req: Request, next: Next) -> Result<Response, AppError> {
    match req.extensions().get::<Session>() {
        Some(s) if s.role == "owner" => Ok(next.run(req).await),
        Some(_) => Err(AppError::Forbidden("hanya pemilik kost yang boleh membuka ini".into())),
        None => Err(AppError::Unauthorized),
    }
}
