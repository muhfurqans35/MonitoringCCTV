//! Pengaturan service vision. Satu sumber kebenaran dengan live view: daftar kamera diambil dari
//! `[[channels]]` di `config/nvr.toml` (dipakai juga oleh service api + go2rtc), dan pengaturan
//! deteksi dari bagian `[vision]` di file yang sama. Semua field `[vision]` punya nilai default,
//! jadi bagian itu boleh tidak ada sama sekali. `config/cameras.toml` TIDAK dipakai lagi.

use std::collections::HashMap;
use std::path::Path;

use common::nvr_config::{NvrChannel, NvrConfig};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VisionSettings {
    /// Alamat API go2rtc. Frame diambil dari `{go2rtc_url}/api/frame.jpeg?src={channel id}`;
    /// nama stream di go2rtc.yaml harus sama dengan `id` channel di nvr.toml.
    pub go2rtc_url: String,
    /// Akhiran nama stream go2rtc untuk analisis. "" = stream yang sama dengan live view.
    /// Contoh "_sub": vision memakai stream `gate_left_sub` (substream NVR, resolusi kecil ->
    /// decode & inference jauh lebih ringan); tambahkan stream itu di go2rtc.yaml.
    pub stream_suffix: String,
    /// Jeda antar-pengambilan frame per kamera (ms). go2rtc menjalankan ffmpeg per frame, jadi
    /// 2000 ms x 4 kamera masih ringan untuk Raspberry Pi 4.
    pub interval_ms: u64,
    /// Channel yang dipantau (id). Kosong = semua channel di nvr.toml.
    pub channels: Vec<String>,
    /// Jeda minimum antar-event per kamera (detik): satu gerakan panjang = satu event.
    pub cooldown_seconds: u64,
    /// Folder simpan snapshot + JSON hasil deteksi (harus di bawah data/, lihat systemd unit).
    pub snapshot_dir: String,
    /// Snapshot lebih tua dari ini dihapus otomatis (0 = simpan selamanya).
    pub keep_days: u64,
    /// true (default) = HANYA gerakan yang berisi objek dari `[vision.model].classes`
    /// (orang/kendaraan) yang disimpan & dikirim ke dashboard. Butuh file model; tanpa model
    /// tidak ada event sama sekali (log memberi peringatan). false = mode lama, semua gerakan.
    pub only_objects: bool,
    /// Simpan juga frame gerakan yang TIDAK berisi objek (data negatif dataset). Hanya berlaku
    /// bila only_objects = false.
    pub save_empty_motion: bool,
    /// Kirim event "motion" ke dashboard walau model tidak menemukan objek. Default: hanya bila
    /// model tidak tersedia (tanpa model, semua gerakan dilaporkan sebagai "motion").
    pub publish_motion_without_detection: bool,
    /// Dengarkan alarm motion langsung dari NVR (DVRIP port 34567). Saat NVR melapor gerakan,
    /// kamera itu langsung diambil frame-nya & dianalisis model (tanpa menunggu interval +
    /// konfirmasi deteksi gerak sendiri) -> wajah/orang tertangkap lebih awal.
    pub nvr_alarm: bool,
    /// Jeda ambil frame selama `alarm_boost_seconds` setelah alarm NVR (lebih rapat dari interval_ms).
    pub alarm_interval_ms: u64,
    pub alarm_boost_seconds: u64,
    pub motion: MotionSettings,
    pub model: ModelSettings,
}

