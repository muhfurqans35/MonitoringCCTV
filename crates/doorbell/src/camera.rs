//! Ambil foto dari node ESP32 lewat HTTP (hub menarik hanya saat dibutuhkan; tanpa streaming).

use std::sync::{Arc, RwLock};

use tonic::async_trait;

use crate::config::CameraCfg;
use crate::traits::Camera;

pub struct HttpCamera {
    http: reqwest::Client,
    cfg: CameraCfg,
    device_ip: Arc<RwLock<Option<String>>>,
}

impl HttpCamera {
    pub fn new(cfg: CameraCfg, device_ip: Arc<RwLock<Option<String>>>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(cfg.timeout_s.max(1)))
            .build()
            .unwrap_or_default();
        if cfg.token.is_empty() {
            tracing::warn!("token kamera kosong (DOORBELL_CAMERA_TOKEN / [camera].token): node akan menolak /capture dengan 401");
        }
        Self { http, cfg, device_ip }
    }
}

#[async_trait]
impl Camera for HttpCamera {
    async fn capture(&self) -> Option<Vec<u8>> {
        match self.capture_detailed().await {
            Ok(b) => Some(b),
            Err(e) => {
                tracing::debug!("capture kamera pintu gagal: {e}");
                None
            }
        }
    }

    async fn capture_detailed(&self) -> Result<Vec<u8>, String> {
        let host = if !self.cfg.host.is_empty() {
            self.cfg.host.clone()
        } else {
            self.device_ip
                .read()
                .ok()
                .and_then(|g| g.clone())
                .ok_or_else(|| "IP node belum diketahui (belum ada status online lewat MQTT; atau isi [camera].host)".to_string())?
        };
        let url = format!("http://{host}:{}/capture", self.cfg.port);
        let resp = self.http.get(&url).header("X-Token", &self.cfg.token).send().await.map_err(|e| {
            if e.is_timeout() {
                format!("timeout {} dtk menghubungi {url}", self.cfg.timeout_s)
            } else if e.is_connect() {
                format!("tidak bisa terhubung ke {url} (node mati / beda jaringan / IP berubah)")
            } else {
                format!("gagal request {url}: {e}")
            }
        })?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err("node menolak token (401): samakan DOORBELL_CAMERA_TOKEN dengan DEVICE_TOKEN di secrets.h".into());
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("node membalas HTTP {status}: {}", body.chars().take(80).collect::<String>()));
        }
        let is_jpeg = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("image/jpeg"))
            .unwrap_or(false);
        if !is_jpeg {
            return Err("balasan node bukan image/jpeg".into());
        }
        let bytes = resp.bytes().await.map_err(|e| format!("gagal membaca body JPEG: {e}"))?;
        if bytes.len() < 1024 {
            return Err(format!("frame terlalu kecil ({} B) — kemungkinan gambar kosong OV5640, turunkan XCLK", bytes.len()));
        }
        Ok(bytes.to_vec())
    }
}
