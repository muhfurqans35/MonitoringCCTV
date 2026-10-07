mod notifier;
mod rules;

use common::config::Config;
use common::db::init_pool;
use common::influx::InfluxClient;
use common::models::{AlertSeverity, AlertType};
use proto::alert::alert_service_client::AlertServiceClient;
use proto::alert::StreamAlertsRequest;
use tonic::transport::Endpoint;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cfg = Config::load()?;

    tracing::info!("alert-engine starting...");

    let pool = init_pool(&cfg.sqlite_path).await?;
    let influx = InfluxClient::new(&cfg.influx_url, &cfg.influx_org, &cfg.influx_bucket, &cfg.influx_token);

    // --- Task 1: konsumsi stream alert mentah dari ingest (loitering/line_cross CCTV) ---
    let stream_pool = pool.clone();
    let telegram_token = cfg.telegram_bot_token.clone();
    let owner_chat_id = cfg.telegram_owner_chat_id.clone();
    let ingest_addr = cfg.ingest_grpc_addr.clone();

    tokio::spawn(async move {
        loop {
            if let Err(e) = consume_alert_stream(&ingest_addr, &stream_pool, &telegram_token, &owner_chat_id).await {
                tracing::warn!("alert stream terputus: {e}, reconnect dalam 5 detik");
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });

    // --- Task 2: periodic rule check (power anomaly, dsb) tiap 5 menit ---
    let period_pool = pool.clone();
    let period_influx = influx.clone();
    let period_token = cfg.telegram_bot_token.clone();
    let period_chat = cfg.telegram_owner_chat_id.clone();

    loop {
        if let Err(e) = rules::check_power_anomaly(&period_pool, &period_influx, &period_token, &period_chat).await {
            tracing::warn!("power anomaly check gagal: {e}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
    }
}

async fn consume_alert_stream(
    ingest_addr: &str,
    pool: &sqlx::SqlitePool,
    telegram_token: &str,
    owner_chat_id: &str,
) -> anyhow::Result<()> {
    let endpoint = Endpoint::from_shared(ingest_addr.to_string())?;
    let channel = endpoint.connect().await?;
    let mut client = AlertServiceClient::new(channel);

    let mut stream = client.stream_alerts(StreamAlertsRequest {}).await?.into_inner();

    while let Some(alert) = stream.message().await? {
        tracing::info!("alert diterima: {} - {}", alert.source, alert.message);

        let alert_type = match alert.alert_type.as_str() {
            "loitering" | "line_cross" | "person" | "vehicle" | "motion" => AlertType::Loitering,
            "night_motion" => AlertType::NightMotion,
            _ => AlertType::Loitering,
        };
        let severity = match alert.severity.as_str() {
            "critical" => AlertSeverity::Critical,
            "warning" => AlertSeverity::Warning,
            _ => AlertSeverity::Info,
        };

        rules::insert_alert(pool, &alert.source, alert_type, severity, &alert.message).await?;
        let _ = notifier::send_telegram_message(telegram_token, owner_chat_id, &alert.message).await;
    }

    Ok(())
}
