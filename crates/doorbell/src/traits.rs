//! Antarmuka ke dunia luar (kamera, notifikasi, MQTT, jam). Diganti dengan tiruan saat uji.

use tonic::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub text: String,
    pub data: String,
}

impl Button {
    pub fn new(text: impl Into<String>, data: impl Into<String>) -> Self {
        Self { text: text.into(), data: data.into() }
    }
}

pub type Buttons = Vec<Vec<Button>>;

#[async_trait]
pub trait Camera: Send + Sync {
    /// JPEG terbaru dari node, atau None bila node/kamera tidak merespons.
    async fn capture(&self) -> Option<Vec<u8>>;

    /// Sama seperti `capture`, tapi dengan alasan kegagalan yang bisa dibaca manusia
    /// (dipakai live view dashboard). Implementasi bawaan cukup untuk kamera tiruan.
    async fn capture_detailed(&self) -> Result<Vec<u8>, String> {
        self.capture().await.ok_or_else(|| "kamera tidak merespons".to_string())
    }
}

#[async_trait]
pub trait Notifier: Send + Sync {
    async fn send(&self, chat_id: i64, text: &str, photo: Option<&[u8]>, buttons: Option<&Buttons>);
}

#[async_trait]
pub trait Publisher: Send + Sync {
    async fn publish(&self, topic: &str, payload: &serde_json::Value, retain: bool);
}

pub trait Clock: Send + Sync {
    /// Epoch detik.
    fn now(&self) -> f64;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }
}
