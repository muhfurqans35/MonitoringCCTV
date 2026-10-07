//! Tes: port dari suite Python yang sudah lulus, ditambah tes integrasi dengan tabel smart-kost.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde_json::{json, Value};
use tonic::async_trait;

use crate::bot::{BotApi, BotRunner};
use crate::config::DoorbellConfig;
use crate::detector::{crop_roi_jpeg, synth::make_jpeg, Detector};
use crate::hub::{Actor, Hub, HubDeps};
use crate::store::Store;
use crate::traits::{Buttons, Camera, Clock, Notifier, Publisher};

const MGR: i64 = 111;
const RES: i64 = 222;
const OTHER: i64 = 333;
const ROI: [f64; 4] = [0.2, 0.3, 0.6, 0.6];
const BASE_TOML: &str = include_str!("../../../config/doorbell.toml");

// ---------------------------------------------------------------- tiruan
struct FakeClock(Mutex<f64>);
impl Clock for FakeClock {
    fn now(&self) -> f64 {
        *self.0.lock().unwrap()
    }
}

#[derive(Default)]
struct FakePub(Mutex<Vec<(String, Value, bool)>>);
#[async_trait]
impl Publisher for FakePub {
    async fn publish(&self, topic: &str, payload: &Value, retain: bool) {
        self.0.lock().unwrap().push((topic.to_string(), payload.clone(), retain));
    }
}

