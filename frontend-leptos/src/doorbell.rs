//! Halaman "Bel & Paket" (modul doorbell): status node, kamera pintu (live view), mode, kode
//! pendaftaran penghuni, dan daftar paket yang menunggu diambil.

use std::time::Duration;

use leptos::*;

use crate::api::{self, DoorbellStatus, Package};

/// Selang ambil foto saat live view. Hub men-cache 0,8 dtk, jadi banyak penonton tetap ringan.
const LIVE_INTERVAL: Duration = Duration::from_millis(1000);
/// Live view berhenti otomatis setelah sekian detik. Tanpa ini, tab dashboard yang lupa ditutup
/// membuat kamera pintu (OV5640) tidak pernah tidur -> panas terus-menerus.
const LIVE_MAX_SECS: u32 = 120;

/// Panel kamera pintu: foto diperbarui tiap detik saat "Live" aktif. Komponen terpisah dari
/// kartu status supaya tidak dibuat ulang (dan live view tidak terputus) tiap status di-refresh.
#[component]
fn DoorCameraPanel(status: RwSignal<Option<DoorbellStatus>>) -> impl IntoView {
    let image = create_rw_signal::<Option<String>>(None);
    let error = create_rw_signal(String::new());
    let live = create_rw_signal(true);
    let busy = create_rw_signal(false);
    let updated = create_rw_signal(String::new());
    let live_secs = create_rw_signal(0u32); // lama live berjalan sejak terakhir diaktifkan

    let grab = move || {
        if busy.get_untracked() {
            return; // permintaan sebelumnya belum selesai: jangan menumpuk request
        }
        busy.set(true);
        // try_set: respons bisa tiba setelah halaman ditinggalkan (signal sudah dibuang).
        spawn_local(async move {
            match api::fetch_door_snapshot().await {
                Ok(url) => {
                    let _ = image.try_set(Some(url));
                    let _ = error.try_set(String::new());
                    let d = js_sys::Date::new_0();
                    let _ = updated.try_set(format!("{:02}:{:02}:{:02}", d.get_hours(), d.get_minutes(), d.get_seconds()));
                }
                Err(e) => {
                    let _ = error.try_set(e);
                }
            }
            let _ = busy.try_set(false);
        });
    };
    grab();
    if let Ok(handle) = set_interval_with_handle(
        move || {
            if live.get_untracked() {
                let n = live_secs.get_untracked() + 1;
                if n >= LIVE_MAX_SECS {
                    live.set(false); // jeda otomatis: kamera di node boleh tidur
                    live_secs.set(0);
                } else {
                    live_secs.set(n);
                    grab();
                }
            }
        },
        LIVE_INTERVAL,
    ) {
        on_cleanup(move || handle.clear());
    }

    view! {
        <div class="card panel door-camera" style="margin-bottom: 1rem;">
            <div class="panel-header">
                <h3>"Kamera pintu"</h3>
                {move || {
                    let s = status.get();
                    let ok = s.as_ref().map(|s| s.camera_ok).unwrap_or(false);
                    let label = match &s {
                        Some(s) if s.camera_ok => format!("OK • {}", if s.camera_sensor.is_empty() { "kamera" } else { s.camera_sensor.as_str() }),
                        Some(s) if !s.node_online => "node offline".to_string(),
                        Some(_) => "kamera gagal".to_string(),
                        None => "memuat...".to_string(),
                    };
                    view! { <span class=if ok { "panel-tag success" } else { "panel-tag danger" }>{label}</span> }
                }}
            </div>

            <div class="door-camera-frame">
                {move || match image.get() {
                    Some(src) => view! { <img src=src alt="Kamera pintu" /> }.into_view(),
                    None => view! { <div class="door-camera-empty">{move || if error.get().is_empty() { "Mengambil gambar...".to_string() } else { "Belum ada gambar".to_string() }}</div> }.into_view(),
                }}
                <Show when=move || live.get() && error.get().is_empty() && image.get().is_some()>
                    <span class="door-camera-live"><span class="live-dot"></span>"LIVE"</span>
                </Show>
            </div>

            <Show when=move || !error.get().is_empty()>
                <p class="error-text" style="margin-top: 0.6rem;">{move || format!("Kamera: {}", error.get())}</p>
            </Show>
            {move || status.get().filter(|s| !s.camera_ok && !s.camera_error.is_empty()).map(|s| view! {
                <p class="muted small" style="margin-top: 0.4rem;">{format!("Laporan node: {}", s.camera_error)}</p>
            })}

            <div class="door-camera-bar">
                <button class="btn" on:click=move |_| {
                    live_secs.set(0);
                    live.update(|v| *v = !*v);
                }>
                    {move || if live.get() { "Jeda" } else { "Live" }}
                </button>
                <button class="btn secondary" on:click=move |_| grab()>"Ambil foto sekarang"</button>
                <span class="muted small">{move || {
                    let upd = if updated.get().is_empty() { String::new() } else { format!("diperbarui {}", updated.get()) };
                    if live.get() {
                        format!("{upd} • jeda otomatis dalam {} dtk", LIVE_MAX_SECS.saturating_sub(live_secs.get()))
                    } else {
                        format!("{upd} • live dijeda (kamera boleh istirahat)")
                    }
                }}</span>
            </div>
            {move || status.get().filter(|s| !s.stream_url.is_empty()).map(|s| view! {
                <p class="muted small" style="margin-top: 0.6rem;">
                    {format!("Video MJPEG langsung (hanya dari jaringan lokal yang sama, 1 penonton): {}?token=<DEVICE_TOKEN>", s.stream_url)}
                </p>
            })}
        </div>
    }
}

