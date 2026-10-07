use common::influx::InfluxClient;
use common::models::{AlertSeverity, AlertType};
use sqlx::SqlitePool;

use crate::notifier::send_telegram_message;

/// Dipanggil tiap N menit (lihat main.rs). Mengecek pola:
/// kamar dalam keadaan kosong (presence=false) tapi daya masih tinggi selama
/// beberapa menit terakhir -> kemungkinan lupa cabut alat elektronik.
pub async fn check_power_anomaly(
    pool: &SqlitePool,
    influx: &InfluxClient,
    telegram_token: &str,
    owner_chat_id: &str,
) -> anyhow::Result<()> {
    let rooms: Vec<(String,)> = sqlx::query_as("SELECT code FROM rooms")
        .fetch_all(pool)
        .await?;

    for (room_code,) in rooms {
        // Ambil rata-rata presence & watt 10 menit terakhir.
        let flux = format!(
            r#"presence = from(bucket: "telemetry")
  |> range(start: -10m)
  |> filter(fn: (r) => r._measurement == "occupancy" and r.room_code == "{code}" and r._field == "presence")
  |> mean()
watt = from(bucket: "telemetry")
  |> range(start: -10m)
  |> filter(fn: (r) => r._measurement == "power" and r.room_code == "{code}" and r._field == "watt")
  |> mean()
union(tables: [presence, watt])"#,
            code = room_code
        );

        let csv = match influx.query_flux(&flux).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("gagal query anomaly check untuk {room_code}: {e}");
                continue;
            }
        };

        // Parsing sederhana: cari baris dengan _field=presence dan _field=watt,
        // ambil kolom _value. Threshold: presence rata-rata < 0.1 (dianggap kosong)
        // dan watt rata-rata > 50 (ambang bisa disesuaikan per kost).
        let (avg_presence, avg_watt) = parse_presence_and_watt(&csv);

        if let (Some(p), Some(w)) = (avg_presence, avg_watt) {
            if p < 0.1 && w > 50.0 {
                let message = format!(
                    "⚠️ Kamar *{room_code}* terdeteksi kosong tapi daya masih {w:.0} W. Cek kemungkinan alat elektronik lupa dimatikan."
                );

                insert_alert(pool, &format!("room_{room_code}"), AlertType::PowerAnomaly, AlertSeverity::Warning, &message).await?;
                let _ = send_telegram_message(telegram_token, owner_chat_id, &message).await;
            }
        }
    }

    Ok(())
}

fn parse_presence_and_watt(csv: &str) -> (Option<f64>, Option<f64>) {
    let mut presence = None;
    let mut watt = None;

    let lines = csv.lines();
    let header = match lines.clone().find(|l| l.starts_with(",result,")) {
        Some(h) => h,
        None => return (None, None),
    };
    let headers: Vec<&str> = header.split(',').collect();
    let field_idx = headers.iter().position(|h| *h == "_field");
    let value_idx = headers.iter().position(|h| *h == "_value");

    let (Some(fi), Some(vi)) = (field_idx, value_idx) else { return (None, None) };

    for line in lines {
        if line.trim().is_empty() || line.starts_with(",result,") { continue; }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() <= fi.max(vi) { continue; }

        match cols[fi] {
            "presence" => presence = cols[vi].parse::<f64>().ok(),
            "watt" => watt = cols[vi].parse::<f64>().ok(),
            _ => {}
        }
    }

    (presence, watt)
}

pub async fn insert_alert(
    pool: &SqlitePool,
    source: &str,
    alert_type: AlertType,
    severity: AlertSeverity,
    message: &str,
) -> anyhow::Result<()> {
    let type_str = alert_type.as_str();
    let severity_str = severity.as_str();

    sqlx::query(
        "INSERT INTO alert_events (source, alert_type, severity, message) VALUES (?, ?, ?, ?)",
    )
    .bind(source)
    .bind(type_str)
    .bind(severity_str)
    .bind(message)
    .execute(pool)
    .await?;

    Ok(())
}
