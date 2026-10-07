//! Alarm motion langsung dari NVR (protokol DVRIP biner, pesan 1504) -> pemicu analisis kamera.
//!
//! Satu thread menjaga koneksi alarm ke NVR (sambung ulang otomatis). Tiap event:
//!   * MQTT `kost/cam/<id>/nvr` (tidak retained): {event, status start/stop, nvr_time, skew_seconds}
//!     -> terlihat di MQTT Explorer; TIDAK jadi alert (NVR bisa terpicu daun/bayangan terus-menerus).
//!   * motion mulai -> `Trigger` kamera itu: loop vision bangun seketika, frame dianalisis model,
//!     dan selama `alarm_boost_seconds` frame diambil lebih rapat.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{Datelike, Timelike};
use dvrip::{AlarmEvent, Nvr, NvrTime};
use rumqttc::{AsyncClient, QoS};
use tokio::sync::Notify;

#[derive(Default)]
pub struct Trigger {
    pub notify: Notify,
    /// Ada alarm NVR yang belum ditangani loop kamera.
    pub pending: AtomicBool,
    pub alarms: AtomicU64,
    boost_until: Mutex<Option<Instant>>,
}

impl Trigger {
    pub fn boosted(&self) -> bool {
        self.boost_until.lock().map(|g| g.map(|t| Instant::now() < t).unwrap_or(false)).unwrap_or(false)
    }

    fn fire(&self, boost: Duration) {
        self.alarms.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut g) = self.boost_until.lock() {
            *g = Some(Instant::now() + boost);
        }
        self.pending.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }
}

/// Selisih jam NVR terhadap jam Pi (detik, positif = NVR lebih cepat).
fn skew_seconds(t: &NvrTime) -> Option<i64> {
    let n = chrono::Local::now().naive_local();
    let local = NvrTime::new(n.year() as u32, n.month(), n.day(), n.hour(), n.minute(), n.second())?;
    Some(t.secs_since(&local))
}

pub fn spawn(
    access: (String, u16, String, String),
    triggers: HashMap<u32, (String, Arc<Trigger>)>,
    mqtt: AsyncClient,
    boost: Duration,
) {
    let (host, port, user, password) = access;
    let res = std::thread::Builder::new().name("nvr-alarm".into()).spawn(move || {
        let nvr = Nvr::new(host.clone(), port, user, password);
        let stop = AtomicBool::new(false);
        let mut backoff = 5u64;
        let mut last_skew_warn: Option<Instant> = None;
        loop {
            let began = Instant::now();
            tracing::info!("alarm NVR: berlangganan ke {host}:{port}");
            let r = nvr.alarm_loop(&stop, |ev: AlarmEvent| {
                let Some((id, trig)) = triggers.get(&ev.channel) else {
                    tracing::debug!("alarm NVR CAM{} {} (tidak dipantau)", ev.channel, ev.event_name());
                    return;
                };
                let skew = ev.time.as_ref().and_then(skew_seconds);
                if let Some(s) = skew.filter(|s| s.abs() > 60) {
                    if last_skew_warn.map(|t| t.elapsed() > Duration::from_secs(3600)).unwrap_or(true) {
                        last_skew_warn = Some(Instant::now());
                        tracing::warn!(
                            "jam NVR selisih {:+.1} menit dari jam Pi: playback & timeline tidak akan cocok. \
                             Aktifkan NTP di NVR (System -> General -> Date/Time).",
                            s as f64 / 60.0
                        );
                    }
                }
                let payload = serde_json::json!({
                    "camera_id": id,
                    "channel": ev.channel,
                    "event": ev.event_name(),
                    "status": if ev.start { "start" } else { "stop" },
                    "nvr_time": ev.time.map(|t| t.to_string()),
                    "skew_seconds": skew,
                    "timestamp": chrono::Utc::now(),
                    "source": "nvr",
                });
                if let Ok(bytes) = serde_json::to_vec(&payload) {
                    let _ = mqtt.try_publish(format!("kost/cam/{id}/nvr"), QoS::AtMostOnce, false, bytes);
                }
                if ev.start && (ev.event == 4 || ev.event == 1) {
                    tracing::debug!("alarm NVR: kamera {id} {} mulai", ev.event_name());
                    trig.fire(boost);
                }
            });
            match r {
                Ok(()) => return,
                Err(e) => tracing::warn!("alarm NVR terputus: {e}"),
            }
            backoff = if began.elapsed() > Duration::from_secs(120) { 5 } else { (backoff * 2).min(60) };
            std::thread::sleep(Duration::from_secs(backoff));
        }
    });
    if let Err(e) = res {
        tracing::error!("thread alarm NVR gagal dibuat: {e}");
    }
}
