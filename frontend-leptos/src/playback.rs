//! Halaman Playback: rekaman NVR langsung dari NVR + event motion NVR + deteksi vision.
//!
//!   garis waktu 24 jam  -> klik = pilih jam (garis waktu jam itu muncul di bawahnya)
//!   garis waktu 1 jam   -> klik = putar dari titik itu
//!   daftar gerakan NVR  -> klik = putar gerakan itu (ditandai bila vision melihat orang/kendaraan)
//!
//! Semua jam di halaman ini = jam NVR. Klip diambil server dari NVR (±separuh durasi klip).

use std::time::Duration;

use leptos::*;
use wasm_bindgen::JsCast;

use crate::api::{self, CctvChannel, DetectionItem, NvrRecording, NvrRecordings};

const DAY: u32 = 86_400;

fn today() -> String {
    let d = js_sys::Date::new_0();
    format!("{:04}-{:02}-{:02}", d.get_full_year(), d.get_month() + 1, d.get_date())
}

/// "2026-10-06T07:56:23" -> detik sejak tengah malam.
fn sod(s: &str) -> u32 {
    let p: Vec<u32> = s.get(11..19).unwrap_or("").split(':').filter_map(|x| x.parse().ok()).collect();
    if p.len() == 3 {
        p[0] * 3600 + p[1] * 60 + p[2]
    } else {
        0
    }
}

fn hms(s: u32) -> String {
    format!("{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

fn hm(s: u32) -> String {
    format!("{:02}.{:02}", s / 3600, s % 3600 / 60)
}

/// Waktu deteksi (UTC ISO) -> detik sejak tengah malam waktu lokal, bila di tanggal `date`.
fn det_sod(ts: &str, date: &str) -> Option<u32> {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(ts));
    if d.get_time().is_nan() {
        return None;
    }
    let ds = format!("{:04}-{:02}-{:02}", d.get_full_year(), d.get_month() + 1, d.get_date());
    (ds == date).then(|| d.get_hours() * 3600 + d.get_minutes() * 60 + d.get_seconds())
}

fn rec_span(r: &NvrRecording) -> (u32, u32) {
    let b = sod(&r.begin);
    let e = sod(&r.end);
    // file yang melewati tengah malam berakhir "00:00:00" hari berikutnya
    (b, if e <= b && r.seconds > 0 { (b + r.seconds as u32).min(DAY) } else { e })
}

fn object_name(label: &str) -> &'static str {
    match label {
        "person" => "Orang",
        "vehicle" => "Kendaraan",
        _ => "Objek",
    }
}

/// Posisi klik pada garis waktu sebagai pecahan 0..1.
fn click_fraction(ev: &web_sys::MouseEvent) -> Option<f64> {
    let el = ev.current_target()?.dyn_into::<web_sys::Element>().ok()?;
    let r = el.get_bounding_client_rect();
    if r.width() <= 0.0 {
        return None;
    }
    Some(((ev.client_x() as f64 - r.left()) / r.width()).clamp(0.0, 0.9999))
}

#[derive(Clone, PartialEq)]
struct MotionEv {
    begin: u32,
    end: u32,
    kind: String,
    /// "person"/"vehicle" yang dilihat vision di sekitar waktu gerakan ini.
    objects: Vec<String>,
}

/// Filter jenis gerakan di halaman Playback.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KindFilter {
    All,
    Person,
    Vehicle,
    MotionOnly,
    Alarm,
}

impl KindFilter {
    const ALL: [KindFilter; 5] = [KindFilter::All, KindFilter::Person, KindFilter::Vehicle, KindFilter::MotionOnly, KindFilter::Alarm];

