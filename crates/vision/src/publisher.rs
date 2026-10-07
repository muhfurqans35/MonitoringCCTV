use rumqttc::{AsyncClient, QoS};

use crate::inference::Detection;

/// Publish event ke `kost/cam/{camera_id}/event` — format CamEventPayload yang sudah diproses
/// ingest (InfluxDB + alert_events + notifikasi WebSocket dashboard) dan alertengine.
pub async fn publish_event(
    client: &AsyncClient,
    camera_id: &str,
    event_type: &str,
    confidence: f32,
    snapshot_path: Option<&str>,
    detections: &[Detection],
    motion_score: f32,
) -> anyhow::Result<()> {
    let payload = serde_json::json!({
        "camera_id": camera_id,
        "event_type": event_type,
        "confidence": confidence,
        "snapshot_path": snapshot_path,
        "timestamp": chrono::Utc::now(),
        "detection_count": detections.len(),
        "detections": detections,
        "motion_score": motion_score,
        "source": "vision",
    });
    client
        .publish(format!("kost/cam/{camera_id}/event"), QoS::AtLeastOnce, false, serde_json::to_vec(&payload)?)
        .await?;
    Ok(())
}

/// Status pemantauan per kamera (retained) -> terlihat di MQTT Explorer: `kost/cam/{id}/vision`.
pub async fn publish_status(client: &AsyncClient, camera_id: &str, status: &serde_json::Value) -> anyhow::Result<()> {
    client
        .publish(format!("kost/cam/{camera_id}/vision"), QoS::AtLeastOnce, true, serde_json::to_vec(status)?)
        .await?;
    Ok(())
}
