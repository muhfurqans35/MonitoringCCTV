use leptos::*;

use crate::api::AlertEvent;
use crate::ws::RoomTelemetryMsg;

/// Asal alert dalam bahasa pemilik kost.
fn source_label(source: &str) -> String {
    match source {
        "gate_left" => "Jalanan kiri gerbang".into(),
        "gate_right" => "Jalanan kanan gerbang".into(),
        "main_gate" => "Gerbang utama".into(),
        "kost" => "Area kost".into(),
        s if s.starts_with("door_") => "Bel pintu".into(),
        s if s.starts_with("room_") => format!("Kamar {}", s.trim_start_matches("room_")),
        s => s.to_string(),
    }
}

fn severity_text(severity: &str) -> &'static str {
    match severity {
        "critical" => "Darurat",
        "warning" => "Perlu dicek",
        _ => "Info",
    }
}

/// "YYYY-MM-DD HH:MM:SS" (UTC dari SQLite) -> jam lokal browser.
fn local_stamp(utc: &str) -> String {
    let iso = format!("{}Z", utc.replace(' ', "T"));
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(&iso));
    if d.get_time().is_nan() {
        return utc.to_string();
    }
    format!("{:02}/{:02} {:02}.{:02}", d.get_date(), d.get_month() + 1, d.get_hours(), d.get_minutes())
}

/// Kartu kamar berbentuk gantungan kunci kost: nomor kamar besar, lubang ring di kiri.
#[component]
pub fn RoomCard(room_code: String, live: Option<RoomTelemetryMsg>) -> impl IntoView {
    let presence = live.as_ref().map(|l| l.presence).unwrap_or(false);
    let has_data = live.is_some();
    let watt = live.as_ref().map(|l| l.watt).unwrap_or(0.0);
    let kwh = live.as_ref().map(|l| l.kwh_cumulative).unwrap_or(0.0);
    let state = if !has_data { "unknown" } else if presence { "in" } else { "out" };

    view! {
        <article class="key-tag" data-state=state>
            <span class="key-hole" aria-hidden="true"></span>
            <div class="key-body">
                <div class="key-number">{room_code}</div>
                <div class="key-state">
                    {match state {
                        "in" => "Ada penghuni",
                        "out" => "Kosong",
                        _ => "Belum ada data",
                    }}
                </div>
                <dl class="key-meter">
                    <div><dt>"Daya"</dt><dd>{format!("{watt:.0} W")}</dd></div>
                    <div><dt>"Bulan ini"</dt><dd>{format!("{kwh:.1} kWh")}</dd></div>
                </dl>
            </div>
        </article>
    }
}

/// Daftar alert. `on_resolve` dipanggil dengan id saat "Tandai selesai" ditekan.
#[component]
pub fn AlertFeed(
    alerts: Signal<Vec<AlertEvent>>,
    on_resolve: Callback<i64>,
    #[prop(default = false)] only_open: bool,
    #[prop(default = 30)] limit: usize,
) -> impl IntoView {
    let shown = move || {
        alerts
            .get()
            .into_iter()
            .filter(|a| !only_open || !a.resolved)
            .take(limit)
            .collect::<Vec<_>>()
    };

    view! {
        <div class="alert-list">
            {move || {
                let list = shown();
                if list.is_empty() {
                    view! {
                        <p class="empty-state">
                            {if only_open { "Tidak ada yang perlu dicek. Semua alert sudah ditangani." } else { "Belum ada alert." }}
                        </p>
                    }.into_view()
                } else {
                    list.into_iter().map(|a| {
                        let id = a.id;
                        let sev = a.severity.clone();
                        view! {
                            <div class="alert-row" data-severity=sev.clone() class:done=a.resolved>
                                <span class="alert-dot" aria-hidden="true"></span>
                                <div class="alert-text">
                                    <p>{a.message.clone()}</p>
                                    <small>
                                        <span class="alert-sev">{severity_text(&sev)}</span>
                                        <span>{source_label(&a.source)}</span>
                                        <time>{local_stamp(&a.created_at)}</time>
                                    </small>
                                </div>
                                {if a.resolved {
                                    view! { <span class="alert-ok">"Selesai"</span> }.into_view()
                                } else {
                                    view! {
                                        <button class="btn small secondary" on:click=move |_| on_resolve.call(id)>"Tandai selesai"</button>
                                    }.into_view()
                                }}
                            </div>
                        }
                    }).collect_view()
                }
            }}
        </div>
    }
}
