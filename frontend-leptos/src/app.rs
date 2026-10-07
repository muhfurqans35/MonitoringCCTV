use std::collections::HashMap;
use std::time::Duration;

use leptos::*;

use crate::account::AccountView;
use crate::api::{self, AlertEvent, Me, Room};
use crate::cctv::CctvView;
use crate::components::{AlertFeed, RoomCard};
use crate::detections::DetectionGallery;
use crate::doorbell::DoorbellView;
use crate::playback::PlaybackView;
use crate::ws::{connect_live_updates, RoomTelemetryMsg};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Ringkasan,
    Deteksi,
    Cctv,
    Playback,
    Paket,
    Akun,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Ringkasan => "Ringkasan",
            Tab::Deteksi => "Deteksi kamera",
            Tab::Cctv => "Live CCTV",
            Tab::Playback => "Playback rekaman",
            Tab::Paket => "Bel & paket",
            Tab::Akun => "Akun & keamanan",
        }
    }
}

#[component]
pub fn App() -> impl IntoView {
    // Sesi ada di cookie HttpOnly: tanya server siapa yang login.
    let me = create_rw_signal::<Option<Me>>(None);
    let checked = create_rw_signal(false);
    spawn_local(async move {
        if let Ok(m) = api::fetch_me().await {
            me.set(Some(m));
        }
        checked.set(true);
    });

    view! {
        {move || {
            if !checked.get() {
                view! { <div class="login-shell"><p class="muted">"Memuat…"</p></div> }.into_view()
            } else if me.with(|m| m.is_some()) {
                view! { <DashboardView me=me /> }.into_view()
            } else {
                view! { <LoginView me=me /> }.into_view()
            }
        }}
    }
}

#[component]
fn LoginView(me: RwSignal<Option<Me>>) -> impl IntoView {
    let username = create_rw_signal(String::new());
    let password = create_rw_signal(String::new());
    let error = create_rw_signal(String::new());
    let busy = create_rw_signal(false);

    let submit = move || {
        let (u, p) = (username.get(), password.get());
        busy.set(true);
        spawn_local(async move {
            match api::login(&u, &p).await {
                Ok(m) => me.set(Some(m)),
                Err(e) => {
                    let _ = error.try_set(e);
                }
            }
            let _ = busy.try_set(false);
        });
    };

    view! {
        <div class="login-shell">
            <form class="login-box" on:submit=move |ev| { ev.prevent_default(); submit(); }>
                <div class="brand-tag" aria-hidden="true"><span class="key-hole"></span>"SK"</div>
                <h1>"Smart Kost"</h1>
                <p class="muted">"Masuk untuk memantau kamar, kamera, dan paket."</p>
                <label>
                    <span>"Username"</span>
                    <input type="text" autocomplete="username" on:input=move |ev| username.set(event_target_value(&ev)) />
                </label>
                <label>
                    <span>"Password"</span>
                    <input type="password" autocomplete="current-password" on:input=move |ev| password.set(event_target_value(&ev)) />
                </label>
                <button class="btn full" type="submit" disabled=move || busy.get()>
                    {move || if busy.get() { "Memeriksa…" } else { "Masuk" }}
                </button>
                <Show when=move || !error.get().is_empty()>
                    <p class="error-text">{move || error.get()}</p>
                </Show>
            </form>
        </div>
    }
}

