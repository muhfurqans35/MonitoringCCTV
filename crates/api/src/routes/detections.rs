//! Galeri deteksi CCTV: membaca hasil service `vision` langsung dari disk
//! (`data/vision/<kamera>/<tanggal>/<jam>_<label>.jpg` + `.json`), tanpa tabel database baru.
//!
//!   GET /api/cctv/detections?limit=48&camera=gate_right   daftar terbaru (JSON)
//!   GET /api/cctv/detections?date=2026-10-06&camera=...    semua deteksi satu hari (timeline playback)
//!   GET /api/cctv/detections/image?path=<path relatif .jpg> gambar JPEG
//!
//! Folder diatur lewat env `VISION_DIR` (default `data/vision`, relatif ke WorkingDirectory).

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use common::error::AppError;
use serde::{Deserialize, Serialize};

fn root() -> PathBuf {
    PathBuf::from(std::env::var("VISION_DIR").unwrap_or_else(|_| "data/vision".to_string()))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    camera: Option<String>,
    /// YYYY-MM-DD (tanggal lokal = nama folder vision)
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DetectionItem {
    /// Path relatif JPEG, dipakai untuk /image?path=
    pub image: String,
    pub camera_id: String,
    pub camera_label: String,
    /// "person" | "vehicle" | "motion"
    pub label: String,
    /// Confidence deteksi teratas (0 bila tidak ada objek)
    pub confidence: f64,
    /// ISO-8601 UTC
    pub timestamp: String,
    pub image_width: u64,
    pub image_height: u64,
    /// Daftar deteksi apa adanya dari vision: [{class, label, confidence, bbox:[x1,y1,x2,y2]}]
    pub detections: serde_json::Value,
}

/// GET /api/cctv/detections
pub async fn list(Query(q): Query<ListQuery>) -> Result<Json<Vec<DetectionItem>>, AppError> {
    let date = q.date.filter(|d| d.len() == 10 && d.chars().all(|c| c.is_ascii_digit() || c == '-'));
    let limit = q.limit.unwrap_or(if date.is_some() { 500 } else { 48 }).clamp(1, 1000);
    let camera = q.camera.filter(|c| !c.is_empty());
    let items = tokio::task::spawn_blocking(move || scan(&root(), limit, camera.as_deref(), date.as_deref()))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("scan deteksi gagal: {e}")))?;
    Ok(Json(items))
}

/// Kumpulkan JSON terbaru: per kamera, telusuri folder tanggal dari yang terbaru sampai cukup.
fn scan(root: &Path, limit: usize, camera: Option<&str>, date: Option<&str>) -> Vec<DetectionItem> {
    let mut files: Vec<PathBuf> = Vec::new();
    let Ok(cams) = std::fs::read_dir(root) else { return Vec::new() };
    for cam in cams.flatten() {
        let name = cam.file_name().to_string_lossy().to_string();
        if camera.map(|c| c != name).unwrap_or(false) || !cam.path().is_dir() {
            continue;
        }
        let mut days: Vec<PathBuf> = std::fs::read_dir(cam.path())
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
            .unwrap_or_default();
        if let Some(date) = date {
            days.retain(|d| d.file_name().map(|n| n == date).unwrap_or(false));
        }
        days.sort();
        let mut taken = 0;
        for day in days.iter().rev() {
            let mut jsons: Vec<PathBuf> = std::fs::read_dir(day)
                .map(|d| {
                    d.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                        .collect()
                })
                .unwrap_or_default();
            jsons.sort();
            for j in jsons.into_iter().rev() {
                files.push(j);
                taken += 1;
                if taken >= limit {
                    break;
                }
            }
            if taken >= limit {
                break;
            }
        }
    }

    let mut items: Vec<DetectionItem> = files.iter().filter_map(|p| read_item(root, p)).collect();
    items.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    items.truncate(limit);
    items
}

fn read_item(root: &Path, json_path: &Path) -> Option<DetectionItem> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(json_path).ok()?).ok()?;
    let jpg = json_path.with_extension("jpg");
    if !jpg.exists() {
        return None;
    }
    let rel = jpg.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let detections = v.get("detections").cloned().unwrap_or(serde_json::Value::Array(vec![]));
    let confidence = detections
        .as_array()
        .map(|a| a.iter().filter_map(|d| d.get("confidence").and_then(|c| c.as_f64())).fold(0.0, f64::max))
        .unwrap_or(0.0);
    Some(DetectionItem {
        image: rel,
        camera_id: s("camera_id"),
        camera_label: s("camera_label"),
        label: s("label"),
        confidence,
        timestamp: s("timestamp"),
        image_width: n("image_width"),
        image_height: n("image_height"),
        detections,
    })
}

#[derive(Debug, Deserialize)]
pub struct ImageQuery {
    path: String,
}

/// Path relatif aman: tanpa "..", tanpa akar/drive, harus .jpg.
fn safe_relative(p: &str) -> Option<PathBuf> {
    let path = Path::new(p);
    if !p.ends_with(".jpg") || path.is_absolute() {
        return None;
    }
    if path.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(path.to_path_buf())
}

/// GET /api/cctv/detections/image?path=gate_right/2026-10-06/132701_412_person.jpg
pub async fn image(Query(q): Query<ImageQuery>) -> Result<Response, AppError> {
    let rel = safe_relative(&q.path).ok_or_else(|| AppError::BadRequest("path gambar tidak valid".into()))?;
    let full = root().join(rel);
    let bytes = tokio::fs::read(&full)
        .await
        .map_err(|_| AppError::NotFound("gambar deteksi tidak ditemukan (mungkin sudah dihapus retensi)".into()))?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/jpeg".to_string()),
            (header::CACHE_CONTROL, "private, max-age=86400".to_string()),
        ],
        Body::from(bytes),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        assert!(safe_relative("gate_right/2026-10-06/1_person.jpg").is_some());
        assert!(safe_relative("../etc/passwd.jpg").is_none());
        assert!(safe_relative("gate_right/../../x.jpg").is_none());
        assert!(safe_relative("/etc/x.jpg").is_none());
        assert!(safe_relative("gate_right/x.json").is_none());
    }

    #[test]
    fn scans_newest_first_across_cameras() {
        let root = std::env::temp_dir().join(format!("det-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (cam, day, stem, ts) in [
            ("gate_right", "2026-10-05", "100000_000_person", "2026-10-05T02:00:00Z"),
            ("gate_right", "2026-10-06", "090000_000_vehicle", "2026-10-06T01:00:00Z"),
            ("kost", "2026-10-06", "120000_000_person", "2026-10-06T04:00:00Z"),
        ] {
            let d = root.join(cam).join(day);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(format!("{stem}.jpg")), [0xFF, 0xD8]).unwrap();
            let j = serde_json::json!({"camera_id": cam, "camera_label": cam, "label": "person", "timestamp": ts,
                "detections": [{"class": "person", "confidence": 0.8, "bbox": [1, 2, 3, 4]}]});
            std::fs::write(d.join(format!("{stem}.json")), j.to_string()).unwrap();
        }
        let items = scan(&root, 10, None, None);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].camera_id, "kost");
        assert_eq!(items[2].timestamp, "2026-10-05T02:00:00Z");
        assert!((items[0].confidence - 0.8).abs() < 1e-9);
        assert_eq!(scan(&root, 10, Some("gate_right"), None).len(), 2);
        assert_eq!(scan(&root, 10, None, Some("2026-10-06")).len(), 2);
        assert_eq!(scan(&root, 1, None, None).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
