use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Room {
    pub id: i64,
    pub code: String, // contoh: "K01"
    pub floor: i64,
    pub tariff_per_kwh: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Tenant {
    pub id: i64,
    pub room_id: i64,
    pub name: String,
    pub phone: String,
    pub telegram_chat_id: String,
    pub check_in_date: DateTime<Utc>,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "text")]
pub enum DeviceType {
    #[sqlx(rename = "room_node")]
    RoomNode,
    #[sqlx(rename = "corridor_node")]
    CorridorNode,
    #[sqlx(rename = "gate_cam")]
    GateCam,
    /// Node bel pintu interaktif (layar sentuh + kamera zona paket), modul doorbell.
    #[sqlx(rename = "door_node")]
    DoorNode,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "text")]
pub enum DeviceStatus {
    #[sqlx(rename = "online")]
    Online,
    #[sqlx(rename = "offline")]
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Device {
    pub id: i64,
    pub device_uid: String, // chip-id ESP32 / camera id
    pub room_id: Option<i64>,
    pub device_type: DeviceType,
    pub location: String, // "Koridor Lt.2", "Gerbang Utama"
    pub status: DeviceStatus,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "text")]
pub enum BillingStatus {
    #[sqlx(rename = "draft")]
    Draft,
    #[sqlx(rename = "issued")]
    Issued,
    #[sqlx(rename = "paid")]
    Paid,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct BillingRecord {
    pub id: i64,
    pub room_id: i64,
    pub period: String, // "2026-07"
    pub total_kwh: f64,
    pub total_amount: f64,
    pub status: BillingStatus,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "text")]
pub enum AlertType {
    #[sqlx(rename = "power_anomaly")]
    PowerAnomaly, // daya tinggi tapi kamar kosong
    #[sqlx(rename = "night_motion")]
    NightMotion, // gerakan di jam rawan
    #[sqlx(rename = "loitering")]
    Loitering, // orang berlama-lama di pagar/gerbang
    #[sqlx(rename = "device_offline")]
    DeviceOffline,
    /// Paket tercatat di layar tapi kamera tidak melihat perubahan di zona paket.
    #[sqlx(rename = "package_unverified")]
    PackageUnverified,
    /// Penghuni menyatakan paket bukan miliknya (kemungkinan salah kamar).
    #[sqlx(rename = "package_disputed")]
    PackageDisputed,
}

impl AlertType {
    pub fn as_str(&self) -> &'static str {
        match self {
            AlertType::PowerAnomaly => "power_anomaly",
            AlertType::NightMotion => "night_motion",
            AlertType::Loitering => "loitering",
            AlertType::DeviceOffline => "device_offline",
            AlertType::PackageUnverified => "package_unverified",
            AlertType::PackageDisputed => "package_disputed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "text")]
pub enum AlertSeverity {
    #[sqlx(rename = "info")]
    Info,
    #[sqlx(rename = "warning")]
    Warning,
    #[sqlx(rename = "critical")]
    Critical,
}

impl AlertSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            AlertSeverity::Info => "info",
            AlertSeverity::Warning => "warning",
            AlertSeverity::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AlertEvent {
    pub id: i64,
    pub source: String, // "room_K01", "gate_cam_1", "corridor_2"
    pub alert_type: AlertType,
    pub severity: AlertSeverity,
    pub message: String,
    pub resolved: bool,
    pub created_at: DateTime<Utc>,
}

/// Payload MQTT dari ESP32 kamar, topic: kost/room/{code}/telemetry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomTelemetryPayload {
    pub device_uid: String,
    pub presence: bool,      // hasil mmWave (LD2410)
    pub distance_cm: Option<f32>,
    pub voltage: f32,
    pub current: f32,
    pub watt: f32,
    pub kwh_cumulative: f32,
    pub timestamp: DateTime<Utc>,
}

/// Payload event dari node koridor/kamera, topic: kost/cam/{camera_id}/event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CamEventPayload {
    pub camera_id: String,
    pub event_type: String, // "motion", "loitering", "line_cross"
    pub confidence: f32,
    pub snapshot_path: Option<String>,
    pub timestamp: DateTime<Utc>,
}