    fn label(self) -> &'static str {
        match self {
            KindFilter::All => "Semua",
            KindFilter::Person => "Orang",
            KindFilter::Vehicle => "Kendaraan",
            KindFilter::MotionOnly => "Gerakan saja",
            KindFilter::Alarm => "Alarm",
        }
    }

    fn matches(self, e: &MotionEv) -> bool {
        let has = |k: &str| e.objects.iter().any(|o| o == k);
        match self {
            KindFilter::All => true,
            KindFilter::Person => has("person"),
            KindFilter::Vehicle => has("vehicle"),
            KindFilter::MotionOnly => e.objects.is_empty() && e.kind == "motion",
            KindFilter::Alarm => e.kind == "alarm",
        }
    }

    /// Tanda deteksi vision yang relevan untuk filter ini.
    fn shows_mark(self, label: &str) -> bool {
        match self {
            KindFilter::Person => label == "person",
            KindFilter::Vehicle => label == "vehicle",
            KindFilter::MotionOnly | KindFilter::Alarm => false,
            KindFilter::All => true,
        }
    }
}

/// Pilihan durasi minimum gerakan (detik).
const MIN_DURATIONS: [(u32, &str); 5] = [(0, "Semua durasi"), (10, "≥ 10 dtk"), (30, "≥ 30 dtk"), (60, "≥ 1 menit"), (300, "≥ 5 menit")];

/// Sorotan filter pada garis waktu: awal gerakan yang lolos filter + filter tanda deteksi.
struct Highlight<'a> {
    begins: &'a std::collections::HashSet<u32>,
    kind: KindFilter,
}

/// Segmen rekaman + tanda deteksi dalam jendela [from, to) detik. Bila `hl` ada, gerakan yang
/// tidak lolos filter diredupkan dan tanda deteksi disaring.
fn track(recs: &[NvrRecording], marks: &[(u32, String)], from: u32, to: u32, hl: Option<&Highlight>) -> View {
    let span = (to - from) as f32;
    let pct = |s: u32| (s.saturating_sub(from).min(to - from)) as f32 / span * 100.0;
    let segs = recs
        .iter()
        .filter_map(|r| {
            let (b, e) = rec_span(r);
            if e <= from || b >= to {
                return None;
            }
            let left = pct(b.max(from));
            let width = (pct(e.min(to)) - left).max(0.12);
            let event = r.kind == "motion" || r.kind == "alarm";
            let dim = hl.map(|h| event && !h.begins.contains(&b)).unwrap_or(false);
            Some(view! {
                <span class="tl-seg" class:dim=dim data-kind=r.kind.clone() style=format!("left:{left:.3}%;width:{width:.3}%")></span>
            })
        })
        .collect_view();
    let dets = marks
        .iter()
        .filter(|(s, _)| *s >= from && *s < to)
        .filter(|(_, l)| hl.map(|h| h.kind.shows_mark(l)).unwrap_or(true))
        .map(|(s, l)| view! { <span class="tl-det" data-kind=l.clone() style=format!("left:{:.3}%", pct(*s))></span> })
        .collect_view();
    view! { {segs}{dets} }.into_view()
}

