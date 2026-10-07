//! Simpan snapshot event + hasil deteksi ke disk, untuk dianalisis ulang dengan model lain
//! atau dikumpulkan menjadi dataset:
//!
//!   data/vision/<camera_id>/<YYYY-MM-DD>/<HHMMSS_mmm>_<label>.jpg   gambar penuh dari NVR
//!   data/vision/<camera_id>/<YYYY-MM-DD>/<HHMMSS_mmm>_<label>.json  metadata + deteksi (bbox)

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::Serialize;

use crate::inference::Detection;

#[derive(Debug, Serialize)]
pub struct EventRecord<'a> {
    pub camera_id: &'a str,
    pub camera_label: &'a str,
    pub nvr_channel: u32,
    pub timestamp: DateTime<Utc>,
    /// "person" | "vehicle" | "motion" | ...
    pub label: &'a str,
    pub motion_score: f32,
    pub detections: &'a [Detection],
    /// Model yang dipakai (None = tanpa model).
    pub model: Option<&'a str>,
    pub model_input_size: Option<u32>,
    pub image_width: u32,
    pub image_height: u32,
    pub published: bool,
}

/// Simpan JPEG + JSON. Mengembalikan path JPEG relatif terhadap `root` (untuk payload MQTT).
pub fn save(root: &Path, rec: &EventRecord, jpeg: &[u8]) -> anyhow::Result<String> {
    let local: DateTime<Local> = rec.timestamp.with_timezone(&Local);
    let dir = root.join(sanitize(rec.camera_id)).join(local.format("%Y-%m-%d").to_string());
    std::fs::create_dir_all(&dir)?;
    let stem = format!("{}_{}", local.format("%H%M%S_%3f"), sanitize(rec.label));
    let jpg = dir.join(format!("{stem}.jpg"));
    std::fs::write(&jpg, jpeg)?;
    std::fs::write(dir.join(format!("{stem}.json")), serde_json::to_vec_pretty(rec)?)?;
    Ok(jpg.strip_prefix(root).unwrap_or(&jpg).to_string_lossy().replace('\\', "/"))
}

/// Hapus folder tanggal yang lebih tua dari `keep_days` (0 = tidak pernah). Mengembalikan jumlah folder dihapus.
pub fn purge(root: &Path, keep_days: u64) -> usize {
    if keep_days == 0 {
        return 0;
    }
    let cutoff = Local::now().date_naive() - chrono::Duration::days(keep_days as i64);
    let mut removed = 0;
    let Ok(cams) = std::fs::read_dir(root) else { return 0 };
    for cam in cams.flatten() {
        let Ok(days) = std::fs::read_dir(cam.path()) else { continue };
        for day in days.flatten() {
            let name = day.file_name().to_string_lossy().to_string();
            if let Ok(d) = NaiveDate::parse_from_str(&name, "%Y-%m-%d") {
                if d < cutoff && std::fs::remove_dir_all(day.path()).is_ok() {
                    removed += 1;
                }
            }
        }
    }
    removed
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

pub fn root(dir: &str) -> PathBuf {
    PathBuf::from(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_jpeg_and_json_and_purges_old_days() {
        let root = std::env::temp_dir().join(format!("vision-sink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dets = vec![Detection { class: "person".into(), label: "person".into(), confidence: 0.9, bbox: [1.0, 2.0, 3.0, 4.0] }];
        let rec = EventRecord {
            camera_id: "gate/right",
            camera_label: "Kanan",
            nvr_channel: 5,
            timestamp: Utc::now(),
            label: "person",
            motion_score: 0.05,
            detections: &dets,
            model: Some("yolov8n.onnx"),
            model_input_size: Some(320),
            image_width: 704,
            image_height: 576,
            published: true,
        };
        let rel = save(&root, &rec, &[0xFF, 0xD8, 0xFF]).unwrap();
        assert!(rel.starts_with("gate_right/") && rel.ends_with("_person.jpg"), "{rel}");
        let json = std::fs::read_to_string(root.join(rel.replace(".jpg", ".json"))).unwrap();
        assert!(json.contains("\"bbox\"") && json.contains("\"nvr_channel\": 5"));

        let old = root.join("gate_right").join("2000-01-01");
        std::fs::create_dir_all(&old).unwrap();
        assert_eq!(purge(&root, 14), 1);
        assert!(!old.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
