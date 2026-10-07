//! Halaman Akun & keamanan: ganti password, perangkat yang sedang login, keluar.

use leptos::*;

use crate::api::{self, Me, SessionItem};

/// "Mozilla/5.0 (Windows NT 10.0; ...) Chrome/129..." -> "Chrome di Windows"
fn device_name(ua: &str) -> String {
    let browser = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("OPR/") {
        "Opera"
    } else if ua.contains("Firefox/") {
        "Firefox"
    } else if ua.contains("Chrome/") {
        "Chrome"
    } else if ua.contains("Safari/") {
        "Safari"
    } else if ua.is_empty() {
        "Perangkat"
    } else {
        "Browser"
    };
    let os = if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iPhone/iPad"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Mac OS") {
        "Mac"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        ""
    };
    if os.is_empty() {
        browser.to_string()
    } else {
        format!("{browser} di {os}")
    }
}

fn ago(unix: i64) -> String {
    let now = (js_sys::Date::now() / 1000.0) as i64;
    let d = (now - unix).max(0);
    match d {
        0..=119 => "baru saja".into(),
        120..=3599 => format!("{} menit lalu", d / 60),
        3600..=86_399 => format!("{} jam lalu", d / 3600),
        _ => format!("{} hari lalu", d / 86_400),
    }
}

#[component]
pub fn AccountView(me: RwSignal<Option<Me>>) -> impl IntoView {
    let current = create_rw_signal(String::new());
    let new1 = create_rw_signal(String::new());
    let new2 = create_rw_signal(String::new());
    let pw_msg = create_rw_signal::<Option<(bool, String)>>(None);
    let pw_busy = create_rw_signal(false);
    let sessions = create_rw_signal(Vec::<SessionItem>::new());
    let sess_err = create_rw_signal(String::new());

    let load_sessions = move || {
        spawn_local(async move {
            match api::fetch_sessions().await {
                Ok(v) => {
                    let _ = sessions.try_set(v);
                }
                Err(e) => {
                    let _ = sess_err.try_set(e);
                }
            }
        });
    };
    load_sessions();

    let submit_pw = move || {
        let (c, a, b) = (current.get_untracked(), new1.get_untracked(), new2.get_untracked());
        if a.chars().count() < 10 {
            pw_msg.set(Some((false, "Password baru minimal 10 karakter.".into())));
            return;
        }
        if a != b {
            pw_msg.set(Some((false, "Ulangi password baru: isinya belum sama.".into())));
            return;
        }
        pw_busy.set(true);
        pw_msg.set(None);
        spawn_local(async move {
            match api::change_password(&c, &a).await {
                Ok(()) => {
                    let _ = current.try_set(String::new());
                    let _ = new1.try_set(String::new());
                    let _ = new2.try_set(String::new());
                    let _ = pw_msg.try_set(Some((true, "Password diganti. Perangkat lain sudah dikeluarkan.".into())));
                    load_sessions();
                }
                Err(e) => {
                    let _ = pw_msg.try_set(Some((false, e)));
                }
            }
            let _ = pw_busy.try_set(false);
        });
    };

    let logout_all = move |_| {
        let ok = web_sys::window()
            .and_then(|w| w.confirm_with_message("Keluarkan akun ini dari SEMUA perangkat, termasuk perangkat ini?").ok())
            .unwrap_or(false);
        if !ok {
            return;
        }
        spawn_local(async move {
            let _ = api::logout_all().await;
            me.set(None);
        });
    };
    let logout = move |_| {
        spawn_local(async move {
            api::logout().await;
            me.set(None);
        });
    };

    view! {
        <p class="lede">
            {move || me.get().map(|m| format!(
                "Masuk sebagai {} ({}).",
                m.username,
                if m.is_owner() { "pemilik" } else { "penghuni" }
            ))}
        </p>

        <div class="two-col account">
            <section class="panel">
                <div class="panel-header"><h2>"Ganti password"</h2></div>
                <form class="stack" on:submit=move |ev| { ev.prevent_default(); submit_pw(); }>
                    <label class="field">
                        <span>"Password sekarang"</span>
                        <input type="password" autocomplete="current-password" required=true
                            prop:value=move || current.get() on:input=move |ev| current.set(event_target_value(&ev)) />
                    </label>
                    <label class="field">
                        <span>"Password baru"</span>
                        <input type="password" autocomplete="new-password" required=true minlength="10"
                            prop:value=move || new1.get() on:input=move |ev| new1.set(event_target_value(&ev)) />
                        <small class="hint">"Minimal 10 karakter, campuran huruf dan angka/simbol. Kalimat panjang juga boleh."</small>
                    </label>
                    <label class="field">
                        <span>"Ulangi password baru"</span>
                        <input type="password" autocomplete="new-password" required=true
                            prop:value=move || new2.get() on:input=move |ev| new2.set(event_target_value(&ev)) />
                    </label>
                    <button class="btn" type="submit" disabled=move || pw_busy.get()>
                        {move || if pw_busy.get() { "Menyimpan…" } else { "Simpan password" }}
                    </button>
                    {move || pw_msg.get().map(|(ok, m)| view! {
                        <p class=if ok { "ok-text" } else { "error-text" } role="status">{m}</p>
                    })}
                </form>
            </section>

            <section class="panel">
                <div class="panel-header">
                    <h2>"Perangkat yang sedang login"</h2>
                    <span class="panel-tag">{move || sessions.get().len()}</span>
                </div>
                <Show when=move || !sess_err.get().is_empty()>
                    <p class="error-text">{move || sess_err.get()}</p>
                </Show>
                <ul class="session-list">
                    {move || sessions.get().into_iter().map(|s| view! {
                        <li class:current=s.current>
                            <div>
                                <strong>{device_name(&s.user_agent)}</strong>
                                {s.current.then(|| view! { <span class="panel-tag success">"Perangkat ini"</span> })}
                            </div>
                            <small>{format!("IP {} · aktif {} · masuk {}", s.ip, ago(s.last_seen), ago(s.created_at))}</small>
                        </li>
                    }).collect_view()}
                </ul>
                <p class="muted small">"Ada perangkat yang tidak Anda kenal? Keluarkan semua lalu ganti password."</p>
                <div class="account-actions">
                    <button class="btn danger" on:click=logout_all>"Keluar dari semua perangkat"</button>
                    <button class="btn secondary" on:click=logout>"Keluar"</button>
                </div>
            </section>
        </div>
    }
}