#[component]
pub fn PlaybackView() -> impl IntoView {
    let channels = create_rw_signal(Vec::<CctvChannel>::new());
    let camera = create_rw_signal(String::new());
    let date = create_rw_signal(today());
    let recs = create_rw_signal::<Option<NvrRecordings>>(None);
    let dets = create_rw_signal(Vec::<DetectionItem>::new());
    let loading = create_rw_signal(false);
    let error = create_rw_signal(String::new());
    let kind_filter = create_rw_signal(KindFilter::All);
    let min_secs = create_rw_signal(0u32);
    let focus_hour = create_rw_signal::<Option<u32>>(None);
    let shown = create_rw_signal(40usize);

    let clip_len = create_rw_signal(30u32);
    let clip = create_rw_signal::<Option<(u32, u32)>>(None); // (mulai, detik)
    let clip_url = create_rw_signal::<Option<String>>(None);
    let clip_err = create_rw_signal(String::new());
    let clip_busy = create_rw_signal(false);
    let busy_secs = create_rw_signal(0u32);
    let request = create_rw_signal(0u32);
    let list_request = create_rw_signal(0u32);

    spawn_local(async move {
        if let Ok(list) = api::fetch_cctv_channels().await {
            if camera.get_untracked().is_empty() {
                if let Some(c) = list.iter().find(|c| c.id == "gate_right").or(list.first()) {
                    let _ = camera.try_set(c.id.clone());
                }
            }
            let _ = channels.try_set(list);
        }
    });

    let load = move || {
        let (cam, d) = (camera.get_untracked(), date.get_untracked());
        if cam.is_empty() || d.is_empty() {
            return;
        }
        loading.set(true);
        error.set(String::new());
        recs.set(None);
        dets.set(vec![]);
        focus_hour.set(None);
        shown.set(40);
        let id = list_request.get_untracked() + 1;
        list_request.set(id);
        // Abaikan jawaban lama bila kamera/tanggal sudah diganti sebelum NVR menjawab.
        let current = move || list_request.try_get_untracked() == Some(id);
        spawn_local(async move {
            let r = api::fetch_nvr_recordings(&cam, &d).await;
            if !current() {
                return;
            }
            match r {
                Ok(r) => {
                    let _ = recs.try_set(Some(r));
                }
                Err(e) => {
                    let _ = error.try_set(e);
                }
            }
            let _ = loading.try_set(false);
            if let Ok(v) = api::fetch_detections_day(&cam, &d).await {
                if current() {
                    let _ = dets.try_set(v);
                }
            }
        });
    };
    create_effect(move |_| {
        camera.track();
        date.track();
        load();
    });

    // Penghitung detik selama klip diambil dari NVR.
    if let Ok(h) = set_interval_with_handle(
        move || {
            if clip_busy.get_untracked() {
                busy_secs.update(|s| *s += 1);
            }
        },
        Duration::from_secs(1),
    ) {
        on_cleanup(move || h.clear());
    }
    on_cleanup(move || {
        if let Some(u) = clip_url.get_untracked() {
            api::revoke_blob_url(&u);
        }
    });

    let play = move |start: u32, secs: u32| {
        let (cam, d) = (camera.get_untracked(), date.get_untracked());
        let start = start.min(DAY - 1);
        clip.set(Some((start, secs)));
        clip_err.set(String::new());
        clip_busy.set(true);
        busy_secs.set(0);
        let id = request.get_untracked() + 1;
        request.set(id);
        spawn_local(async move {
            let r = api::fetch_nvr_clip(&cam, &format!("{d}T{}", hms(start)), secs).await;
            if request.try_get_untracked() != Some(id) {
                if let Ok(u) = r {
                    api::revoke_blob_url(&u); // sudah diganti permintaan lain
                }
                return;
            }
            match r {
                Ok(u) => {
                    if let Some(old) = clip_url.get_untracked() {
                        api::revoke_blob_url(&old);
                    }
                    let _ = clip_url.try_set(Some(u));
                }
                Err(e) => {
                    let _ = clip_url.try_set(None);
                    let _ = clip_err.try_set(e);
                }
            }
            let _ = clip_busy.try_set(false);
        });
    };

    let marks = create_memo(move |_| {
        let d = date.get();
        dets.get().iter().filter_map(|x| det_sod(&x.timestamp, &d).map(|s| (s, x.label.clone()))).collect::<Vec<_>>()
    });
    let events = create_memo(move |_| {
        let m = marks.get();
        recs.get()
            .map(|r| {
                r.recordings
                    .iter()
                    .filter(|x| x.kind == "motion" || x.kind == "alarm")
                    .map(|x| {
                        let (b, e) = rec_span(x);
                        let mut objects: Vec<String> =
                            m.iter().filter(|(s, _)| *s + 5 >= b && *s <= e + 5).map(|(_, l)| l.clone()).collect();
                        objects.sort();
                        objects.dedup();
                        MotionEv { begin: b, end: e, kind: x.kind.clone(), objects }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    let rec_list = move || recs.get().map(|r| r.recordings).unwrap_or_default();
    let filtered = create_memo(move |_| {
        let (k, min) = (kind_filter.get(), min_secs.get());
        events.get().into_iter().filter(|e| k.matches(e) && e.end.saturating_sub(e.begin) >= min).collect::<Vec<_>>()
    });
    let filter_active = move || kind_filter.get() != KindFilter::All || min_secs.get() > 0;
    let highlight_begins = create_memo(move |_| filtered.get().iter().map(|e| e.begin).collect::<std::collections::HashSet<u32>>());
    // Garis waktu memakai sorotan hanya bila ada filter aktif.
    let draw = move |from: u32, to: u32| {
        let begins = highlight_begins.get();
        let hl = Highlight { begins: &begins, kind: kind_filter.get() };
        track(&rec_list(), &marks.get(), from, to, filter_active().then_some(&hl))
    };
    let visible = move || {
        let h = focus_hour.get();
        let mut v: Vec<MotionEv> = filtered
            .get()
            .into_iter()
            .filter(|e| h.map(|h| e.begin / 3600 == h || e.end / 3600 == h).unwrap_or(true))
            .collect();
        v.reverse();
        v
    };

    view! {
        <p class="lede">"Putar rekaman CCTV langsung dari NVR. Pilih jam di garis waktu atau ketuk gerakan yang dicatat NVR."</p>

        <section class="panel">
            <div class="filter-row pb-controls">
                <label class="field">
                    <span>"Kamera"</span>
                    <select on:change=move |ev| camera.set(event_target_value(&ev))>
                        {move || channels.get().into_iter().map(|c| {
                            let id = c.id.clone();
                            view! { <option value=c.id.clone() selected=move || camera.get() == id>{c.label.clone()}</option> }
                        }).collect_view()}
                    </select>
                </label>
                <label class="field">
                    <span>"Tanggal"</span>
                    <input type="date" max=today() prop:value=move || date.get() on:change=move |ev| date.set(event_target_value(&ev)) />
                </label>
                <span class="filter-gap"></span>
                <button class="btn secondary small" on:click=move |_| load() disabled=move || loading.get()>"Muat ulang"</button>
            </div>

            <Show when=move || !error.get().is_empty()>
                <p class="error-text">{move || format!("Daftar rekaman tidak bisa diambil: {}", error.get())}</p>
            </Show>

            <div class="tl-head">
                <h2>"Garis waktu"</h2>
                <span class="muted small">
                    {move || if loading.get() {
                        "Mengambil daftar rekaman dari NVR…".to_string()
                    } else {
                        let ev = events.get();
                        let with = ev.iter().filter(|e| !e.objects.is_empty()).count();
                        if filter_active() {
                            format!("{} dari {} gerakan cocok dengan filter", filtered.get().len(), ev.len())
                        } else {
                            format!("{} gerakan dicatat NVR, {} dengan orang/kendaraan", ev.len(), with)
                        }
                    }}
                </span>
            </div>
            <div
                class="tl"
                class:busy=move || loading.get()
                role="group"
                aria-label="Garis waktu 24 jam, ketuk untuk memilih jam"
                on:click=move |ev| {
                    if let Some(f) = click_fraction(&ev) {
                        focus_hour.set(Some(((f * 24.0) as u32).min(23)));
                        shown.set(40);
                    }
                }
            >
                {move || draw(0, DAY)}
                {move || focus_hour.get().map(|h| view! {
                    <span class="tl-focus" style=format!("left:{:.3}%;width:{:.3}%", h as f32 / 24.0 * 100.0, 100.0 / 24.0)></span>
                })}
                {move || clip.get().map(|(s, _)| view! {
                    <span class="tl-now" style=format!("left:{:.3}%", s as f32 / DAY as f32 * 100.0)></span>
                })}
            </div>
            <div class="tl-scale" aria-hidden="true">
                {(0..=8).map(|i| view! { <span style=format!("left:{}%", i as f32 * 12.5)>{format!("{:02}", i * 3)}</span> }).collect_view()}
            </div>
            <div class="tl-legend">
                <span><i class="lg-rec"></i>"Rekaman"</span>
                <span><i class="lg-motion"></i>"Gerakan NVR"</span>
                <span><i class="lg-det"></i>"Orang/kendaraan (vision)"</span>
            </div>

            <div class="pb-filters" role="group" aria-label="Filter gerakan">
                <div class="chip-group">
                    {KindFilter::ALL.into_iter().map(|k| {
                        let count = move || {
                            let min = min_secs.get();
                            events.get().iter().filter(|e| k.matches(e) && e.end.saturating_sub(e.begin) >= min).count()
                        };
                        view! {
                            <button class="chip" class:on=move || kind_filter.get() == k
                                aria-pressed=move || (kind_filter.get() == k).to_string()
                                on:click=move |_| { kind_filter.set(k); shown.set(40); }>
                                {k.label()}<span class="chip-count">{count}</span>
                            </button>
                        }
                    }).collect_view()}
                </div>
                <label class="field inline">
                    <span>"Durasi"</span>
                    <select on:change=move |ev| { min_secs.set(event_target_value(&ev).parse().unwrap_or(0)); shown.set(40); }>
                        {MIN_DURATIONS.into_iter().map(|(v, t)| view! {
                            <option value=v.to_string() selected=move || min_secs.get() == v>{t}</option>
                        }).collect_view()}
                    </select>
                </label>
                <Show when=filter_active>
                    <button class="link-btn" on:click=move |_| { kind_filter.set(KindFilter::All); min_secs.set(0); }>"Hapus filter"</button>
                </Show>
            </div>

            {move || focus_hour.get().map(|h| {
                let from = h * 3600;
                view! {
                    <div class="tl-zoom">
                        <div class="tl-head">
                            <h3>{format!("Pukul {:02}.00 – {:02}.00", h, h + 1)}</h3>
                            <span class="muted small">"Ketuk untuk memutar dari titik itu"</span>
                        </div>
                        <div
                            class="tl tall"
                            role="group"
                            aria-label="Garis waktu satu jam, ketuk untuk memutar"
                            on:click=move |ev| {
                                if let Some(f) = click_fraction(&ev) {
                                    let t = from + (f * 3600.0) as u32;
                                    play(t.saturating_sub(3), clip_len.get_untracked());
                                }
                            }
                        >
                            {move || draw(from, from + 3600)}
                            {move || clip.get().filter(|(s, _)| *s >= from && *s < from + 3600).map(|(s, _)| view! {
                                <span class="tl-now" style=format!("left:{:.3}%", (s - from) as f32 / 36.0)></span>
                            })}
                        </div>
                        <div class="tl-scale" aria-hidden="true">
                            {(0..=6).map(|i| view! {
                                <span style=format!("left:{:.3}%", i as f32 * 100.0 / 6.0)>{format!("{:02}.{:02}", h + i / 6, (i % 6) * 10)}</span>
                            }).collect_view()}
                        </div>
                    </div>
                }
            })}
        </section>

        <div class="pb-layout">
            <section class="panel pb-player">
                <div class="panel-header">
                    <h2>"Rekaman"</h2>
                    <label class="field inline">
                        <span>"Durasi"</span>
                        <select on:change=move |ev| clip_len.set(event_target_value(&ev).parse().unwrap_or(30))>
                            {[15u32, 30, 60, 120].into_iter().map(|s| view! {
                                <option value=s.to_string() selected=move || clip_len.get() == s>{format!("{s} dtk")}</option>
                            }).collect_view()}
                        </select>
                    </label>
                </div>
                <div class="pb-screen">
                    {move || {
                        if clip_busy.get() {
                            let est = clip.get().map(|(_, s)| s / 2 + 3).unwrap_or(15);
                            view! {
                                <div class="pb-wait">
                                    <span class="pb-spinner" aria-hidden="true"></span>
                                    <strong>"Mengambil rekaman dari NVR…"</strong>
                                    <small>{move || format!("{} dtk berjalan, perkiraan ±{est} dtk", busy_secs.get())}</small>
                                </div>
                            }.into_view()
                        } else if let Some(u) = clip_url.get() {
                            view! { <video src=u controls=true autoplay=true muted=true playsinline=true></video> }.into_view()
                        } else {
                            view! {
                                <div class="pb-wait">
                                    <span>"Pilih jam di garis waktu atau ketuk gerakan di daftar untuk memutar rekaman."</span>
                                </div>
                            }.into_view()
                        }
                    }}
                </div>
                <Show when=move || !clip_err.get().is_empty()>
                    <p class="error-text">{move || clip_err.get()}</p>
                </Show>
                {move || clip.get().map(|(s, secs)| view! {
                    <div class="pb-bar">
                        <div class="pb-info">
                            <strong>{format!("{} – {}", hms(s), hms((s + secs).min(DAY - 1)))}</strong>
                            <small>{move || {
                                let cam = camera.get();
                                let label = channels.get().into_iter().find(|c| c.id == cam).map(|c| c.label).unwrap_or(cam);
                                format!("{label}, {} (jam NVR)", date.get())
                            }}</small>
                        </div>
                        <div class="pb-actions">
                            <button class="btn secondary small" disabled=move || clip_busy.get()
                                on:click=move |_| play(s.saturating_sub(secs), secs)>"‹ Sebelumnya"</button>
                            <button class="btn secondary small" disabled=move || clip_busy.get()
                                on:click=move |_| play(s + secs, secs)>"Berikutnya ›"</button>
                            {move || clip_url.get().filter(|_| !clip_busy.get()).map(|u| view! {
                                <a class="btn small" href=u download=format!("{}_{}_{}.mp4", camera.get_untracked(), date.get_untracked(), hms(s).replace(':', ""))>"Unduh"</a>
                            })}
                        </div>
                    </div>
                })}
            </section>

            <section class="panel pb-events">
                <div class="panel-header">
                    <h2>{move || match focus_hour.get() {
                        Some(h) => format!("Gerakan pukul {h:02}.00"),
                        None if filter_active() => format!("Gerakan: {}", kind_filter.get().label()),
                        None => "Gerakan dicatat NVR".to_string(),
                    }}</h2>
                    <Show when=move || focus_hour.get().is_some()>
                        <button class="link-btn" on:click=move |_| focus_hour.set(None)>"Semua jam"</button>
                    </Show>
                </div>
                {move || {
                    let list = visible();
                    if list.is_empty() {
                        let msg = if loading.get() {
                            "Memuat…"
                        } else if filter_active() {
                            "Tidak ada gerakan yang cocok dengan filter pada rentang ini."
                        } else {
                            "Tidak ada gerakan tercatat pada rentang ini."
                        };
                        return view! { <p class="empty-state">{msg}</p> }.into_view();
                    }
                    let total = list.len();
                    let n = shown.get();
                    view! {
                        <ul class="ev-list">
                            {list.into_iter().take(n).map(|e| {
                                let (b, en) = (e.begin, e.end);
                                let dur = en.saturating_sub(b);
                                let secs = (dur + 6).clamp(15, 120);
                                let active = move || clip.get().map(|(s, _)| s + 3 >= b && s <= en).unwrap_or(false);
                                view! {
                                    <li>
                                        <button class="ev-row" class:active=active data-has=(!e.objects.is_empty()).to_string()
                                            on:click=move |_| play(b.saturating_sub(3), secs)>
                                            <span class="ev-time">{hm(b)}<small>{format!(":{:02}", b % 60)}</small></span>
                                            <span class="ev-main">
                                                <span class="ev-what">
                                                    {if e.objects.is_empty() {
                                                        if e.kind == "alarm" { "Alarm NVR".to_string() } else { "Gerakan".to_string() }
                                                    } else {
                                                        e.objects.iter().map(|o| object_name(o)).collect::<Vec<_>>().join(" + ")
                                                    }}
                                                </span>
                                                <small>{format!("{} dtk, sampai {}", dur, hms(en))}</small>
                                            </span>
                                            <span class="ev-play" aria-hidden="true">"▶"</span>
                                        </button>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                        {(total > n).then(|| view! {
                            <button class="btn secondary full" on:click=move |_| shown.update(|s| *s += 40)>
                                {format!("Tampilkan lagi ({} tersisa)", total - n)}
                            </button>
                        })}
                    }.into_view()
                }}
            </section>
        </div>
    }
}
