//! Inti sistem doorbell: alur kiriman dari layar, verifikasi zona paket lewat kamera,
//! notifikasi, pengingat, penutupan otomatis, retensi foto, dan ringkasan harian.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, FixedOffset, Utc};
use common::models::{AlertSeverity, AlertType};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::{DoorbellConfig, Notify, Purpose};
use crate::detector::{crop_roi_jpeg, Detector, ZoneState};
use crate::store::Store;
use crate::traits::{Button, Buttons, Camera, Clock, Notifier, Publisher};

const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Siapa yang menutup/menyanggah catatan paket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// Chat Telegram (pengelola atau penghuni kamar terkait).
    Chat(i64),
    /// Pemilik yang sudah login di dashboard (JWT diperiksa oleh API).
    Dashboard,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub door_id: String,
    pub mode: String,
    pub node_online: bool,
    pub node_ip: String,
    pub node_cfg_ver: String,
    pub hub_cfg_ver: String,
    pub calibrated: bool,
    pub waiting: i64,
    pub verifying: i64,
    pub camera_ok: bool,
    pub camera_sensor: String,
    pub camera_error: String,
    pub stream_url: String,
}

/// Info kamera terakhir yang dilaporkan node lewat payload status MQTT.
#[derive(Debug, Clone, Default)]
struct CameraInfo {
    ok: bool,
    sensor: String,
    error: String,
    stream_url: String,
}

/// Foto terakhir untuk live view dashboard. Dipakai bersama oleh semua penonton supaya
/// node ESP32 tidak dibanjiri request (/capture dilayani berurutan di node).
#[derive(Default)]
struct SnapshotCache {
    at: f64,
    jpeg: Vec<u8>,
}

/// Umur maksimum foto cache sebelum mengambil ulang dari node.
const SNAPSHOT_MAX_AGE_S: f64 = 0.8;

struct Session {
    sub_id: i64,
    room: String,
    start: f64,
    next_poll: f64,
    before: Vec<u8>,
    prev: Vec<u8>,
    last: Vec<u8>,
    hits: u32,
    fails: u32,
}

enum Poll {
    Pending,
    Verified,
    Failed(&'static str),
}

#[derive(Default)]
struct State {
    sessions: HashMap<i64, Session>,
    device_online: bool,
    device_cfg_ver: Option<String>,
    camera: CameraInfo,
    rooms: Vec<String>,
    empty_hits: u32,
    last_zone_check: f64,
    last_housekeeping: f64,
    last_purge: f64,
    last_rooms_refresh: f64,
}

pub struct Hub {
    pub cfg: Arc<DoorbellConfig>,
    pub store: Store,
    pub detector: Detector,
    camera: Arc<dyn Camera>,
    notifier: Arc<dyn Notifier>,
    publisher: Arc<dyn Publisher>,
    clock: Arc<dyn Clock>,
    photo_dir: PathBuf,
    managers: Vec<i64>,
    tz: FixedOffset,
    /// IP node terakhir yang dilaporkan lewat MQTT; dibaca oleh kamera HTTP.
    device_ip: Arc<RwLock<Option<String>>>,
    state: Mutex<State>,
    snapshot: Mutex<SnapshotCache>,
}

pub struct HubDeps {
    pub camera: Arc<dyn Camera>,
    pub notifier: Arc<dyn Notifier>,
    pub publisher: Arc<dyn Publisher>,
    pub clock: Arc<dyn Clock>,
    pub device_ip: Arc<RwLock<Option<String>>>,
}

fn topic_base(door: &str) -> String {
    format!("kost/door/{door}")
}

impl Hub {
    pub fn new(
        cfg: Arc<DoorbellConfig>,
        store: Store,
        detector: Detector,
        managers: Vec<i64>,
        photo_dir: impl AsRef<Path>,
        deps: HubDeps,
    ) -> anyhow::Result<Self> {
        std::fs::create_dir_all(photo_dir.as_ref())?;
        let tz = FixedOffset::east_opt(cfg.utc_offset_minutes * 60).ok_or_else(|| anyhow::anyhow!("utc_offset tidak valid"))?;
        Ok(Self {
            cfg,
            store,
            detector,
            camera: deps.camera,
            notifier: deps.notifier,
            publisher: deps.publisher,
            clock: deps.clock,
            photo_dir: photo_dir.as_ref().to_path_buf(),
            managers,
            tz,
            device_ip: deps.device_ip,
            state: Mutex::new(State::default()),
            snapshot: Mutex::new(SnapshotCache::default()),
        })
    }

