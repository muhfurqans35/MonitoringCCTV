use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use proto::telemetry::RoomTelemetry as ProtoRoomTelemetry;
use proto::alert::AlertNotification;

/// State yang dishare antara task MQTT subscriber dan gRPC server:
/// - `snapshot`: data live per kamar (buat GetLiveSnapshot)
/// - `telemetry_tx`: broadcast channel buat StreamRoomTelemetry
/// - `alert_tx`: broadcast channel buat StreamAlerts
#[derive(Clone)]
pub struct SharedState {
    pub snapshot: Arc<RwLock<HashMap<String, ProtoRoomTelemetry>>>,
    pub telemetry_tx: broadcast::Sender<ProtoRoomTelemetry>,
    pub alert_tx: broadcast::Sender<AlertNotification>,
}

impl SharedState {
    pub fn new() -> Self {
        let (telemetry_tx, _) = broadcast::channel(256);
        let (alert_tx, _) = broadcast::channel(256);
        Self {
            snapshot: Arc::new(RwLock::new(HashMap::new())),
            telemetry_tx,
            alert_tx,
        }
    }

    pub async fn update_room(&self, data: ProtoRoomTelemetry) {
        self.snapshot.write().await.insert(data.room_code.clone(), data.clone());
        // Abaikan error kalau tidak ada subscriber aktif saat ini.
        let _ = self.telemetry_tx.send(data);
    }

    pub fn emit_alert(&self, alert: AlertNotification) {
        let _ = self.alert_tx.send(alert);
    }
}
