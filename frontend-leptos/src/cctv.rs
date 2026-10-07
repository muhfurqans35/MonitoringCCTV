use leptos::*;

use crate::api::{fetch_cctv_channels, fetch_live_player_path, CctvChannel};

#[component]
pub fn CctvView(active_player_path: RwSignal<Option<String>>) -> impl IntoView {
    let channels = create_rw_signal::<Vec<CctvChannel>>(vec![]);
    let error_msg = create_rw_signal(String::new());
    let selected_channel = create_rw_signal(String::new());

    spawn_local(async move {
        if let Ok(list) = fetch_cctv_channels().await {
            channels.set(list);
        }
    });

    view! {
        <div class="cctv-view">
            <div class="topbar">
                <div>
                    <p class="eyebrow">"CCTV"</p>
                    <h2>"Monitor Live CCTV"</h2>
                </div>
            </div>

            <p class="muted small">
                "Streaming langsung dari NVR hanya tersedia di jaringan lokal yang sama dengan perangkat NVR 192.168.1.200. Rekaman lama bisa diputar di menu Playback."
            </p>

            <div class="camera-grid">
                <For
                    each=move || channels.get()
                    key=|c| c.id.clone()
                    children=move |ch: CctvChannel| {
                        let ch_id = ch.id.clone();
                        let ch_label = ch.label.clone();
                        view! {
                            <div class="card camera-card">
                                <div class="camera-status">
                                    <span class="live-dot"></span>
                                    "Online"
                                </div>
                                <div class="camera-name">{ch_label.clone()}</div>
                                <div class="camera-actions">
                                    <button
                                        class="btn"
                                        on:click=move |_| {
                                            let ch_id = ch_id.clone();
                                            let ch_label = ch_label.clone();
                                            selected_channel.set(ch_label);
                                            error_msg.set(String::new());
                                            spawn_local(async move {
                                                match fetch_live_player_path(&ch_id).await {
                                                    Ok(path) => active_player_path.set(Some(path)),
                                                    Err(e) => error_msg.set(e),
                                                }
                                            });
                                        }
                                    >
                                        "Live"
                                    </button>
                                </div>
                            </div>
                        }
                    }
                />
            </div>

            <Show when=move || !error_msg.get().is_empty()>
                <p class="error-text">{move || error_msg.get()}</p>
            </Show>

            <Show when=move || active_player_path.get().is_some()>
                <div class="card player-panel">
                    <div class="panel-header">
                        <h3>{move || {
                            if selected_channel.get().is_empty() {
                                "Player CCTV".to_string()
                            } else {
                                format!("Player CCTV — {}", selected_channel.get())
                            }
                        }}</h3>
                        <span class="panel-tag success">"Live stream"</span>
                    </div>
                    <iframe
                        src=move || active_player_path.get().unwrap_or_default()
                        title="CCTV player"
                    ></iframe>
                </div>
            </Show>
        </div>
    }
}