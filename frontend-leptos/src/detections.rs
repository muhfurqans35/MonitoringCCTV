//! Galeri deteksi CCTV: foto dari service vision (data/vision) dengan kotak deteksi (bbox)
//! digambar di atas foto. Dipakai di halaman "Deteksi" (lengkap) dan Ringkasan (strip terbaru).

use std::time::Duration;

use leptos::*;

use crate::api::{self, DetectionItem};

/// Nama yang dimengerti pemilik kost, bukan nama kelas model.
pub fn label_id(label: &str) -> &'static str {
    match label {
        "person" => "Orang",
        "vehicle" => "Kendaraan",
        "motion" => "Gerakan",
        _ => "Objek",
    }
}

fn class_id(class: &str) -> String {
    match class {
        "person" => "orang".into(),
        "car" => "mobil".into(),
        "motorcycle" => "motor".into(),
        "bicycle" => "sepeda".into(),
        "bus" => "bus".into(),
        "truck" => "truk".into(),
        other => other.into(),
    }
}

/// "2026-10-06T05:27:01.123Z" -> jam & tanggal lokal browser.
fn local_time(ts: &str) -> (String, String) {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(ts));
    if d.get_time().is_nan() {
        return (ts.to_string(), String::new());
    }
    let jam = format!("{:02}.{:02}", d.get_hours(), d.get_minutes());
    const BULAN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "Mei", "Jun", "Jul", "Agu", "Sep", "Okt", "Nov", "Des"];
    let tgl = format!("{} {}", d.get_date(), BULAN[(d.get_month() as usize).min(11)]);
    (jam, tgl)
}

/// Satu foto deteksi; gambar diambil saat kartu dibuat (data URL, karena butuh JWT).
#[component]
fn DetectionCard(item: DetectionItem, on_open: Callback<DetectionItem>) -> impl IntoView {
    let src = create_rw_signal::<Option<String>>(None);
    let failed = create_rw_signal(false);
    let path = item.image.clone();
    spawn_local(async move {
        match api::fetch_detection_image(&path).await {
            Ok(u) => {
                let _ = src.try_set(Some(u));
            }
            Err(_) => {
                let _ = failed.try_set(true);
            }
        }
    });

    let (jam, tgl) = local_time(&item.timestamp);
    let cam = if item.camera_label.is_empty() { item.camera_id.clone() } else { item.camera_label.clone() };
    let kind = item.label.clone();
    let pct = (item.confidence * 100.0).round() as i32;
    let open_item = item.clone();

    view! {
        <button class="det-card" data-kind=kind.clone() on:click=move |_| on_open.call(open_item.clone())>
            <div class="det-photo">
                {move || match src.get() {
                    Some(u) => view! { <img src=u alt=format!("Deteksi di {}", cam.clone()) loading="lazy" /> }.into_view(),
                    None => view! {
                        <div class="det-photo-empty">{move || if failed.get() { "Foto tidak tersedia" } else { "Memuat foto…" }}</div>
                    }.into_view(),
                }}
                <BoxOverlay item=item.clone() />
            </div>
            <div class="det-caption">
                <span class="det-kind">{label_id(&kind)}{if pct > 0 { format!(" {pct}%") } else { String::new() }}</span>
                <span class="det-where">{if item.camera_label.is_empty() { item.camera_id.clone() } else { item.camera_label.clone() }}</span>
                <time class="det-time">{jam}<small>{tgl}</small></time>
            </div>
        </button>
    }
}

/// Kotak deteksi dalam persen ukuran gambar, jadi ikut menyesuaikan lebar kartu.
#[component]
fn BoxOverlay(item: DetectionItem) -> impl IntoView {
    let (w, h) = (item.image_width.max(1) as f32, item.image_height.max(1) as f32);
    if item.image_width == 0 || item.image_height == 0 {
        return view! { <span></span> }.into_view();
    }
    item.detections
        .into_iter()
        .map(|d| {
            let [x1, y1, x2, y2] = d.bbox;
            let style = format!(
                "left:{:.2}%;top:{:.2}%;width:{:.2}%;height:{:.2}%",
                x1 / w * 100.0,
                y1 / h * 100.0,
                (x2 - x1).max(0.0) / w * 100.0,
                (y2 - y1).max(0.0) / h * 100.0
            );
            let tag = format!("{} {:.0}%", class_id(&d.class), d.confidence * 100.0);
            view! { <span class="det-box" data-kind=d.label.clone() style=style><em>{tag}</em></span> }
        })
        .collect_view()
}