impl Default for VisionSettings {
    fn default() -> Self {
        Self {
            go2rtc_url: "http://127.0.0.1:1984".into(),
            stream_suffix: String::new(),
            interval_ms: 2000,
            channels: Vec::new(),
            cooldown_seconds: 30,
            snapshot_dir: "data/vision".into(),
            keep_days: 14,
            only_objects: true,
            save_empty_motion: false,
            publish_motion_without_detection: false,
            nvr_alarm: true,
            alarm_interval_ms: 700,
            alarm_boost_seconds: 15,
            motion: MotionSettings::default(),
            model: ModelSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MotionSettings {
    /// Lebar gambar kerja (px) untuk deteksi gerak; kecil = cepat & tahan noise.
    pub width: u32,
    /// Selisih abu-abu per piksel (0-255) agar dianggap berubah.
    pub pixel_threshold: u8,
    /// Porsi piksel berubah (0-1) di dalam ROI agar dianggap ada gerakan.
    pub min_fraction: f32,
    /// Di atas porsi ini dianggap perubahan cahaya (lampu/IR malam), bukan gerakan.
    pub max_fraction: f32,
    /// Berapa frame berturut-turut harus bergerak sebelum memicu (redam noise/hujan).
    pub consecutive: u32,
    /// Area deteksi per kamera: id -> [x, y, lebar, tinggi] pecahan 0-1. Tidak ada = seluruh frame.
    pub roi: HashMap<String, [f32; 4]>,
}

impl Default for MotionSettings {
    fn default() -> Self {
        Self {
            width: 160,
            pixel_threshold: 25,
            min_fraction: 0.015,
            max_fraction: 0.6,
            consecutive: 2,
            roi: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ModelSettings {
    /// Model ONNX format YOLOv8/YOLO11 (output 1 x (4+kelas) x N). Tidak ada file = tanpa deteksi objek.
    pub path: String,
    /// Ukuran input model. 320 ~4x lebih cepat dari 640 (cukup untuk orang/kendaraan di CCTV).
    /// HARUS sama dengan ukuran saat model diekspor (yolo export imgsz=320).
    pub input_size: u32,
    pub confidence: f32,
    /// Kelas COCO yang dilaporkan; sisanya diabaikan.
    pub classes: Vec<String>,
    /// Inference bersamaan maksimum (CPU Pi terbatas).
    pub max_concurrent: usize,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            path: "models/yolov8n.onnx".into(),
            input_size: 320,
            confidence: 0.45,
            classes: ["person", "bicycle", "car", "motorcycle", "bus", "truck"].iter().map(|s| s.to_string()).collect(),
            max_concurrent: 1,
        }
    }
}

#[derive(Deserialize, Default)]
struct NvrFileVisionPart {
    #[serde(default)]
    vision: VisionSettings,
}

/// Kamera yang dipantau: channel nvr.toml yang lolos filter `[vision].channels`.
pub struct Loaded {
    pub settings: VisionSettings,
    pub channels: Vec<NvrChannel>,
    /// Akses DVRIP ke NVR untuk alarm (ip, port, user, password).
    pub nvr: (String, u16, String, String),
}

pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Loaded> {
    let path = path.as_ref();
    let nvr = NvrConfig::load(path)?;
    let text = std::fs::read_to_string(path)?;
    let part: NvrFileVisionPart =
        toml::from_str(&text).map_err(|e| anyhow::anyhow!("bagian [vision] di {path:?} tidak valid: {e}"))?;
    let settings = part.vision;
    let access = (nvr.ip.clone(), nvr.dvrip_port, nvr.username.clone(), nvr.password.clone());
    let channels: Vec<NvrChannel> = nvr
        .channels
        .into_iter()
        .filter(|c| settings.channels.is_empty() || settings.channels.iter().any(|s| s == &c.id))
        .collect();
    if channels.is_empty() {
        anyhow::bail!("tidak ada channel yang dipantau (cek [[channels]] dan [vision].channels di {path:?})");
    }
    Ok(Loaded { settings, channels, nvr: access })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NVR: &str = r#"
ip = "192.168.1.200"
username = "admin"
password = ""
rtsp_port = 554
live_url_pattern = "rtsp://{ip}"

[[channels]]
id = "gate_left"
label = "Kiri"
channel_number = 4

[[channels]]
id = "gate_right"
label = "Kanan"
channel_number = 5
"#;

    fn write(extra: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("nvr-test-{}-{}.toml", std::process::id(), extra.len()));
        std::fs::write(&p, format!("{NVR}{extra}")).unwrap();
        p
    }

    #[test]
    fn without_vision_section_uses_defaults_and_all_channels() {
        let l = load(write("")).unwrap();
        assert_eq!(l.channels.len(), 2);
        assert_eq!(l.settings.model.input_size, 320);
        assert_eq!(l.settings.go2rtc_url, "http://127.0.0.1:1984");
        assert!(l.settings.nvr_alarm);
        assert_eq!(l.nvr.1, 34567);
    }

    #[test]
    fn vision_section_filters_channels_and_sets_roi() {
        let l = load(write(
            "\n[vision]\nchannels = [\"gate_right\"]\ninterval_ms = 1500\n\n[vision.motion.roi]\ngate_right = [0.0, 0.3, 1.0, 0.7]\n",
        ))
        .unwrap();
        assert_eq!(l.channels.len(), 1);
        assert_eq!(l.channels[0].id, "gate_right");
        assert_eq!(l.settings.interval_ms, 1500);
        assert_eq!(l.settings.motion.roi["gate_right"], [0.0, 0.3, 1.0, 0.7]);
    }
}
