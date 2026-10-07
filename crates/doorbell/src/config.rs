//! Konfigurasi doorbell (config/doorbell.toml) + validasi.
//! Aturan penting: teks layar tidak boleh membocorkan status kost kosong (FR-07),
//! hanya ASCII (font bawaan TFT), dan zona/ROI harus masuk akal.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// FR-07: teks di layar tidak boleh menyiratkan bahwa kost sedang kosong.
pub const FORBIDDEN_PHRASES: [&str; 9] = [
    "kosong",
    "tidak ada orang",
    "tidak ada penghuni",
    "tidak di rumah",
    "tidak di kost",
    "sedang keluar",
    "sedang pergi",
    "ditinggal",
    "sedang tidak ada",
];
pub const MAX_TEXT: usize = 120;
pub const MAX_LABEL: usize = 20;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Notify {
    Resident,
    Manager,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Purpose {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub needs_room: bool,
    #[serde(default = "default_notify")]
    pub notify: Notify,
    #[serde(default)]
    pub verify_zone: bool,
}
fn default_notify() -> Notify {
    Notify::Manager
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiCfg {
    pub idle_timeout_s: u32,
    pub done_screen_s: u32,
}
impl Default for UiCfg {
    fn default() -> Self {
        Self { idle_timeout_s: 30, done_screen_s: 8 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CameraCfg {
    /// Kosong = pakai IP yang dilaporkan node lewat MQTT.
    pub host: String,
    pub port: u16,
    pub token: String,
    pub timeout_s: u64,
}
impl Default for CameraCfg {
    fn default() -> Self {
        Self { host: String::new(), port: 80, token: String::new(), timeout_s: 4 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ZoneCfg {
    pub roi: [f64; 4],
    pub pix_thr: u32,
    pub occupied_frac: f64,
    pub obstructed_frac: f64,
    pub change_frac: f64,
    pub motion_frac: f64,
    pub stable_polls: u32,
    pub poll_interval_s: f64,
    pub verify_timeout_s: f64,
    pub idle_check_s: f64,
    pub empty_hits_to_close: u32,
    pub min_age_autoclose_s: f64,
}
impl Default for ZoneCfg {
    fn default() -> Self {
        Self {
            roi: [0.15, 0.35, 0.70, 0.50],
            pix_thr: 25,
            occupied_frac: 0.03,
            obstructed_frac: 0.60,
            change_frac: 0.03,
            motion_frac: 0.01,
            stable_polls: 2,
            poll_interval_s: 4.0,
            verify_timeout_s: 120.0,
            idle_check_s: 60.0,
            empty_hits_to_close: 2,
            min_age_autoclose_s: 300.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RetentionCfg {
    pub photo_days: u32,
    pub reminder_after_h: u32,
    pub reminder_max: u32,
    pub invite_ttl_h: u32,
}
impl Default for RetentionCfg {
    fn default() -> Self {
        Self { photo_days: 7, reminder_after_h: 6, reminder_max: 2, invite_ttl_h: 24 }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DoorbellConfig {
    pub door_id: String,
    /// Lokasi node untuk tabel devices, mis. "Pintu depan".
    #[serde(default = "default_location")]
    pub location: String,
    /// WITA = 480. Dipakai untuk jam di pesan Telegram (Indonesia tanpa DST).
    #[serde(default = "default_offset")]
    pub utc_offset_minutes: i32,
    pub default_mode: String,
    #[serde(default = "default_summary_hour")]
    pub summary_hour: u32,
    #[serde(default = "default_package_location")]
    pub package_location: String,
    /// chat_id pengelola TAMBAHAN selain TELEGRAM_OWNER_CHAT_ID.
    #[serde(default)]
    pub managers: Vec<i64>,
    pub purposes: Vec<Purpose>,
    /// texts[mode][purpose_id] = teks di layar.
    pub texts: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    pub ui: UiCfg,
    #[serde(default)]
    pub camera: CameraCfg,
    #[serde(default)]
    pub zone: ZoneCfg,
    #[serde(default)]
    pub retention: RetentionCfg,
}
fn default_location() -> String {
    "Pintu depan".into()
}
fn default_offset() -> i32 {
    480
}
fn default_summary_hour() -> u32 {
    20
}
fn default_package_location() -> String {
    "area paket".into()
}

#[derive(Debug)]
pub struct ConfigError(pub Vec<String>);
impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Konfigurasi doorbell tidak valid:")?;
        for p in &self.0 {
            writeln!(f, "- {p}")?;
        }
        Ok(())
    }
}
impl std::error::Error for ConfigError {}

fn check_device_text(where_: &str, s: Option<&String>, problems: &mut Vec<String>) {
    let Some(s) = s else {
        problems.push(format!("{where_}: teks wajib diisi"));
        return;
    };
    if s.trim().is_empty() {
        problems.push(format!("{where_}: teks wajib diisi"));
        return;
    }
    if !s.is_ascii() {
        problems.push(format!("{where_}: hanya karakter ASCII (font TFT bawaan tidak mendukung emoji/aksen)"));
    }
    if s.len() > MAX_TEXT {
        problems.push(format!("{where_}: terlalu panjang ({} > {MAX_TEXT} karakter)", s.len()));
    }
    let low = s.to_lowercase();
    for ph in FORBIDDEN_PHRASES {
        if low.contains(ph) {
            problems.push(format!("{where_}: mengandung frasa terlarang '{ph}' (dapat membocorkan status kost)"));
        }
    }
}

impl DoorbellConfig {
    pub fn parse(toml_text: &str) -> anyhow::Result<Self> {
        let cfg: DoorbellConfig =
            toml::from_str(toml_text).map_err(|e| anyhow::anyhow!("gagal parse doorbell.toml: {e}"))?;
        cfg.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(cfg)
    }

    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| anyhow::anyhow!("gagal baca {:?}: {e}", path.as_ref()))?;
        let mut cfg = Self::parse(&text)?;
        // Rahasia lewat environment (konsisten dengan service lain).
        if let Ok(t) = std::env::var("DOORBELL_CAMERA_TOKEN") {
            if !t.is_empty() {
                cfg.camera.token = t;
            }
        }
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut p: Vec<String> = Vec::new();

        if self.door_id.is_empty()
            || self.door_id.len() > 20
            || !self.door_id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            p.push("door_id harus huruf kecil/angka/-/_ (maks 20 karakter)".into());
        }
        if self.utc_offset_minutes.abs() > 14 * 60 {
            p.push("utc_offset_minutes di luar rentang".into());
        }
        if self.summary_hour > 23 {
            p.push("summary_hour harus 0-23".into());
        }

        if self.purposes.is_empty() {
            p.push("purposes kosong".into());
        }
        if self.purposes.len() > 4 {
            p.push("purposes maksimal 4 (layar hanya memuat 4 tombol)".into());
        }
        let mut seen = std::collections::HashSet::new();
        for pu in &self.purposes {
            let id_ok = !pu.id.is_empty()
                && pu.id.len() <= 16
                && pu.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !id_ok {
                p.push(format!("purpose id '{}' harus huruf kecil/angka/_ (maks 16)", pu.id));
            }
            if !seen.insert(pu.id.clone()) {
                p.push(format!("purpose id '{}' duplikat", pu.id));
            }
            if pu.label.trim().is_empty() || !pu.label.is_ascii() || pu.label.len() > MAX_LABEL {
                p.push(format!("purpose '{}': label wajib ASCII maks {MAX_LABEL} karakter", pu.id));
            }
            if pu.notify == Notify::Resident && !pu.needs_room {
                p.push(format!("purpose '{}': notify resident membutuhkan needs_room = true", pu.id));
            }
            if pu.verify_zone && pu.notify != Notify::Resident {
                p.push(format!("purpose '{}': verify_zone membutuhkan notify = \"resident\"", pu.id));
            }
        }

        if self.texts.is_empty() {
            p.push("texts kosong".into());
        }
        for (mode, mt) in &self.texts {
            for pu in &self.purposes {
                check_device_text(&format!("texts.{mode}.{}", pu.id), mt.get(&pu.id), &mut p);
            }
        }
        if !self.texts.contains_key(&self.default_mode) {
            p.push("default_mode tidak ada di texts".into());
        }

        let z = &self.zone;
        let [x, y, w, h] = z.roi;
        if !(0.0..1.0).contains(&x) || !(0.0..1.0).contains(&y) || w < 0.05 || h < 0.05 || x + w > 1.0001 || y + h > 1.0001 {
            p.push("zone.roi di luar frame atau terlalu kecil".into());
        } else if w * h > 0.6 {
            p.push("zone.roi terlalu besar (>60% frame) sehingga normalisasi cahaya tidak andal".into());
        }
        for (name, v) in [
            ("occupied_frac", z.occupied_frac),
            ("obstructed_frac", z.obstructed_frac),
            ("change_frac", z.change_frac),
            ("motion_frac", z.motion_frac),
        ] {
            if !(v > 0.0 && v < 1.0) {
                p.push(format!("zone.{name} harus antara 0 dan 1"));
            }
        }
        if !(1..=255).contains(&z.pix_thr) {
            p.push("zone.pix_thr harus 1-255".into());
        }
        if z.occupied_frac >= z.obstructed_frac {
            p.push("zone.occupied_frac harus lebih kecil dari obstructed_frac".into());
        }
        if z.stable_polls < 1 || z.empty_hits_to_close < 1 {
            p.push("zone.stable_polls dan empty_hits_to_close harus >= 1".into());
        }
        if z.poll_interval_s < 1.0 || z.idle_check_s < 1.0 {
            p.push("zone.poll_interval_s dan idle_check_s harus >= 1".into());
        }
        if z.verify_timeout_s < 2.0 * z.poll_interval_s {
            p.push("zone.verify_timeout_s terlalu pendek dibanding poll_interval_s".into());
        }

        if p.is_empty() {
            Ok(())
        } else {
            Err(ConfigError(p))
        }
    }

    pub fn purpose(&self, id: &str) -> Option<&Purpose> {
        self.purposes.iter().find(|p| p.id == id)
    }

    /// Payload konfigurasi ringkas untuk ESP32 (dikirim retained lewat MQTT).
    /// `rooms` berasal dari tabel `rooms` (satu sumber kebenaran untuk seluruh sistem).
    pub fn device_payload(&self, mode: &str, rooms: &[String]) -> serde_json::Value {
        let empty = HashMap::new();
        let texts = self.texts.get(mode).unwrap_or(&empty);
        let purposes: Vec<_> = self
            .purposes
            .iter()
            .map(|p| serde_json::json!({ "id": p.id, "label": p.label, "room": p.needs_room }))
            .collect();
        let mut payload = serde_json::json!({
            "mode": mode,
            "rooms": rooms,
            "purposes": purposes,
            "texts": texts,
            "idle_timeout_s": self.ui.idle_timeout_s,
            "done_screen_s": self.ui.done_screen_s,
        });
        // serde_json::Value memakai BTreeMap (kunci terurut) -> serialisasi deterministik.
        let ver = fnv1a_hex(serde_json::to_string(&payload).unwrap_or_default().as_bytes());
        payload["ver"] = serde_json::Value::String(ver);
        payload
    }
}

/// FNV-1a 32-bit -> 8 hex. Cukup untuk penanda versi konfigurasi (bukan keamanan).
pub fn fnv1a_hex(data: &[u8]) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in data {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{h:08x}")
}