/// Galeri lengkap (halaman Deteksi) atau ringkas (`compact`, untuk Ringkasan).
#[component]
pub fn DetectionGallery(
    #[prop(default = 48)] limit: usize,
    #[prop(default = false)] compact: bool,
) -> impl IntoView {
    let items = create_rw_signal::<Vec<DetectionItem>>(vec![]);
    let error = create_rw_signal(String::new());
    let loaded = create_rw_signal(false);
    let camera = create_rw_signal(String::new());
    let kind = create_rw_signal(String::from("all"));
    let opened = create_rw_signal::<Option<DetectionItem>>(None);
    let big_src = create_rw_signal::<Option<String>>(None);

    let load = move || {
        let cam = camera.get_untracked();
        spawn_local(async move {
            match api::fetch_detections(limit, Some(&cam)).await {
                Ok(list) => {
                    // Hanya ganti bila ada perubahan: kartu lama tidak dimuat ulang tanpa perlu.
                    if items.try_get_untracked().map(|cur| cur != list).unwrap_or(false) {
                        let _ = items.try_set(list);
                    }
                    let _ = error.try_set(String::new());
                }
                Err(e) => {
                    let _ = error.try_set(e);
                }
            }
            let _ = loaded.try_set(true);
        });
    };
    load();
    if let Ok(h) = set_interval_with_handle(load, Duration::from_secs(15)) {
        on_cleanup(move || h.clear());
    }

    let on_open = Callback::new(move |it: DetectionItem| {
        big_src.set(None);
        let path = it.image.clone();
        opened.set(Some(it));
        spawn_local(async move {
            if let Ok(u) = api::fetch_detection_image(&path).await {
                let _ = big_src.try_set(Some(u));
            }
        });
    });

    let cameras = move || {
        let mut v: Vec<(String, String)> = items
            .get()
            .iter()
            .map(|i| (i.camera_id.clone(), if i.camera_label.is_empty() { i.camera_id.clone() } else { i.camera_label.clone() }))
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let visible = move || {
        let k = kind.get();
        items.get().into_iter().filter(|i| k == "all" || i.label == k).collect::<Vec<_>>()
    };

    view! {
        <section class=if compact { "gallery compact" } else { "gallery" }>
            <Show when=move || !compact>
                <div class="filter-row" role="toolbar" aria-label="Saring deteksi">
                    {[("all", "Semua"), ("person", "Orang"), ("vehicle", "Kendaraan")]
                        .into_iter()
                        .map(|(k, t)| view! {
                            <button class="chip" class:on=move || kind.get() == k on:click=move |_| kind.set(k.to_string())>{t}</button>
                        })
                        .collect_view()}
                    <span class="filter-gap"></span>
                    <select
                        aria-label="Kamera"
                        on:change=move |ev| {
                            camera.set(event_target_value(&ev));
                            items.set(vec![]);
                            load();
                        }
                    >
                        <option value="">"Semua kamera"</option>
                        {move || cameras().into_iter().map(|(id, lbl)| view! { <option value=id>{lbl}</option> }).collect_view()}
                    </select>
                </div>
            </Show>

            <Show when=move || !error.get().is_empty()>
                <p class="error-text">{move || format!("Galeri deteksi tidak bisa dimuat: {}", error.get())}</p>
            </Show>

            {move || {
                let list = visible();
                if list.is_empty() {
                    view! {
                        <div class="empty-state">
                            {move || if !loaded.get() {
                                "Memuat deteksi…".to_string()
                            } else if compact {
                                "Belum ada orang atau kendaraan terdeteksi hari ini.".to_string()
                            } else {
                                "Belum ada deteksi. Foto muncul di sini saat kamera menangkap orang atau kendaraan.".to_string()
                            }}
                        </div>
                    }.into_view()
                } else {
                    let list = if compact { list.into_iter().take(4).collect::<Vec<_>>() } else { list };
                    view! {
                        <div class="det-grid">
                            <For
                                each=move || list.clone()
                                key=|i| i.image.clone()
                                children=move |i: DetectionItem| view! { <DetectionCard item=i on_open=on_open /> }
                            />
                        </div>
                    }.into_view()
                }
            }}

            // Tampilan besar
            {move || opened.get().map(|it| {
                let (jam, tgl) = local_time(&it.timestamp);
                let cam = if it.camera_label.is_empty() { it.camera_id.clone() } else { it.camera_label.clone() };
                let objs = it.detections.iter().map(|d| format!("{} {:.0}%", class_id(&d.class), d.confidence * 100.0)).collect::<Vec<_>>().join(", ");
                view! {
                    <div class="lightbox" role="dialog" aria-modal="true" on:click=move |_| opened.set(None)>
                        <figure on:click=|ev| ev.stop_propagation()>
                            <div class="det-photo large">
                                {move || match big_src.get() {
                                    Some(u) => view! { <img src=u alt=format!("Deteksi di {}", cam.clone()) /> }.into_view(),
                                    None => view! { <div class="det-photo-empty">"Memuat foto…"</div> }.into_view(),
                                }}
                                <BoxOverlay item=it.clone() />
                            </div>
                            <figcaption>
                                <strong>{format!("{} di {}", label_id(&it.label), if it.camera_label.is_empty() { it.camera_id.clone() } else { it.camera_label.clone() })}</strong>
                                <span>{format!("{tgl}, pukul {jam}")}</span>
                                <span>{objs}</span>
                                <button class="btn secondary" on:click=move |_| opened.set(None)>"Tutup"</button>
                            </figcaption>
                        </figure>
                    </div>
                }
            })}
        </section>
    }
}