    pub fn is_manager(&self, chat_id: i64) -> bool {
        self.managers.contains(&chat_id)
    }

    // ------------------------------------------------------------------ util
    fn hhmm(&self, ts: f64) -> String {
        DateTime::<Utc>::from_timestamp(ts as i64, 0)
            .map(|d| d.with_timezone(&self.tz).format("%H:%M").to_string())
            .unwrap_or_else(|| "--:--".into())
    }

    async fn event(&self, type_: &str, room: Option<&str>, zone: Option<&str>, score: Option<f64>, latency: Option<f64>, detail: Option<&str>) {
        if let Err(e) = self.store.add_event(self.clock.now(), type_, room, zone, score, latency, detail).await {
            tracing::warn!("gagal menulis event {type_}: {e}");
        }
    }

    async fn send(&self, chat_id: i64, text: &str, photo: Option<&[u8]>, buttons: Option<&Buttons>) {
        self.notifier.send(chat_id, text, photo, buttons).await;
    }

    async fn notify_managers(&self, text: &str, photo: Option<&[u8]>, buttons: Option<&Buttons>) {
        for cid in &self.managers {
            self.send(*cid, text, photo, buttons).await;
        }
    }

    /// Catat ke alert_events (tampil di dashboard) — notifikasi Telegram dikirim terpisah oleh hub.
    async fn alert(&self, source: &str, t: AlertType, sev: AlertSeverity, message: &str) {
        if let Err(e) = self.store.insert_alert(source, t.as_str(), sev.as_str(), message).await {
            tracing::warn!("gagal menulis alert: {e}");
        }
    }

    fn package_buttons(sub_id: i64) -> Buttons {
        vec![vec![
            Button::new("✅ Sudah diambil", format!("taken:{sub_id}")),
            Button::new("❌ Bukan paket saya", format!("notmine:{sub_id}")),
        ]]
    }

    fn close_button(sub_id: i64, label: &str) -> Buttons {
        vec![vec![Button::new(label, format!("taken:{sub_id}"))]]
    }

    /// Kirim ke penghuni kamar. Bila tak ada yang terdaftar, arahkan ke pengelola.
    async fn deliver(&self, room: &str, sub_id: i64, text: &str, photo: Option<&[u8]>, manager_fallback: bool) -> usize {
        let chats = self.store.active_chats(room).await.unwrap_or_default();
        let buttons = Self::package_buttons(sub_id);
        for cid in &chats {
            self.send(*cid, text, photo, Some(&buttons)).await;
        }
        if chats.is_empty() && manager_fallback {
            let b = Self::close_button(sub_id, "✅ Tutup (sudah diambil)");
            self.notify_managers(&format!("ℹ️ Kamar {room} belum punya penghuni terdaftar. {text}"), photo, Some(&b)).await;
        }
        chats.len()
    }

