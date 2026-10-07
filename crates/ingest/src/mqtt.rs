use common::influx::{escape_tag, InfluxClient};
use common::models::{CamEventPayload, RoomTelemetryPayload};
use proto::telemetry::RoomTelemetry as ProtoRoomTelemetry;
use proto::alert::AlertNotification;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use sqlx::SqlitePool;
use std::time::Duration;

use crate::state::SharedState;

const TOPIC_ROOM_TELEMETRY: &str = "kost/room/+/telemetry";
const TOPIC_CAM_EVENT: &str = "kost/cam/+/event";

/// Setup koneksi MQTT ke Mosquitto dan subscribe topic telemetry kamar + event kamera.
pub fn build_client(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
) -> (AsyncClient, EventLoop) {
    let mut opts = MqttOptions::new("smartkost-ingest", host, port);
    opts.set_keep_alive(Duration::from_secs(30));
    if !username.is_empty() {
        opts.set_credentials(username, password);
    }
    AsyncClient::new(opts, 100)
}

pub async fn subscribe_all(client: &AsyncClient) -> anyhow::Result<()> {
    client.subscribe(TOPIC_ROOM_TELEMETRY, QoS::AtLeastOnce).await?;
    client.subscribe(TOPIC_CAM_EVENT, QoS::AtLeastOnce).await?;
    tracing::info!("subscribed ke {TOPIC_ROOM_TELEMETRY} dan {TOPIC_CAM_EVENT}");
    Ok(())
}

/// Loop utama: poll event dari EventLoop, dispatch tiap Publish message ke handler
/// sesuai topic-nya. Dipanggil dari main sebagai tokio task terpisah (long running).
pub async fn run_event_loop(
    mut eventloop: EventLoop,
    pool: SqlitePool,
    influx: InfluxClient,
    state: SharedState,
) {
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let topic = publish.topic.clone();
                let payload = publish.payload.to_vec();
                let pool = pool.clone();
                let influx = influx.clone();
                let state = state.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_message(&topic, &payload, &pool, &influx, &state).await {
                        tracing::warn!("gagal proses pesan topic={topic}: {e}");
                    }
                });
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!("mqtt eventloop error: {e}, retry dalam 3 detik");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

async fn handle_message(
    topic: &str,
    payload: &[u8],
    pool: &SqlitePool,
    influx: &InfluxClient,
    state: &SharedState,
) -> anyhow::Result<()> {
    let parts: Vec<&str> = topic.split('/').collect();

    match parts.as_slice() {
        ["kost", "room", room_code, "telemetry"] => {
            let data: RoomTelemetryPayload = serde_json::from_slice(payload)?;
            handle_room_telemetry(room_code, data, pool, influx, state).await
        }
        ["kost", "cam", camera_id, "event"] => {
            let data: CamEventPayload = serde_json::from_slice(payload)?;
            handle_cam_event(camera_id, data, pool, influx, state).await
        }
        _ => {
            tracing::debug!("topic tidak dikenali: {topic}");
            Ok(())
        }
    }
}

