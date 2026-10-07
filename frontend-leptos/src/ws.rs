use futures::StreamExt;
use gloo_net::websocket::{futures::WebSocket, Message};
use leptos::*;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RoomTelemetryMsg {
    pub room_code: String,
    pub presence: bool,
    #[allow(dead_code)]
    pub distance_cm: f32,
    #[allow(dead_code)]
    pub voltage: f32,
    #[allow(dead_code)]
    pub current: f32,
    pub watt: f32,
    pub kwh_cumulative: f32,
    pub timestamp_unix: i64,
}

/// Tunggu `ms` milidetik (setTimeout sebagai Future).
async fn sleep_ms(ms: i32) {
    let p = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
}

/// Buka koneksi WebSocket ke `/ws` dan set `on_message` tiap kali ada data baru.
/// Dipanggil sekali di root App (lihat app.rs) dengan callback yang update
/// signal RwSignal<HashMap<String, RoomTelemetryMsg>> global.
/// Koneksi yang putus (restart api, jaringan, proxy/Cloudflare) disambung ulang otomatis
/// dengan jeda bertambah 1 -> 30 detik.
pub fn connect_live_updates(on_message: impl Fn(RoomTelemetryMsg) + 'static) {
    let ws_url = build_ws_url();
    // Berhenti menyambung ulang saat dashboard ditutup (logout).
    let alive = std::rc::Rc::new(std::cell::Cell::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.set(false));
    }

    spawn_local(async move {
        let mut backoff_ms = 1000;
        let mut failures = 0u32;
        while alive.get() {
            match WebSocket::open(&ws_url) {
                Ok(ws) => {
                    let (_write, mut read) = ws.split();
                    while let Some(msg) = read.next().await {
                        if !alive.get() {
                            return;
                        }
                        match msg {
                            Ok(Message::Text(text)) => {
                                if let Ok(data) = serde_json::from_str::<RoomTelemetryMsg>(&text) {
                                    backoff_ms = 1000;
                                    failures = 0;
                                    on_message(data);
                                }
                            }
                            Ok(Message::Bytes(_)) => {}
                            Err(e) => {
                                leptos::logging::warn!("websocket terputus: {e:?}");
                                break;
                            }
                        }
                    }
                }
                Err(e) => leptos::logging::warn!("gagal buka websocket: {e:?}"),
            }
            // Gagal terus: mungkin sesi sudah dicabut -> cek; bila ya, muat ulang ke layar login.
            failures += 1;
            if failures % 3 == 0 && crate::api::fetch_me().await.is_err() {
                if let Some(w) = web_sys::window() {
                    let _ = w.location().reload();
                }
                return;
            }
            sleep_ms(backoff_ms).await;
            backoff_ms = (backoff_ms * 2).min(30_000);
        }
    });
}

fn build_ws_url() -> String {
    let location = web_sys::window().unwrap().location();
    let protocol = if location.protocol().unwrap_or_default() == "https:" { "wss" } else { "ws" };
    let host = location.host().unwrap_or_default();
    format!("{protocol}://{host}/ws")
}