fn status_label(p: &Package) -> &'static str {
    match p.status.as_str() {
        "waiting" => "Menunggu diambil",
        "verifying" => "Memverifikasi...",
        "disputed" => "Dibantah penghuni",
        _ => "Selesai",
    }
}

#[component]
pub fn DoorbellView() -> impl IntoView {
    let status = create_rw_signal::<Option<DoorbellStatus>>(None);
    let packages = create_rw_signal::<Vec<Package>>(vec![]);
    let message = create_rw_signal(String::new());
    let invite_room = create_rw_signal(String::new());

    let refresh = move || {
        spawn_local(async move {
            match api::fetch_doorbell_status().await {
                Ok(s) => status.set(Some(s)),
                Err(e) => message.set(format!("Status doorbell: {e}")),
            }
            if let Ok(p) = api::fetch_packages().await {
                packages.set(p);
            }
        });
    };
    refresh();
    // Perbarui berkala (belum ada WebSocket khusus doorbell).
    set_interval(refresh, Duration::from_secs(10));

    let on_close = move |id: i64| {
        spawn_local(async move {
            match api::close_package(id).await {
                Ok(r) => message.set(if r.ok { r.message } else { format!("Gagal: {}", r.message) }),
                Err(e) => message.set(e),
            }
            refresh();
        });
    };
    let on_mode = move |mode: String| {
        spawn_local(async move {
            match api::set_doorbell_mode(&mode).await {
                Ok(r) => message.set(if r.ok { r.message } else { format!("Gagal: {}", r.message) }),
                Err(e) => message.set(e),
            }
            refresh();
        });
    };
    let on_calibrate = move |_| {
        spawn_local(async move {
            match api::calibrate_doorbell().await {
                Ok(r) => message.set(if r.ok { r.message } else { format!("Gagal: {}", r.message) }),
                Err(e) => message.set(e),
            }
            refresh();
        });
    };
    let on_invite = move |_| {
        let room = invite_room.get();
        if room.trim().is_empty() {
            message.set("Isi kode kamar dulu.".into());
            return;
        }
        spawn_local(async move {
            match api::create_invite(room.trim()).await {
                Ok(r) if r.ok => message.set(format!(
                    "Kode kamar {}: {} (sekali pakai, {} jam). Penghuni mengirim ke bot Telegram: /start {}",
                    room.trim(),
                    r.code,
                    r.ttl_hours,
                    r.code
                )),
                Ok(r) => message.set(format!("Gagal: {}", r.message)),
                Err(e) => message.set(e),
            }
        });
    };

    view! {
        <div>
            <div class="topbar">
                <div>
                    <p class="eyebrow">"Bel Pintu"</p>
                    <h2>"Bel & Paket"</h2>
                </div>
            </div>

            <Show when=move || !message.get().is_empty()>
                <div class="card" style="margin-bottom: 1rem;">
                    <p>{move || message.get()}</p>
                </div>
            </Show>

            {move || match status.get() {
                None => view! { <p class="empty-state">"Memuat status..."</p> }.into_view(),
                Some(s) => {
                    let modes = s.modes.clone();
                    let current = s.mode.clone();
                    view! {
                        <div class="stat-grid">
                            <div class=if s.node_online { "stat-card highlight" } else { "stat-card warning" }>
                                <span>"Node pintu"</span>
                                <strong>{if s.node_online { "Online" } else { "Offline" }}</strong>
                                <small>{format!("{} • IP {}", s.door_id, if s.node_ip.is_empty() { "-".to_string() } else { s.node_ip.clone() })}</small>
                            </div>
                            <div class="stat-card">
                                <span>"Paket menunggu"</span>
                                <strong>{s.waiting}</strong>
                                <small>{format!("{} sedang diverifikasi", s.verifying)}</small>
                            </div>
                            <div class="stat-card">
                                <span>"Zona paket"</span>
                                <strong>{if s.calibrated { "Terkalibrasi" } else { "Belum" }}</strong>
                                <small>{if s.config_in_sync { "konfigurasi node sinkron" } else { "konfigurasi node belum sinkron" }}</small>
                            </div>
                            <div class=if s.camera_ok { "stat-card highlight" } else { "stat-card warning" }>
                                <span>"Kamera"</span>
                                <strong>{if s.camera_ok { "Online" } else { "Offline" }}</strong>
                                <small>{if s.camera_ok {
                                    if s.camera_sensor.is_empty() { "siap".to_string() } else { s.camera_sensor.clone() }
                                } else if s.camera_error.is_empty() {
                                    "cek node / status MQTT".to_string()
                                } else {
                                    s.camera_error.clone()
                                }}</small>
                            </div>
                        </div>

                        <div class="card panel" style="margin-bottom: 1rem;">
                            <div class="panel-header">
                                <h3>"Mode & kalibrasi"</h3>
                                <span class="panel-tag">{format!("mode: {}", current.clone())}</span>
                            </div>
                            <div style="display: flex; gap: 0.5rem; flex-wrap: wrap;">
                                {modes.into_iter().map(|m| {
                                    let active = m == current;
                                    let m2 = m.clone();
                                    view! {
                                        <button
                                            class=if active { "btn" } else { "btn secondary" }
                                            on:click=move |_| on_mode(m2.clone())
                                        >
                                            {m}
                                        </button>
                                    }
                                }).collect_view()}
                                <button class="btn secondary" on:click=on_calibrate>"Kalibrasi zona (saat kosong)"</button>
                            </div>
                        </div>
                    }.into_view()
                }
            }}

            <DoorCameraPanel status=status />

            <div class="card panel" style="margin-bottom: 1rem;">
                <div class="panel-header">
                    <h3>"Daftarkan penghuni ke notifikasi paket"</h3>
                </div>
                <div style="display: flex; gap: 0.5rem; flex-wrap: wrap;">
                    <input
                        type="text"
                        placeholder="Kode kamar, mis. K01"
                        on:input=move |ev| invite_room.set(event_target_value(&ev))
                    />
                    <button class="btn" on:click=on_invite>"Buat kode"</button>
                </div>
            </div>

            <div class="card panel">
                <div class="panel-header">
                    <h3>"Paket yang perlu tindakan"</h3>
                    <span class="panel-tag">{move || format!("{} item", packages.get().len())}</span>
                </div>
                {move || {
                    let list = packages.get();
                    if list.is_empty() {
                        view! { <p class="empty-state">"Tidak ada paket menunggu."</p> }.into_view()
                    } else {
                        list.into_iter().map(|p| {
                            let id = p.id;
                            view! {
                                <div class="event-card" style="margin-bottom: 0.75rem;">
                                    <div class="event-header">
                                        <div>
                                            <div class="event-label">"Kamar"</div>
                                            <strong>{p.room_code.clone()}</strong>
                                        </div>
                                        {if p.verified {
                                            view! { <span class="badge presence-yes">"Terverifikasi kamera"</span> }
                                        } else {
                                            view! { <span class="badge presence-no">"Belum terverifikasi"</span> }
                                        }}
                                    </div>
                                    <div class="event-body">
                                        <p>{format!("{}{}", status_label(&p), if p.note.is_empty() { String::new() } else { format!(" • {}", p.note) })}</p>
                                    </div>
                                    <div class="event-meta">
                                        <span>{format!("{} UTC", p.created_at)}</span>
                                        <button class="btn small" on:click=move |_| on_close(id)>"Sudah diambil"</button>
                                    </div>
                                </div>
                            }
                        }).collect_view()
                    }
                }}
            </div>
        </div>
    }
}