    /// Simpan HANYA potongan ROI. Mengembalikan (path, bytes).
    fn save_photo(&self, sub_id: i64, jpeg: &[u8]) -> Option<(String, Vec<u8>)> {
        let crop = match crop_roi_jpeg(jpeg, &self.cfg.zone.roi, 80) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("gagal memotong ROI: {e}");
                return None;
            }
        };
        let path = self.photo_dir.join(format!("pkg_{sub_id}_{}.jpg", self.clock.now() as i64));
        std::fs::write(&path, &crop).ok()?;
        Some((path.to_string_lossy().into_owned(), crop))
    }

    fn read_photo(path: &Option<String>) -> Option<Vec<u8>> {
        path.as_ref().and_then(|p| std::fs::read(p).ok())
    }

    async fn delete_photo(&self, id: i64, path: &Option<String>) {
        if let Some(p) = path {
            let _ = std::fs::remove_file(p);
            let _ = self.store.clear_photo(id).await;
        }
    }

    // ------------------------------------------------------- konfigurasi/mode
    pub async fn mode(&self) -> String {
        match self.store.get_setting("mode").await {
            Ok(Some(m)) if self.cfg.texts.contains_key(&m) => m,
            _ => self.cfg.default_mode.clone(),
        }
    }

    pub async fn refresh_rooms(&self) {
        let mut st = self.state.lock().await;
        self.refresh_rooms_locked(&mut st).await;
    }

    async fn refresh_rooms_locked(&self, st: &mut State) {
        match self.store.rooms().await {
            Ok(r) => {
                st.rooms = r.into_iter().filter(|c| !c.is_empty() && c.len() <= 4 && c.is_ascii()).collect();
                st.last_rooms_refresh = self.clock.now();
                if st.rooms.is_empty() {
                    tracing::warn!("tabel rooms kosong/tidak valid: layar node akan memakai daftar bawaan");
                }
            }
            Err(e) => tracing::warn!("gagal baca tabel rooms: {e}"),
        }
    }

    pub async fn publish_config(&self) {
        let mut st = self.state.lock().await;
        self.publish_config_locked(&mut st).await;
    }

    async fn publish_config_locked(&self, st: &mut State) {
        self.refresh_rooms_locked(st).await;
        let mode = self.mode().await;
        let payload = self.cfg.device_payload(&mode, &st.rooms);
        let ver = payload["ver"].as_str().unwrap_or("").to_string();
        self.publisher.publish(&format!("{}/cmd/config", topic_base(&self.cfg.door_id)), &payload, true).await;
        self.event("config_published", None, None, None, None, Some(&ver)).await;
    }

    pub async fn set_mode(&self, mode: &str) -> bool {
        if !self.cfg.texts.contains_key(mode) {
            return false;
        }
        let mut st = self.state.lock().await;
        if self.store.set_setting("mode", mode).await.is_err() {
            return false;
        }
        self.publish_config_locked(&mut st).await;
        true
    }

    pub async fn calibrate(&self) -> (bool, String) {
        let mut st = self.state.lock().await;
        let Some(jpeg) = self.camera.capture().await else {
            return (false, "Kamera tidak merespons. Cek node.".into());
        };
        if self.detector.set_reference(&jpeg).is_err() {
            return (false, "Gambar dari kamera tidak valid.".into());
        }
        st.empty_hits = 0;
        drop(st);
        self.event("calibrated", None, None, None, None, None).await;
        (true, "Referensi zona kosong tersimpan. Pastikan saat ini zona memang kosong.".into())
    }

    // ------------------------------------------------------------------- MQTT
    pub async fn handle_message(&self, topic: &str, payload: &[u8]) {
        let prefix = format!("{}/", topic_base(&self.cfg.door_id));
        let Some(rest) = topic.strip_prefix(&prefix) else { return };
        let d: Value = match serde_json::from_slice(payload) {
            Ok(v @ Value::Object(_)) => v,
            _ => {
                self.event("bad_json", None, None, None, None, Some(topic)).await;
                return;
            }
        };
        let mut st = self.state.lock().await;
        match rest {
            "event/submit" => self.on_submit(&mut st, &d).await,
            "event/wake" => self.event("wake", None, None, None, None, None).await,
            "event/zone" => self.on_zone_event(&mut st, &d).await,
            "status" => self.on_status(&mut st, &d).await,
            _ => {}
        }
    }

    async fn on_status(&self, st: &mut State, d: &Value) {
        let online = d.get("online").and_then(Value::as_bool).unwrap_or(true);
        let uid = format!("door_{}", self.cfg.door_id);
        if online != st.device_online {
            self.event(if online { "node_online" } else { "node_offline" }, None, None, None, None, d.get("ip").and_then(Value::as_str)).await;
            if let Err(e) = self.store.upsert_door_device(&uid, &self.cfg.location, online).await {
                tracing::warn!("gagal update devices: {e}");
            }
            if !online && st.device_online {
                let msg = format!("Node doorbell '{}' offline.", self.cfg.door_id);
                self.alert(&uid, AlertType::DeviceOffline, AlertSeverity::Warning, &msg).await;
                self.notify_managers(&format!("⚠️ {msg}"), None, None).await;
            }
        } else if online {
            let _ = self.store.upsert_door_device(&uid, &self.cfg.location, true).await;
        }
        st.device_online = online;
        if online {
            if let Some(ip) = d.get("ip").and_then(Value::as_str) {
                if let Ok(mut g) = self.device_ip.write() {
                    *g = Some(ip.to_string());
                }
            }
        }
        if let Some(v) = d.get("ver").and_then(Value::as_str) {
            st.device_cfg_ver = Some(v.to_string());
        }
        if online {
            let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let cam_ok = d.get("cam").and_then(Value::as_bool).unwrap_or(false);
            if cam_ok != st.camera.ok {
                self.event(if cam_ok { "camera_ok" } else { "camera_fail" }, None, None, None, None, Some(&s("cam_err"))).await;
                if !cam_ok {
                    // Node masih online tapi kamera berhenti: verifikasi paket tidak bisa jalan.
                    let msg = format!("Kamera node doorbell '{}' gagal: {}", self.cfg.door_id, s("cam_err"));
                    self.alert(&uid, AlertType::DeviceOffline, AlertSeverity::Warning, &msg).await;
                }
            }
            st.camera = CameraInfo { ok: cam_ok, sensor: s("cam_sensor"), error: s("cam_err"), stream_url: s("stream_url") };
        } else {
            st.camera.ok = false;
        }
    }

    /// Perintah /foto (pengelola): kirim foto penuh kamera pintu ke chat tersebut.
    pub async fn send_snapshot(&self, chat_id: i64) -> Result<(), String> {
        let (jpeg, at) = self.snapshot().await?;
        let text = format!("📷 Kamera pintu {}, pukul {}.", self.cfg.door_id, self.hhmm(at));
        self.send(chat_id, &text, Some(&jpeg), None).await;
        self.event("snapshot_sent", None, None, None, None, Some(&chat_id.to_string())).await;
        Ok(())
    }

    /// Foto penuh terbaru untuk live view dashboard (bukan potongan zona; tidak disimpan ke disk).
    /// Mengembalikan (jpeg, epoch detik saat diambil).
    pub async fn snapshot(&self) -> Result<(Vec<u8>, f64), String> {
        // Kunci cache SELAMA mengambil: penonton lain menunggu & memakai hasil yang sama.
        let mut cache = self.snapshot.lock().await;
        let now = self.clock.now();
        if !cache.jpeg.is_empty() && now - cache.at < SNAPSHOT_MAX_AGE_S {
            return Ok((cache.jpeg.clone(), cache.at));
        }
        let jpeg = self.camera.capture_detailed().await?;
        cache.at = self.clock.now();
        cache.jpeg = jpeg.clone();
        Ok((jpeg, cache.at))
    }

    async fn on_zone_event(&self, st: &mut State, d: &Value) {
        // Hasil inferensi dari ESP32 (mode edge). Dipakai untuk penutupan otomatis.
        if let Some(state) = d.get("state").and_then(Value::as_str) {
            if matches!(state, "kosong" | "ada_paket" | "terhalang") {
                self.event("zone_edge", None, Some(state), d.get("score").and_then(Value::as_f64), None, None).await;
                self.observe_zone(st, state, self.clock.now()).await;
            }
        }
    }

    // ---------------------------------------------------------------- kiriman
    async fn on_submit(&self, st: &mut State, d: &Value) {
        let sid: String = d.get("sid").and_then(Value::as_str).unwrap_or("").chars().take(40).collect();
        let purpose = d.get("purpose").and_then(Value::as_str).and_then(|p| self.cfg.purpose(p));
        let Some(p) = purpose.filter(|_| !sid.is_empty()) else {
            let detail: String = d.to_string().chars().take(200).collect();
            self.event("bad_submit", None, None, None, None, Some(&detail)).await;
            return;
        };
        let mut room: Option<String> = None;
        if p.needs_room {
            let r = match d.get("room") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Number(n)) => Some(n.to_string()),
                _ => None,
            };
            match r {
                Some(r) if st.rooms.contains(&r) => room = Some(r),
                other => {
                    self.event("bad_submit", None, None, None, None, Some(&format!("kamar tidak dikenal: {other:?}"))).await;
                    return;
                }
            }
        }
        let late = d.get("queued").and_then(Value::as_bool).unwrap_or(false) || d.get("late").and_then(Value::as_bool).unwrap_or(false);
        let now = self.clock.now();
        let sub_id = match self.store.add_submission(&sid, &self.cfg.door_id, now, &p.id, room.as_deref(), late).await {
            Ok(Some(id)) => id,
            Ok(None) => {
                self.event("dup_submit", room.as_deref(), None, None, None, Some(&sid)).await;
                return;
            }
            Err(e) => {
                tracing::error!("gagal menyimpan kiriman: {e}");
                return;
            }
        };
        self.event("submit", room.as_deref(), None, None, None, Some(&format!("{} late={late}", p.id))).await;
        let t = self.hhmm(now);
        let late_note = if late { " (dikirim ulang setelah node sempat offline)" } else { "" };

        if p.notify == Notify::Manager {
            let _ = self.store.mark_done(sub_id).await;
            self.notify_managers(&format!("🔔 {} di pintu {}, pukul {t}.{late_note}", p.label, self.cfg.door_id), None, None).await;
            return;
        }
        let room = room.unwrap_or_default();
        if p.verify_zone {
            if late {
                self.finish_unverified(sub_id, &room, None, "dikirim ulang setelah node offline").await;
            } else {
                self.start_verification(st, sub_id, room, now).await;
            }
            return;
        }
        self.notify_room_simple(&room, p, &t, late_note, sub_id).await;
    }

    /// Tujuan lain untuk penghuni (mis. pengantar makanan): cukup beri tahu.
    async fn notify_room_simple(&self, room: &str, p: &Purpose, t: &str, late_note: &str, sub_id: i64) {
        let _ = self.store.mark_done(sub_id).await;
        let text = format!("🛵 Ada {} untuk kamar {room} di pintu {}, pukul {t}.{late_note}", p.label.to_lowercase(), self.cfg.door_id);
        let chats = self.store.active_chats(room).await.unwrap_or_default();
        for cid in &chats {
            self.send(*cid, &text, None, None).await;
        }
        if chats.is_empty() {
            self.notify_managers(&format!("ℹ️ Kamar {room} belum punya penghuni terdaftar. {text}"), None, None).await;
        }
    }

    // ------------------------------------------------- verifikasi zona paket
    async fn start_verification(&self, st: &mut State, sub_id: i64, room: String, now: f64) {
        let Some(before) = self.camera.capture().await else {
            self.finish_unverified(sub_id, &room, None, "kamera tidak merespons saat pencatatan").await;
            return;
        };
        let _ = self.store.mark_verifying(sub_id).await;
        st.sessions.insert(
            sub_id,
            Session {
                sub_id,
                room,
                start: now,
                next_poll: now + self.cfg.zone.poll_interval_s,
                prev: before.clone(),
                last: before.clone(),
                before,
                hits: 0,
                fails: 0,
            },
        );
    }

    async fn poll(&self, s: &mut Session, now: f64) -> Poll {
        let z = &self.cfg.zone;
        s.next_poll = now + z.poll_interval_s;
        let measured = match self.camera.capture().await {
            None => Err(()),
            Some(cur) => {
                let r = (|| -> anyhow::Result<(ZoneState, f64, f64)> {
                    let (state, _) = self.detector.classify(&cur)?;
                    let changed = self.detector.change(&s.before, &cur)?;
                    let motion = self.detector.change(&s.prev, &cur)?;
                    Ok((state, changed, motion))
                })();
                r.map(|v| (v, cur)).map_err(|_| ())
            }
        };
        let ((state, changed, motion), cur) = match measured {
            Ok(v) => v,
            Err(()) => {
                s.fails += 1;
                return if s.fails >= 3 { Poll::Failed("kamera tidak merespons") } else { Poll::Pending };
            }
        };
        s.fails = 0;
        s.prev = cur.clone();
        s.last = cur;
        // Perubahan sangat besar terhadap kondisi sebelum = orang/objek besar menghalangi, bukan paket.
        // Berlaku juga saat detektor belum dikalibrasi.
        let obstructed = state == ZoneState::Terhalang || changed >= z.obstructed_frac;
        let ok = !obstructed && motion < z.motion_frac && changed >= z.change_frac;
        s.hits = if ok { s.hits + 1 } else { 0 };
        self.event("zone_poll", Some(&s.room), Some(state.as_str()), Some(changed), None, Some(&format!("motion={motion:.3} hits={}", s.hits))).await;
        if s.hits >= z.stable_polls {
            Poll::Verified
        } else {
            Poll::Pending
        }
    }

    async fn finish_verified(&self, s: &Session, now: f64) {
        let saved = self.save_photo(s.sub_id, &s.last);
        let (path, photo) = match &saved {
            Some((p, b)) => (Some(p.as_str()), Some(b.as_slice())),
            None => (None, None),
        };
        let _ = self.store.mark_waiting(s.sub_id, true, path, None).await;
        self.event("verified", Some(&s.room), None, None, Some(now - s.start), None).await;
        let text = format!(
            "📦 Paket untuk kamar {} tercatat pukul {} dan terlihat di {}. Mohon segera diambil.",
            s.room,
            self.hhmm(now),
            self.cfg.package_location
        );
        self.deliver(&s.room, s.sub_id, &text, photo, true).await;
    }

    async fn finish_unverified(&self, sub_id: i64, room: &str, last: Option<&[u8]>, reason: &str) {
        let saved = last.and_then(|j| self.save_photo(sub_id, j));
        let (path, photo) = match &saved {
            Some((p, b)) => (Some(p.as_str()), Some(b.as_slice())),
            None => (None, None),
        };
        let _ = self.store.mark_waiting(sub_id, false, path, Some(reason)).await;
        self.event("unverified", Some(room), None, None, None, Some(reason)).await;
        let text = format!(
            "📦 Ada catatan paket untuk kamar {room} pukul {}, tetapi belum terverifikasi kamera ({reason}). Mohon cek {}.",
            self.hhmm(self.clock.now()),
            self.cfg.package_location
        );
        let n = self.deliver(room, sub_id, &text, photo, false).await;
        let msg = format!(
            "Paket kamar {room} (#{sub_id}) belum terverifikasi: {reason}. {}",
            if n > 0 { "Penghuni sudah diberi tahu." } else { "Kamar belum punya penghuni terdaftar." }
        );
        self.alert(&format!("door_{}", self.cfg.door_id), AlertType::PackageUnverified, AlertSeverity::Warning, &msg).await;
        let b = Self::close_button(sub_id, "✅ Tutup (sudah diambil)");
        self.notify_managers(&format!("⚠️ {msg}"), photo, Some(&b)).await;
    }

    // --------------------------------------------------- aksi penghuni/pengelola
    async fn may_act(&self, room: Option<&str>, actor: Actor) -> bool {
        match actor {
            Actor::Dashboard => true,
            Actor::Chat(c) => {
                self.managers.contains(&c)
                    || match room {
                        Some(r) => self.store.is_subscriber(r, c).await.unwrap_or(false),
                        None => false,
                    }
            }
        }
    }

    async fn open_sub(&self, id: i64) -> Option<crate::store::Submission> {
        match self.store.get_submission(id).await {
            Ok(Some(s)) if matches!(s.status.as_str(), "waiting" | "verifying" | "disputed") => Some(s),
            _ => None,
        }
    }

    pub async fn on_taken(&self, sub_id: i64, actor: Actor) -> (bool, String) {
        let mut st = self.state.lock().await;
        let Some(sub) = self.open_sub(sub_id).await else {
            return (false, "Catatan ini sudah ditutup sebelumnya.".into());
        };
        if !self.may_act(sub.room_code.as_deref(), actor).await {
            return (false, "Anda tidak berhak menutup catatan ini.".into());
        }
        st.sessions.remove(&sub_id);
        let _ = self.store.mark_taken(sub_id, self.clock.now(), None).await;
        self.delete_photo(sub_id, &sub.photo_path).await;
        self.event("taken", sub.room_code.as_deref(), None, None, None, Some(&format!("by={actor:?}"))).await;
        (true, format!("Tercatat: paket kamar {} sudah diambil.", sub.room_code.unwrap_or_default()))
    }

    pub async fn on_notmine(&self, sub_id: i64, actor: Actor) -> (bool, String) {
        let _st = self.state.lock().await;
        let Some(sub) = self.open_sub(sub_id).await else {
            return (false, "Catatan ini sudah ditutup sebelumnya.".into());
        };
        if !self.may_act(sub.room_code.as_deref(), actor).await {
            return (false, "Anda tidak berhak mengubah catatan ini.".into());
        }
        let room = sub.room_code.clone().unwrap_or_default();
        let _ = self.store.mark_disputed(sub_id, "dibantah penghuni").await;
        self.event("disputed", Some(&room), None, None, None, None).await;
        let msg = format!("Penghuni kamar {room} menyatakan paket #{sub_id} bukan miliknya (mungkin salah kamar). Mohon periksa.");
        self.alert(&format!("door_{}", self.cfg.door_id), AlertType::PackageDisputed, AlertSeverity::Warning, &msg).await;
        let photo = Self::read_photo(&sub.photo_path);
        let b = Self::close_button(sub_id, "✅ Tutup");
        self.notify_managers(&format!("❗ {msg}"), photo.as_deref(), Some(&b)).await;
        (true, "Terima kasih. Pengelola sudah diberi tahu.".into())
    }

    // ---------------------------------------------------- pendaftaran penghuni
    pub async fn create_invite(&self, room: &str) -> Option<String> {
        let mut st = self.state.lock().await;
        if st.rooms.is_empty() || st.last_rooms_refresh == 0.0 {
            self.refresh_rooms_locked(&mut st).await;
        }
        if !st.rooms.iter().any(|r| r == room) {
            return None;
        }
        // 6 karakter dari alfabet 32 huruf/angka tanpa yang membingungkan (0/O, 1/I).
        let bytes = uuid::Uuid::new_v4();
        let code: String = bytes.as_bytes()[..6].iter().map(|b| CODE_ALPHABET[(*b % 32) as usize] as char).collect();
        self.store.add_invite(&code, room, self.clock.now()).await.ok()?;
        Some(code)
    }

    pub async fn register(&self, chat_id: i64, code: &str) -> (bool, String) {
        let _st = self.state.lock().await;
        let ttl = self.cfg.retention.invite_ttl_h as f64 * 3600.0;
        match self.store.take_invite(&code.trim().to_uppercase(), self.clock.now(), ttl).await {
            Ok(Some(room)) => {
                if self.store.add_subscriber(&room, chat_id).await.is_err() {
                    return (false, "Gagal menyimpan pendaftaran. Coba lagi.".into());
                }
                self.event("registered", Some(&room), None, None, None, None).await;
                (true, room)
            }
            _ => (false, "Kode tidak valid, sudah dipakai, atau kedaluwarsa. Minta kode baru ke pengelola.".into()),
        }
    }

    pub async fn unregister(&self, chat_id: i64) -> Vec<String> {
        let _st = self.state.lock().await;
        let rooms = self.store.deactivate_chat(chat_id).await.unwrap_or_default();
        for r in &rooms {
            self.event("unregistered", Some(r), None, None, None, None).await;
        }
        rooms
    }

    // ------------------------------------------------------- pengamatan zona
    /// Tutup otomatis paket TERVERIFIKASI bila zona terlihat kosong beberapa kali berturut-turut.
    async fn observe_zone(&self, st: &mut State, state: &str, now: f64) {
        let z = &self.cfg.zone;
        st.empty_hits = if state == "kosong" { st.empty_hits + 1 } else { 0 };
        if st.empty_hits < z.empty_hits_to_close {
            return;
        }
        st.empty_hits = 0;
        for sub in self.store.waiting(true).await.unwrap_or_default() {
            if now - sub.ts < z.min_age_autoclose_s {
                continue; // jangan menutup paket yang baru saja tercatat
            }
            let room = sub.room_code.clone().unwrap_or_default();
            let _ = self.store.mark_taken(sub.id, now, Some("ditutup otomatis (zona kosong)")).await;
            self.delete_photo(sub.id, &sub.photo_path).await;
            self.event("auto_close", Some(&room), None, None, None, None).await;
            for cid in self.store.active_chats(&room).await.unwrap_or_default() {
                self.send(cid, &format!("✅ Paket kamar {room} sudah tidak terlihat di area paket, dicatat sudah diambil. Jika keliru, hubungi pengelola."), None, None).await;
            }
        }
    }

    // ------------------------------------------------------------------ tick
    /// Dipanggil tiap ~1 detik dari main.
    pub async fn tick(&self) {
        let mut st = self.state.lock().await;
        let now = self.clock.now();
        let z = &self.cfg.zone;

        let ids: Vec<i64> = st.sessions.keys().copied().collect();
        for id in ids {
            let Some(mut s) = st.sessions.remove(&id) else { continue };
            let mut outcome = Poll::Pending;
            if now >= s.next_poll {
                outcome = self.poll(&mut s, now).await;
            }
            if matches!(outcome, Poll::Pending) && now - s.start >= z.verify_timeout_s {
                outcome = Poll::Failed("perubahan di area paket tidak terdeteksi");
            }
            match outcome {
                Poll::Pending => {
                    st.sessions.insert(id, s);
                }
                Poll::Verified => self.finish_verified(&s, now).await,
                Poll::Failed(reason) => self.finish_unverified(id, &s.room, Some(&s.last), reason).await,
            }
        }

        if st.sessions.is_empty() && now - st.last_zone_check >= z.idle_check_s && self.detector.calibrated() && !self.store.waiting(true).await.unwrap_or_default().is_empty() {
            st.last_zone_check = now;
            if let Some(cur) = self.camera.capture().await {
                match self.detector.classify(&cur) {
                    Ok((state, score)) => {
                        self.event("zone_idle", None, Some(state.as_str()), Some(score), None, None).await;
                        self.observe_zone(&mut st, state.as_str(), now).await;
                    }
                    Err(e) => tracing::warn!("klasifikasi idle gagal: {e}"),
                }
            }
        }

        if now - st.last_rooms_refresh >= 300.0 {
            self.refresh_rooms_locked(&mut st).await;
        }
        if now - st.last_housekeeping >= 60.0 {
            st.last_housekeeping = now;
            self.reminders(now).await;
            self.daily_summary(now).await;
        }
        if now - st.last_purge >= 3600.0 {
            st.last_purge = now;
            self.purge(now).await;
        }
    }

    async fn reminders(&self, now: f64) {
        let r = &self.cfg.retention;
        for sub in self.store.due_reminders(now, r.reminder_after_h as f64 * 3600.0, r.reminder_max as i64).await.unwrap_or_default() {
            let room = sub.room_code.clone().unwrap_or_default();
            let hours = ((now - sub.ts) / 3600.0) as i64;
            let b = Self::package_buttons(sub.id);
            for cid in self.store.active_chats(&room).await.unwrap_or_default() {
                self.send(cid, &format!("⏰ Pengingat: paket kamar {room} tercatat {hours} jam lalu dan belum ditandai diambil."), None, Some(&b)).await;
            }
            let _ = self.store.set_reminded(sub.id, now, sub.remind_count + 1).await;
            self.event("reminder", Some(&room), None, None, None, None).await;
        }
    }

    async fn daily_summary(&self, now: f64) {
        let Some(local) = DateTime::<Utc>::from_timestamp(now as i64, 0).map(|d| d.with_timezone(&self.tz)) else { return };
        if local.format("%H").to_string().parse::<u32>().unwrap_or(0) < self.cfg.summary_hour {
            return;
        }
        let today = local.format("%Y-%m-%d").to_string();
        if self.store.get_setting("summary_date").await.ok().flatten().as_deref() == Some(today.as_str()) {
            return;
        }
        let _ = self.store.set_setting("summary_date", &today).await;
        let start = local.date_naive().and_hms_opt(0, 0, 0).and_then(|n| n.and_local_timezone(self.tz).single()).map(|d| d.timestamp() as f64).unwrap_or(0.0);
        if let Ok(s) = self.store.stats_since(start).await {
            self.notify_managers(
                &format!("📊 Ringkasan {today}: {} kunjungan tercatat, {} paket ({} terverifikasi kamera). Menunggu diambil: {}.", s.total, s.paket, s.verified, s.waiting),
                None,
                None,
            )
            .await;
        }
    }

    async fn purge(&self, now: f64) {
        let cutoff = now - self.cfg.retention.photo_days as f64 * 86400.0;
        for (id, path) in self.store.old_photos(cutoff).await.unwrap_or_default() {
            self.delete_photo(id, &Some(path)).await;
            self.event("photo_purged", None, None, None, None, Some(&id.to_string())).await;
        }
    }

    // ------------------------------------------------------------- teks bantu
    pub async fn status(&self) -> Status {
        let st = self.state.lock().await;
        let mode = self.mode().await;
        let hub_ver = self.cfg.device_payload(&mode, &st.rooms)["ver"].as_str().unwrap_or("").to_string();
        Status {
            door_id: self.cfg.door_id.clone(),
            mode,
            node_online: st.device_online,
            node_ip: self.device_ip.read().ok().and_then(|g| g.clone()).unwrap_or_default(),
            node_cfg_ver: st.device_cfg_ver.clone().unwrap_or_default(),
            hub_cfg_ver: hub_ver,
            calibrated: self.detector.calibrated(),
            waiting: self.store.waiting(false).await.map(|v| v.len() as i64).unwrap_or(0),
            verifying: st.sessions.len() as i64,
            camera_ok: st.device_online && st.camera.ok,
            camera_sensor: st.camera.sensor.clone(),
            camera_error: st.camera.error.clone(),
            stream_url: st.camera.stream_url.clone(),
        }
    }

    pub async fn status_text(&self) -> String {
        let s = self.status().await;
        let sync = if s.node_cfg_ver == s.hub_cfg_ver { "sinkron".to_string() } else { format!("node: {}, hub: {}", s.node_cfg_ver, s.hub_cfg_ver) };
        format!(
            "Pintu {}: node {} (IP {})\nKamera: {}\nMode: {}\nKonfigurasi: {sync}\nZona paket: {}\nPaket menunggu: {}\nVerifikasi berjalan: {}",
            s.door_id,
            if s.node_online { "online" } else { "OFFLINE" },
            if s.node_ip.is_empty() { "-" } else { &s.node_ip },
            if s.camera_ok {
                format!("OK ({})", s.camera_sensor)
            } else if s.camera_error.is_empty() {
                "GAGAL".to_string()
            } else {
                format!("GAGAL ({})", s.camera_error)
            },
            s.mode,
            if s.calibrated { "terkalibrasi" } else { "BELUM dikalibrasi (/kalibrasi)" },
            s.waiting,
            s.verifying
        )
    }

    pub async fn waiting_text(&self) -> String {
        let rows = self.store.waiting(false).await.unwrap_or_default();
        if rows.is_empty() {
            return "Tidak ada paket yang menunggu.".into();
        }
        rows.iter()
            .map(|r| format!("#{} kamar {} - {} - {}", r.id, r.room_code.clone().unwrap_or_default(), self.hhmm(r.ts), if r.verified == 1 { "terverifikasi" } else { "belum terverifikasi" }))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