async fn handle_room_telemetry(
    room_code: &str,
    data: RoomTelemetryPayload,
    pool: &SqlitePool,
    influx: &InfluxClient,
    state: &SharedState,
) -> anyhow::Result<()> {
    // 1) Tulis ke InfluxDB (measurement: occupancy & power, digabung 1 line biar hemat request)
    let ts_nanos = data.timestamp.timestamp_nanos_opt().unwrap_or_default();
    let line_occupancy = format!(
        "occupancy,room_code={},device_id={} presence={},distance_cm={} {}",
        escape_tag(room_code),
        escape_tag(&data.device_uid),
        if data.presence { "true" } else { "false" },
        data.distance_cm.unwrap_or(-1.0),
        ts_nanos
    );
    let line_power = format!(
        "power,room_code={},device_id={} voltage={},current={},watt={},kwh_cumulative={} {}",
        escape_tag(room_code),
        escape_tag(&data.device_uid),
        data.voltage,
        data.current,
        data.watt,
        data.kwh_cumulative,
        ts_nanos
    );
    influx.write_line(&format!("{line_occupancy}\n{line_power}")).await?;

    // 2) Update status device di SQLite (last_seen_at, status=online)
    sqlx::query(
        "UPDATE devices SET status = 'online', last_seen_at = datetime('now'), updated_at = datetime('now') \
         WHERE device_uid = ?",
    )
    .bind(&data.device_uid)
    .execute(pool)
    .await?;

    // 3) Update in-memory snapshot + broadcast ke subscriber gRPC (api service)
    let proto_data = ProtoRoomTelemetry {
        room_code: room_code.to_string(),
        presence: data.presence,
        distance_cm: data.distance_cm.unwrap_or(-1.0),
        voltage: data.voltage,
        current: data.current,
        watt: data.watt,
        kwh_cumulative: data.kwh_cumulative,
        timestamp_unix: data.timestamp.timestamp(),
    };
    state.update_room(proto_data).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{camera_alert_severity, camera_alert_source};

    #[test]
    fn camera_alerts_use_camera_source_and_warning_severity() {
        assert_eq!(camera_alert_source("camera1"), "camera1");
        assert_eq!(camera_alert_source("CAMERA1"), "camera1");
        assert_eq!(camera_alert_severity("motion"), "warning");
        assert_eq!(camera_alert_severity("person"), "warning");
        assert_eq!(camera_alert_severity("unknown"), "info");
    }
}

fn camera_alert_source(camera_id: &str) -> String {
    camera_id.trim().trim_matches('/').to_ascii_lowercase()
}

fn camera_alert_severity(event_type: &str) -> &'static str {
    if matches!(event_type.trim().to_ascii_lowercase().as_str(), "person" | "vehicle" | "motion") {
        "warning"
    } else {
        "info"
    }
}

async fn handle_cam_event(
    camera_id: &str,
    data: CamEventPayload,
    pool: &SqlitePool,
    influx: &InfluxClient,
    state: &SharedState,
) -> anyhow::Result<()> {
    let normalized_camera_id = camera_alert_source(camera_id);
    let ts_nanos = data.timestamp.timestamp_nanos_opt().unwrap_or_default();
    let line = format!(
        "cctv_event,camera_id={},event_type={} confidence={} {}",
        escape_tag(&normalized_camera_id),
        escape_tag(&data.event_type),
        data.confidence,
        ts_nanos
    );
    // InfluxDB hanya untuk grafik historis. Dulu kegagalannya (Influx mati/token salah) membuat
    // fungsi berhenti di sini sehingga alert kamera TIDAK PERNAH tersimpan & tidak tampil di web.
    if let Err(e) = influx.write_line(&line).await {
        tracing::warn!("influx gagal untuk event kamera {normalized_camera_id} (alert tetap disimpan): {e}");
    }

    sqlx::query(
        "UPDATE devices SET status = 'online', last_seen_at = datetime('now'), updated_at = datetime('now') \
         WHERE device_uid = ?",
    )
    .bind(&normalized_camera_id)
    .execute(pool)
    .await?;

    // Persist to alert_events so the frontend's /api/alerts endpoint can render camera detections.
    if !data.event_type.trim().is_empty() {
        let severity = camera_alert_severity(&data.event_type);
        let alert_source = normalized_camera_id.clone();
        let alert_type = data.event_type.trim().to_string();
        let what = match alert_type.to_ascii_lowercase().as_str() {
            "person" => "Orang".to_string(),
            "vehicle" => "Kendaraan".to_string(),
            "motion" => "Gerakan".to_string(),
            other => other.to_string(),
        };
        let message = format!(
            "{what} terdeteksi di kamera {alert_source} ({:.0}%)",
            (data.confidence * 100.0).clamp(0.0, 100.0)
        );

        sqlx::query(
            "INSERT INTO alert_events (source, alert_type, severity, message) VALUES (?, ?, ?, ?)",
        )
        .bind(&alert_source)
        .bind(&alert_type)
        .bind(severity)
        .bind(&message)
        .execute(pool)
        .await?;

        state.emit_alert(AlertNotification {
            source: alert_source,
            alert_type: alert_type.clone(),
            severity: severity.to_string(),
            message,
            timestamp_unix: data.timestamp.timestamp(),
        });
    }

    Ok(())
}