#[component]
fn DashboardView(me: RwSignal<Option<Me>>) -> impl IntoView {
    let owner = move || me.with(|m| m.as_ref().map(|m| m.is_owner()).unwrap_or(false));
    let rooms = create_rw_signal::<Vec<Room>>(vec![]);
    let alerts = create_rw_signal::<Vec<AlertEvent>>(vec![]);
    let live_data = create_rw_signal::<HashMap<String, RoomTelemetryMsg>>(HashMap::new());
    let tab = create_rw_signal(Tab::Ringkasan);
    let active_player_path = create_rw_signal::<Option<String>>(None);

    spawn_local(async move {
        if let Ok(r) = api::fetch_rooms().await {
            rooms.set(r);
        }
    });

    // Alert dimuat ulang berkala: dulu hanya sekali saat halaman dibuka, sehingga deteksi
    // kamera yang baru masuk tidak pernah tampil sampai halaman di-refresh.
    let load_alerts = move || {
        // Alert berisi kejadian kamera: hanya untuk pemilik.
        if !me.with_untracked(|m| m.as_ref().map(|m| m.is_owner()).unwrap_or(false)) {
            return;
        }
        spawn_local(async move {
            if let Ok(a) = api::fetch_alerts().await {
                if alerts.try_get_untracked().map(|cur| cur != a).unwrap_or(false) {
                    let _ = alerts.try_set(a);
                }
            }
        });
    };
    load_alerts();
    if let Ok(h) = set_interval_with_handle(load_alerts, Duration::from_secs(10)) {
        on_cleanup(move || h.clear());
    }
    let on_resolve = Callback::new(move |id: i64| {
        spawn_local(async move {
            if api::resolve_alert(id).await.is_ok() {
                load_alerts();
            }
        });
    });

    connect_live_updates(move |data| {
        live_data.update(|map| {
            map.insert(data.room_code.clone(), data);
        });
    });

    let on_logout = move |_| {
        spawn_local(async move {
            api::logout().await;
            me.set(None);
        });
    };

    let open_count = move || alerts.get().iter().filter(|a| !a.resolved).count();
    let occupied = move || {
        let live = live_data.get();
        rooms.get().iter().filter(|r| live.get(&r.code).map(|s| s.presence).unwrap_or(false)).count()
    };
    let camera_alerts = Signal::derive(move || {
        alerts
            .get()
            .into_iter()
            .filter(|a| !a.source.starts_with("room_") && !a.source.starts_with("door_"))
            .collect::<Vec<_>>()
    });
    let all_alerts = Signal::derive(move || alerts.get());

    let nav = move |t: Tab, label: &'static str| {
        view! {
            <button
                class="nav-item"
                class:active=move || tab.get() == t
                aria-current=move || if tab.get() == t { "page" } else { "false" }
                on:click=move |_| tab.set(t)
            >
                <span class="nav-mark" aria-hidden="true"></span>
                {label}
                {move || (t == Tab::Ringkasan && open_count() > 0).then(|| view! { <span class="nav-count">{open_count()}</span> })}
            </button>
        }
    };

    view! {
        <div class="app-shell">
            <aside class="sidebar">
                <div class="brand">
                    <div class="brand-tag" aria-hidden="true"><span class="key-hole"></span>"SK"</div>
                    <div>
                        <strong>"Smart Kost"</strong>
                        <small>"Pemantauan kost"</small>
                    </div>
                </div>
                <nav class="nav-menu" aria-label="Menu utama">
                    {nav(Tab::Ringkasan, "Ringkasan")}
                    // Kamera, rekaman, dan bel hanya untuk pemilik (server juga menolak peran lain).
                    {move || owner().then(|| view! {
                        {nav(Tab::Deteksi, "Deteksi kamera")}
                        {nav(Tab::Cctv, "Live CCTV")}
                        {nav(Tab::Playback, "Playback")}
                        {nav(Tab::Paket, "Bel & paket")}
                    })}
                    {nav(Tab::Akun, "Akun")}
                </nav>
                <button class="logout-btn" on:click=on_logout>"Keluar"</button>
            </aside>

            <main class="content">
                <header class="page-head">
                    <h1>{move || tab.get().title()}</h1>
                </header>

                {move || match tab.get() {
                    Tab::Ringkasan => view! {
                        <p class="lede">
                            {move || {
                                let total = rooms.get().len();
                                let open = open_count();
                                let kamar = if total == 0 {
                                    "Belum ada kamar terdaftar".to_string()
                                } else {
                                    format!("{} dari {} kamar sedang berpenghuni", occupied(), total)
                                };
                                let cek = match open {
                                    0 => "tidak ada yang perlu dicek.".to_string(),
                                    n => format!("{n} hal perlu dicek."),
                                };
                                format!("{kamar}, {cek}")
                            }}
                        </p>

                        <section class="key-board" aria-label="Kamar">
                            <div class="key-rail" aria-hidden="true"></div>
                            <div class="key-grid">
                                <For
                                    each=move || rooms.get()
                                    key=|r| r.code.clone()
                                    children=move |room: Room| {
                                        let code = room.code.clone();
                                        let live = create_memo(move |_| live_data.get().get(&code).cloned());
                                        let rc = room.code.clone();
                                        view! { {move || view! { <RoomCard room_code=rc.clone() live=live.get() /> }} }
                                    }
                                />
                            </div>
                            <Show when=move || rooms.get().is_empty()>
                                <p class="empty-state">"Kamar belum terdaftar. Tambahkan di tabel rooms agar tampil di sini."</p>
                            </Show>
                        </section>

                        // Kamera & alert hanya untuk pemilik.
                        <Show when=owner>
                        <div class="two-col">
                            <section class="panel">
                                <div class="panel-header">
                                    <h2>"Terlihat kamera"</h2>
                                    <button class="link-btn" on:click=move |_| tab.set(Tab::Deteksi)>"Semua deteksi"</button>
                                </div>
                                <DetectionGallery limit=12 compact=true />
                            </section>
                            <section class="panel">
                                <div class="panel-header">
                                    <h2>"Perlu dicek"</h2>
                                    <span class="panel-tag">{move || format!("{} terbuka", open_count())}</span>
                                </div>
                                <AlertFeed alerts=all_alerts on_resolve=on_resolve only_open=true limit=8 />
                            </section>
                        </div>
                        </Show>
                    }.into_view(),

                    Tab::Deteksi => view! {
                        <p class="lede">"Foto orang dan kendaraan yang tertangkap kamera CCTV, dengan kotak hasil deteksi. Ketuk foto untuk memperbesar."</p>
                        <section class="panel">
                            <DetectionGallery limit=60 />
                        </section>
                        <section class="panel">
                            <div class="panel-header">
                                <h2>"Riwayat alert kamera"</h2>
                            </div>
                            <AlertFeed alerts=camera_alerts on_resolve=on_resolve limit=40 />
                        </section>
                    }.into_view(),

                    Tab::Cctv => view! { <CctvView active_player_path=active_player_path /> }.into_view(),

                    Tab::Playback => view! { <PlaybackView /> }.into_view(),

                    Tab::Paket => view! { <DoorbellView /> }.into_view(),

                    Tab::Akun => view! { <AccountView me=me /> }.into_view(),
                }}
            </main>
        </div>
    }
}