#[derive(Debug, Clone)]
struct Sent {
    chat: i64,
    text: String,
    photo: Option<Vec<u8>>,
    buttons: Vec<String>,
}
#[derive(Default)]
struct FakeNotifier(Mutex<Vec<Sent>>);
#[async_trait]
impl Notifier for FakeNotifier {
    async fn send(&self, chat_id: i64, text: &str, photo: Option<&[u8]>, buttons: Option<&Buttons>) {
        self.0.lock().unwrap().push(Sent {
            chat: chat_id,
            text: text.to_string(),
            photo: photo.map(|p| p.to_vec()),
            buttons: buttons.map(|b| b.iter().flatten().map(|x| x.data.clone()).collect()).unwrap_or_default(),
        });
    }
}
impl FakeNotifier {
    fn to(&self, chat: i64) -> Vec<Sent> {
        self.0.lock().unwrap().iter().filter(|s| s.chat == chat).cloned().collect()
    }
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

struct FakeCamera {
    scene: Mutex<String>,
    brightness: Mutex<f64>,
    fail: AtomicBool,
    seed: AtomicU32,
}
impl FakeCamera {
    fn set(&self, scene: &str) {
        *self.scene.lock().unwrap() = scene.to_string();
    }
}
#[async_trait]
impl Camera for FakeCamera {
    async fn capture(&self) -> Option<Vec<u8>> {
        if self.fail.load(Ordering::SeqCst) {
            return None;
        }
        let scene = self.scene.lock().unwrap().clone();
        let b = *self.brightness.lock().unwrap();
        Some(make_jpeg(&scene, b, self.seed.fetch_add(1, Ordering::SeqCst)))
    }
}

struct Env {
    hub: Arc<Hub>,
    cam: Arc<FakeCamera>,
    notif: Arc<FakeNotifier>,
    publ: Arc<FakePub>,
    clock: Arc<FakeClock>,
    store: Store,
    dir: PathBuf,
}
impl Env {
    async fn advance_ticks(&self, n: usize, step: f64) {
        for _ in 0..n {
            *self.clock.0.lock().unwrap() += step;
            self.hub.tick().await;
        }
    }
    fn advance(&self, s: f64) {
        *self.clock.0.lock().unwrap() += s;
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn test_cfg() -> DoorbellConfig {
    let mut cfg: DoorbellConfig = toml::from_str(BASE_TOML).expect("doorbell.toml bawaan harus bisa di-parse");
    cfg.zone.roi = ROI;
    cfg.zone.verify_timeout_s = 40.0;
    cfg.managers = vec![MGR];
    cfg
}

async fn env() -> Env {
    let dir = std::env::temp_dir().join(format!("doorbell-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let pool = common::db::init_pool(&format!("sqlite://{}/t.db", dir.display())).await.unwrap();
    for i in 1..=12 {
        sqlx::query("INSERT INTO rooms(code) VALUES (?)").bind(i.to_string()).execute(&pool).await.unwrap();
    }
    let store = Store::new(pool);
    let cfg = Arc::new(test_cfg());
    cfg.validate().unwrap();
    let cam = Arc::new(FakeCamera { scene: Mutex::new("empty".into()), brightness: Mutex::new(1.0), fail: AtomicBool::new(false), seed: AtomicU32::new(1) });
    let notif = Arc::new(FakeNotifier::default());
    let publ = Arc::new(FakePub::default());
    let clock = Arc::new(FakeClock(Mutex::new(1_700_000_000.0)));
    let detector = Detector::new(&cfg.zone, Some(dir.join("ref.jpg")));
    let hub = Arc::new(
        Hub::new(
            cfg.clone(),
            store.clone(),
            detector,
            vec![MGR],
            dir.join("photos"),
            HubDeps {
                camera: cam.clone(),
                notifier: notif.clone(),
                publisher: publ.clone(),
                clock: clock.clone(),
                device_ip: Arc::new(RwLock::new(None)),
            },
        )
        .unwrap(),
    );
    hub.refresh_rooms().await;
    Env { hub, cam, notif, publ, clock, store, dir }
}

static SID: AtomicU32 = AtomicU32::new(0);

async fn submit(e: &Env, purpose: &str, room: Option<&str>, extra: Value) -> Value {
    let mut d = json!({ "sid": format!("t-{}", SID.fetch_add(1, Ordering::SeqCst)), "purpose": purpose });
    if let Some(r) = room {
        d["room"] = json!(r);
    }
    if let Value::Object(m) = extra {
        for (k, v) in m {
            d[k] = v;
        }
    }
    e.hub.handle_message("kost/door/depan/event/submit", d.to_string().as_bytes()).await;
    d
}

async fn register(e: &Env, room: &str, chat: i64) {
    let code = e.hub.create_invite(room).await.expect("kamar harus ada");
    let (ok, r) = e.hub.register(chat, &code).await;
    assert!(ok && r == room, "{r}");
}

async fn verified_package(e: &Env) {
    register(e, "7", RES).await;
    submit(e, "paket", Some("7"), json!({})).await;
    e.cam.set("package");
    e.advance_ticks(4, 4.0).await;
}

async fn one_sub(e: &Env) -> crate::store::Submission {
    e.store.get_submission(1).await.unwrap().unwrap()
}

// ------------------------------------------------------------------ config
#[test]
fn default_config_is_valid() {
    let cfg = test_cfg();
    cfg.validate().unwrap();
    assert_eq!(cfg.door_id, "depan");
    assert_eq!(cfg.purposes[0].id, "paket"); // urutan menu terjaga (array of tables)
}

fn problems(mutate: impl FnOnce(&mut DoorbellConfig)) -> Vec<String> {
    let mut cfg = test_cfg();
    mutate(&mut cfg);
    cfg.validate().expect_err("harus ditolak").0
}

#[test]
fn config_rejections() {
    let cases: Vec<(&str, Box<dyn FnOnce(&mut DoorbellConfig)>)> = vec![
        ("terlarang", Box::new(|c| { c.texts.get_mut("normal").unwrap().insert("paket".into(), "Pemilik sedang keluar".into()); })),
        ("ASCII", Box::new(|c| { c.texts.get_mut("normal").unwrap().insert("paket".into(), "Terima kasih 😀".into()); })),
        ("texts.sibuk.lainnya", Box::new(|c| { c.texts.get_mut("sibuk").unwrap().remove("lainnya"); })),
        ("default_mode", Box::new(|c| c.default_mode = "libur".into())),
        ("terlalu besar", Box::new(|c| c.zone.roi = [0.0, 0.0, 1.0, 1.0])),
        ("luar frame", Box::new(|c| c.zone.roi = [0.8, 0.8, 0.5, 0.5])),
        ("needs_room", Box::new(|c| c.purposes[3].notify = crate::config::Notify::Resident)),
        ("maksimal 4", Box::new(|c| { let p = c.purposes[0].clone(); for i in 0..2 { let mut x = p.clone(); x.id = format!("x{i}"); c.purposes.push(x); } })),
        ("duplikat", Box::new(|c| c.purposes[1].id = "paket".into())),
        ("door_id", Box::new(|c| c.door_id = "Depan Pintu".into())),
    ];
    for (needle, f) in cases {
        let p = problems(f);
        assert!(p.iter().any(|x| x.contains(needle)), "{needle}: {p:?}");
    }
}

#[test]
fn device_payload_version_tracks_content() {
    let cfg = test_cfg();
    let rooms: Vec<String> = (1..=3).map(|i| i.to_string()).collect();
    let a = cfg.device_payload("normal", &rooms);
    let b = cfg.device_payload("sibuk", &rooms);
    let c = cfg.device_payload("normal", &["1".to_string()]);
    assert_ne!(a["ver"], b["ver"]);
    assert_ne!(a["ver"], c["ver"]);
    assert_eq!(a["ver"], cfg.device_payload("normal", &rooms)["ver"]);
    assert_eq!(a["purposes"][0]["id"], "paket");
    assert_eq!(a["rooms"][0], "1");
}

// ---------------------------------------------------------------- detector
#[test]
fn detector_states_and_exposure_invariance() {
    let dir = std::env::temp_dir().join(format!("doorbell-det-{}", uuid::Uuid::new_v4()));
    let cfg = test_cfg();
    let d = Detector::new(&cfg.zone, Some(dir.join("ref.jpg")));
    assert_eq!(d.classify(&make_jpeg("empty", 1.0, 1)).unwrap().0.as_str(), "tidak_terkalibrasi");
    d.set_reference(&make_jpeg("empty", 1.0, 2)).unwrap();
    assert_eq!(d.classify(&make_jpeg("empty", 1.0, 3)).unwrap().0.as_str(), "kosong");
    assert_eq!(d.classify(&make_jpeg("package", 1.0, 4)).unwrap().0.as_str(), "ada_paket");
    assert_eq!(d.classify(&make_jpeg("person", 1.0, 5)).unwrap().0.as_str(), "terhalang");
    for b in [0.75, 1.25] {
        // auto-exposure berubah, isi zona tetap
        assert_eq!(d.classify(&make_jpeg("empty", b, 6)).unwrap().0.as_str(), "kosong", "{b}");
        assert_eq!(d.classify(&make_jpeg("package", b, 7)).unwrap().0.as_str(), "ada_paket", "{b}");
    }
    assert!(d.change(&make_jpeg("empty", 1.0, 8), &make_jpeg("empty", 1.0, 9)).unwrap() < 0.01);
    assert!(d.change(&make_jpeg("empty", 1.0, 8), &make_jpeg("package", 1.0, 9)).unwrap() > 0.05);
    assert!(d.classify(b"bukan gambar").is_err());
    // referensi tersimpan & dimuat ulang
    assert!(Detector::new(&cfg.zone, Some(dir.join("ref.jpg"))).calibrated());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn photo_is_roi_crop_only() {
    let crop = crop_roi_jpeg(&make_jpeg("package", 1.0, 1), &ROI, 80).unwrap();
    let img = image::load_from_memory(&crop).unwrap();
    assert_eq!((img.width(), img.height()), (192, 144)); // 60% x 60% dari 320x240
}

// ------------------------------------------------------- alur paket utama
#[tokio::test]
async fn package_verified_flow() {
    let e = env().await;
    register(&e, "7", RES).await;
    submit(&e, "paket", Some("7"), json!({})).await;
    e.cam.set("person"); // kurir masih berdiri di zona
    e.advance_ticks(3, 4.0).await;
    assert!(e.notif.to(RES).is_empty());
    e.cam.set("package"); // kurir pergi, paket tertinggal
    e.advance_ticks(4, 4.0).await;
    let m = e.notif.to(RES);
    assert_eq!(m.len(), 1);
    assert!(m[0].text.contains("tercatat") && m[0].photo.is_some());
    assert_eq!(m[0].buttons, vec!["taken:1", "notmine:1"]);
    let s = one_sub(&e).await;
    assert_eq!((s.verified, s.status.as_str()), (1, "waiting"));
    assert!(e.notif.to(MGR).is_empty(), "kasus normal tidak membanjiri pengelola");
}

#[tokio::test]
async fn unverified_when_nothing_changes_and_alert_recorded() {
    let e = env().await;
    register(&e, "7", RES).await;
    submit(&e, "paket", Some("7"), json!({})).await;
    e.advance_ticks(12, 4.0).await;
    assert!(e.notif.to(RES)[0].text.contains("belum terverifikasi"));
    assert!(e.notif.to(MGR)[0].text.contains("belum terverifikasi"));
    let s = one_sub(&e).await;
    assert_eq!((s.status.as_str(), s.verified), ("waiting", 0));
    // integrasi: muncul di alert_events yang dibaca dashboard
    let t: String = sqlx::query_scalar("SELECT alert_type FROM alert_events").fetch_one(&e.store.pool).await.unwrap();
    assert_eq!(t, "package_unverified");
}

#[tokio::test]
async fn camera_failures_are_recorded_not_lost() {
    let e = env().await;
    register(&e, "7", RES).await;
    e.cam.fail.store(true, Ordering::SeqCst);
    submit(&e, "paket", Some("7"), json!({})).await;
    let s = one_sub(&e).await;
    assert_eq!((s.status.as_str(), s.verified), ("waiting", 0));
    assert!(s.note.unwrap().contains("kamera"));
    assert_eq!(e.notif.to(RES).len(), 1);

    // kamera mati di tengah verifikasi
    e.cam.fail.store(false, Ordering::SeqCst);
    submit(&e, "paket", Some("7"), json!({})).await;
    e.cam.fail.store(true, Ordering::SeqCst);
    e.advance_ticks(4, 4.0).await;
    let s2 = e.store.get_submission(2).await.unwrap().unwrap();
    assert_eq!((s2.status.as_str(), s2.verified), ("waiting", 0));
}

#[tokio::test]
async fn unregistered_room_goes_to_manager() {
    let e = env().await;
    submit(&e, "paket", Some("5"), json!({})).await;
    e.cam.set("package");
    e.advance_ticks(4, 4.0).await;
    let m = e.notif.to(MGR);
    assert_eq!(m.len(), 1);
    assert!(m[0].text.contains("belum punya penghuni terdaftar"));
}

#[tokio::test]
async fn duplicate_sid_ignored_and_late_replay_skips_camera() {
    let e = env().await;
    register(&e, "7", RES).await;
    let d = submit(&e, "paket", Some("7"), json!({})).await;
    e.hub.handle_message("kost/door/depan/event/submit", d.to_string().as_bytes()).await;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM doorbell_submissions").fetch_one(&e.store.pool).await.unwrap();
    assert_eq!(n, 1);

    let e2 = env().await;
    register(&e2, "7", RES).await;
    submit(&e2, "paket", Some("7"), json!({"queued": true})).await;
    let m = e2.notif.to(RES);
    assert_eq!(m.len(), 1);
    assert!(m[0].text.contains("offline"));
    assert_eq!(one_sub(&e2).await.late, 1);
}

#[tokio::test]
async fn invalid_submissions_rejected() {
    let e = env().await;
    for d in [
        json!({"sid": "a", "purpose": "hantu", "room": "7"}),
        json!({"sid": "b", "purpose": "paket", "room": "99"}), // tidak ada di tabel rooms
        json!({"purpose": "paket", "room": "7"}),
    ] {
        e.hub.handle_message("kost/door/depan/event/submit", d.to_string().as_bytes()).await;
    }
    e.hub.handle_message("kost/door/depan/event/submit", b"bukan json").await;
    e.hub.handle_message("kost/door/lain/event/submit", br#"{"sid":"z","purpose":"paket","room":"7"}"#).await;
    e.hub.handle_message("door/depan/event/submit", br#"{"sid":"y","purpose":"paket","room":"7"}"#).await; // namespace lama
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM doorbell_submissions").fetch_one(&e.store.pool).await.unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn manager_purposes_and_food() {
    let e = env().await;
    register(&e, "7", RES).await;
    submit(&e, "pengelola", None, json!({})).await;
    assert!(e.notif.to(MGR)[0].text.contains("Hubungi Pengelola"));
    submit(&e, "makanan", Some("7"), json!({})).await;
    let m = e.notif.to(RES);
    assert_eq!(m.len(), 1);
    assert!(m[0].photo.is_none());
}

// ------------------------------------------------- tombol & kewenangan
#[tokio::test]
async fn taken_authorization_and_photo_deleted() {
    let e = env().await;
    verified_package(&e).await;
    let photo = one_sub(&e).await.photo_path.unwrap();
    assert!(std::path::Path::new(&photo).exists());
    assert!(!e.hub.on_taken(1, Actor::Chat(OTHER)).await.0);
    assert!(e.hub.on_taken(1, Actor::Chat(RES)).await.0);
    assert_eq!(one_sub(&e).await.status, "taken");
    assert!(!std::path::Path::new(&photo).exists() && one_sub(&e).await.photo_path.is_none());
    assert!(!e.hub.on_taken(1, Actor::Chat(RES)).await.0, "sudah ditutup");
}

#[tokio::test]
async fn dashboard_actor_can_close() {
    let e = env().await;
    verified_package(&e).await;
    assert!(e.hub.on_taken(1, Actor::Dashboard).await.0);
    assert_eq!(one_sub(&e).await.status, "taken");
}

#[tokio::test]
async fn not_mine_escalates_and_records_alert() {
    let e = env().await;
    verified_package(&e).await;
    assert!(e.hub.on_notmine(1, Actor::Chat(RES)).await.0);
    assert_eq!(one_sub(&e).await.status, "disputed");
    let m = e.notif.to(MGR);
    assert_eq!(m.len(), 1);
    assert!(m[0].text.contains("bukan miliknya") && m[0].photo.is_some());
    let t: String = sqlx::query_scalar("SELECT alert_type FROM alert_events").fetch_one(&e.store.pool).await.unwrap();
    assert_eq!(t, "package_disputed");
    assert!(e.hub.on_taken(1, Actor::Chat(MGR)).await.0);
}

// ---------------------------------------- penutupan otomatis, pengingat, retensi
#[tokio::test]
async fn auto_close_needs_calibration_age_and_verified() {
    let e = env().await;
    verified_package(&e).await;
    e.cam.set("empty");
    e.advance(400.0);
    e.advance_ticks(3, 61.0).await;
    assert_eq!(one_sub(&e).await.status, "waiting", "belum dikalibrasi: tidak ditutup");
    assert!(e.hub.calibrate().await.0);
    e.advance_ticks(3, 61.0).await;
    assert_eq!(one_sub(&e).await.status, "taken");
    assert!(e.notif.to(RES).iter().any(|m| m.text.contains("sudah tidak terlihat")));
}

#[tokio::test]
async fn auto_close_not_for_unverified_or_too_recent() {
    let e = env().await;
    register(&e, "7", RES).await;
    submit(&e, "paket", Some("7"), json!({})).await;
    e.advance_ticks(12, 4.0).await; // tidak terverifikasi
    e.hub.calibrate().await;
    e.advance(400.0);
    e.advance_ticks(4, 61.0).await;
    assert_eq!(one_sub(&e).await.status, "waiting");
    let now = e.clock.now();
    sqlx::query("UPDATE doorbell_submissions SET verified = 1, ts = ? WHERE id = 1").bind(now - 10.0).execute(&e.store.pool).await.unwrap();
    e.advance_ticks(4, 61.0).await;
    assert_eq!(one_sub(&e).await.status, "waiting", "terlalu baru");
}

#[tokio::test]
async fn zone_event_from_edge_node_feeds_autoclose() {
    let e = env().await;
    verified_package(&e).await;
    e.advance(400.0);
    for _ in 0..2 {
        e.hub.handle_message("kost/door/depan/event/zone", br#"{"state":"kosong","score":0.0}"#).await;
    }
    assert_eq!(one_sub(&e).await.status, "taken");
}

#[tokio::test]
async fn reminders_are_limited() {
    let e = env().await;
    verified_package(&e).await;
    e.notif.clear();
    e.advance_ticks(8, 6.0 * 3600.0 + 5.0).await;
    let n = e.notif.to(RES).iter().filter(|m| m.text.contains("Pengingat")).count();
    assert_eq!(n as u32, e.hub.cfg.retention.reminder_max);
}

#[tokio::test]
async fn photo_retention_purge() {
    let e = env().await;
    verified_package(&e).await;
    let photo = one_sub(&e).await.photo_path.unwrap();
    e.advance_ticks(1, 8.0 * 86400.0).await;
    assert!(!std::path::Path::new(&photo).exists());
    assert!(one_sub(&e).await.photo_path.is_none());
}

#[tokio::test]
async fn daily_summary_sent_once() {
    let e = env().await;
    let local = e.clock.now() + 480.0 * 60.0;
    let midnight_utc = (local / 86400.0).floor() * 86400.0 - 480.0 * 60.0;
    *e.clock.0.lock().unwrap() = midnight_utc + e.hub.cfg.summary_hour as f64 * 3600.0 + 60.0;
    e.hub.tick().await;
    e.hub.tick().await;
    e.advance_ticks(1, 120.0).await;
    assert_eq!(e.notif.to(MGR).iter().filter(|m| m.text.contains("Ringkasan")).count(), 1);
}

// ------------------------------------------------- mode, config, integrasi
#[tokio::test]
async fn config_comes_from_rooms_table_and_mode_republishes() {
    let e = env().await;
    e.hub.publish_config().await;
    let (topic, payload, retain) = e.publ.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!(topic, "kost/door/depan/cmd/config");
    assert!(retain);
    assert_eq!(payload["rooms"].as_array().unwrap().len(), 12);
    let v1 = payload["ver"].clone();

    sqlx::query("INSERT INTO rooms(code) VALUES ('K13')").execute(&e.store.pool).await.unwrap();
    assert!(e.hub.set_mode("sibuk").await && !e.hub.set_mode("nope").await);
    let (_, p2, _) = e.publ.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!(p2["mode"], "sibuk");
    assert_ne!(p2["ver"], v1);
    assert!(p2["rooms"].as_array().unwrap().iter().any(|r| r == "K13"), "kamar baru ikut terkirim ke layar");
    assert_eq!(e.hub.mode().await, "sibuk");
}

#[tokio::test]
async fn node_status_updates_devices_table_and_alerts_on_offline() {
    let e = env().await;
    e.hub.handle_message("kost/door/depan/status", br#"{"online":true,"ip":"192.168.1.50","ver":"abc"}"#).await;
    let st: String = sqlx::query_scalar("SELECT status FROM devices WHERE device_uid = 'door_depan'").fetch_one(&e.store.pool).await.unwrap();
    assert_eq!(st, "online");
    let t: String = sqlx::query_scalar("SELECT device_type FROM devices WHERE device_uid = 'door_depan'").fetch_one(&e.store.pool).await.unwrap();
    assert_eq!(t, "door_node");
    let s = e.hub.status().await;
    assert!(s.node_online && s.node_ip == "192.168.1.50");
    assert!(e.hub.status_text().await.contains("online"));

    e.hub.handle_message("kost/door/depan/status", br#"{"online":false}"#).await;
    let st: String = sqlx::query_scalar("SELECT status FROM devices WHERE device_uid = 'door_depan'").fetch_one(&e.store.pool).await.unwrap();
    assert_eq!(st, "offline");
    assert!(e.hub.status_text().await.contains("OFFLINE"));
    let a: String = sqlx::query_scalar("SELECT alert_type FROM alert_events").fetch_one(&e.store.pool).await.unwrap();
    assert_eq!(a, "device_offline");
    assert!(e.notif.to(MGR).iter().any(|m| m.text.contains("offline")));
}

#[tokio::test]
async fn camera_status_from_mqtt_is_exposed_and_failure_alerts() {
    let e = env().await;
    e.hub
        .handle_message(
            "kost/door/depan/status",
            br#"{"online":true,"ip":"192.168.1.50","ver":"abc","cam":true,"cam_sensor":"OV5640","cam_err":"","stream_url":"http://192.168.1.50:81/stream"}"#,
        )
        .await;
    let s = e.hub.status().await;
    assert!(s.camera_ok && s.camera_sensor == "OV5640" && s.stream_url.ends_with(":81/stream"));
    assert!(e.hub.status_text().await.contains("Kamera: OK (OV5640)"));

    // Node tetap online tapi kamera berhenti -> status gagal + alert dashboard.
    e.hub
        .handle_message("kost/door/depan/status", br#"{"online":true,"ip":"192.168.1.50","cam":false,"cam_err":"init gagal 0x105"}"#)
        .await;
    let s = e.hub.status().await;
    assert!(s.node_online && !s.camera_ok && s.camera_error == "init gagal 0x105");
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_events WHERE message LIKE '%Kamera%0x105%'").fetch_one(&e.store.pool).await.unwrap();
    assert_eq!(n, 1);

    // Node offline -> kamera ikut dianggap tidak tersedia.
    e.hub.handle_message("kost/door/depan/status", br#"{"online":true,"cam":true}"#).await;
    e.hub.handle_message("kost/door/depan/status", br#"{"online":false}"#).await;
    assert!(!e.hub.status().await.camera_ok);
}

#[tokio::test]
async fn snapshot_is_cached_briefly_and_reports_failure() {
    let e = env().await;
    let (a, t1) = e.hub.snapshot().await.expect("kamera tiruan harus memberi foto");
    assert!(a.starts_with(&[0xFF, 0xD8]), "harus JPEG");
    // Dalam jendela cache: foto yang sama (node tidak diminta ulang).
    e.advance(0.3);
    let (b, t2) = e.hub.snapshot().await.unwrap();
    assert_eq!(a, b);
    assert_eq!(t1, t2);
    // Lewat jendela cache: ambil ulang.
    e.advance(1.0);
    let (_, t3) = e.hub.snapshot().await.unwrap();
    assert!(t3 > t1);
    // Kamera gagal -> Err dengan pesan, bukan panic / foto basi.
    e.cam.fail.store(true, Ordering::SeqCst);
    e.advance(1.0);
    assert!(e.hub.snapshot().await.is_err());
}

#[tokio::test]
async fn foto_command_sends_door_photo_to_manager_only() {
    let e = env().await;
    let api = Arc::new(FakeApi::default());
    let bot = BotRunner::new(api.clone(), e.hub.clone());
    bot.handle_update(&msg(MGR, "/foto", "private")).await;
    let sent = e.notif.to(MGR);
    assert!(sent.iter().any(|m| m.text.contains("Kamera pintu") && m.photo.is_some()), "{sent:?}");
    // Penghuni biasa tidak boleh melihat kamera pintu.
    bot.handle_update(&msg(OTHER, "/foto", "private")).await;
    assert!(e.notif.to(OTHER).is_empty());
    assert!(last(&api).contains("hanya untuk pengelola"));
}

#[tokio::test]
async fn invite_is_one_time_and_only_for_known_rooms() {
    let e = env().await;
    assert!(e.hub.create_invite("99").await.is_none());
    let code = e.hub.create_invite("7").await.unwrap();
    assert_eq!(code.len(), 6);
    assert!(e.hub.register(RES, &code).await.0);
    assert!(!e.hub.register(OTHER, &code).await.0);
    assert!(!e.hub.register(OTHER, "XXXXXX").await.0);
    assert_eq!(e.store.active_chats("7").await.unwrap(), vec![RES]);
    assert_eq!(e.hub.unregister(RES).await, vec!["7".to_string()]);
    assert!(e.store.active_chats("7").await.unwrap().is_empty());
}

#[tokio::test]
async fn expired_invite_rejected() {
    let e = env().await;
    let code = e.hub.create_invite("7").await.unwrap();
    e.advance(e.hub.cfg.retention.invite_ttl_h as f64 * 3600.0 + 1.0);
    assert!(!e.hub.register(RES, &code).await.0);
}

// ------------------------------------------------------------------- bot
#[derive(Default)]
struct FakeApi {
    sent: Mutex<Vec<(i64, String)>>,
    answers: Mutex<Vec<String>>,
    cleared: Mutex<Vec<(i64, i64)>>,
}
#[async_trait]
impl BotApi for FakeApi {
    async fn send_message(&self, chat_id: i64, text: &str) {
        self.sent.lock().unwrap().push((chat_id, text.to_string()));
    }
    async fn answer_callback(&self, _id: &str, text: &str) {
        self.answers.lock().unwrap().push(text.to_string());
    }
    async fn clear_buttons(&self, chat_id: i64, message_id: i64) {
        self.cleared.lock().unwrap().push((chat_id, message_id));
    }
}
fn msg(chat: i64, text: &str, kind: &str) -> Value {
    json!({"update_id": 1, "message": {"chat": {"id": chat, "type": kind}, "text": text}})
}
fn cb(chat: i64, data: &str) -> Value {
    json!({"update_id": 2, "callback_query": {"id": "x", "data": data, "from": {"id": chat}, "message": {"chat": {"id": chat}, "message_id": 9}}})
}
fn last(api: &FakeApi) -> String {
    api.sent.lock().unwrap().last().unwrap().1.clone()
}

#[tokio::test]
async fn bot_registration_and_permissions() {
    let e = env().await;
    let api = Arc::new(FakeApi::default());
    let bot = BotRunner::new(api.clone(), e.hub.clone());
    bot.handle_update(&msg(RES, "/kode 7", "private")).await;
    assert!(last(&api).contains("hanya untuk pengelola"));
    bot.handle_update(&msg(MGR, "/kode 99", "private")).await;
    assert!(last(&api).contains("tidak ada"));
    bot.handle_update(&msg(MGR, "/kode 7", "private")).await;
    let code = last(&api).split(": ").nth(1).unwrap().split_whitespace().next().unwrap().to_string();
    bot.handle_update(&msg(RES, &format!("/start {code}"), "private")).await;
    assert!(last(&api).contains("Terdaftar untuk kamar 7"));
    assert_eq!(e.store.active_chats("7").await.unwrap(), vec![RES]);
    bot.handle_update(&msg(OTHER, &format!("/start {code}"), "private")).await;
    assert!(last(&api).contains("tidak valid"));
    bot.handle_update(&msg(RES, "/berhenti", "private")).await;
    assert!(e.store.active_chats("7").await.unwrap().is_empty());
    let n = api.sent.lock().unwrap().len();
    bot.handle_update(&msg(MGR, "/status", "group")).await; // grup diabaikan
    assert_eq!(api.sent.lock().unwrap().len(), n);
}

#[tokio::test]
async fn bot_callbacks_and_mode() {
    let e = env().await;
    verified_package(&e).await;
    let api = Arc::new(FakeApi::default());
    let bot = BotRunner::new(api.clone(), e.hub.clone());
    bot.handle_update(&cb(OTHER, "taken:1")).await;
    assert!(api.answers.lock().unwrap().last().unwrap().contains("tidak berhak"));
    assert!(api.cleared.lock().unwrap().is_empty());
    bot.handle_update(&cb(RES, "taken:1")).await;
    assert!(api.answers.lock().unwrap().last().unwrap().contains("sudah diambil"));
    assert_eq!(*api.cleared.lock().unwrap(), vec![(RES, 9)]);
    bot.handle_update(&cb(RES, "taken:abc")).await;
    assert!(api.answers.lock().unwrap().last().unwrap().contains("tidak dikenal"));
    bot.handle_update(&msg(MGR, "/mode sibuk", "private")).await;
    assert_eq!(e.hub.mode().await, "sibuk");
    bot.handle_update(&msg(MGR, "/paket", "private")).await;
    assert!(last(&api).contains("Tidak ada paket"));
}
