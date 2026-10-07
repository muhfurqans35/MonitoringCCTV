mod bot;
mod camera;
mod config;
mod detector;
mod grpc;
mod hub;
mod mqtt;
mod store;
mod telegram;
mod traits;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use common::config::Config;
use common::db::init_pool;
use proto::doorbell::doorbell_service_server::DoorbellServiceServer;
use tonic::transport::Server;

use crate::bot::BotRunner;
use crate::camera::HttpCamera;
use crate::config::DoorbellConfig;
use crate::detector::Detector;
use crate::hub::{Hub, HubDeps};
use crate::mqtt::MqttPublisher;
use crate::store::Store;
use crate::telegram::{ConsoleNotifier, TelegramApi, TelegramNotifier};
use crate::traits::{Notifier, SystemClock};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cfg = Config::load()?;

    // `doorbell --check` memvalidasi konfigurasi lalu keluar (untuk CI / sebelum deploy).
    let check_only = std::env::args().any(|a| a == "--check");
    let dcfg = Arc::new(DoorbellConfig::load(&cfg.doorbell_config)?);
    if check_only {
        println!("Konfigurasi doorbell valid ({}).", cfg.doorbell_config);
        return Ok(());
    }
    tracing::info!("doorbell service starting (pintu={}, mode default={})", dcfg.door_id, dcfg.default_mode);

    let pool = init_pool(&cfg.sqlite_path).await?;
    let store = Store::new(pool);

    // Pengelola = pemilik (TELEGRAM_OWNER_CHAT_ID, sama dengan alertengine) + daftar tambahan di doorbell.toml.
    let mut managers = dcfg.managers.clone();
    if let Ok(owner) = cfg.telegram_owner_chat_id.trim().parse::<i64>() {
        if !managers.contains(&owner) {
            managers.push(owner);
        }
    }
    if managers.is_empty() {
        tracing::warn!("tidak ada pengelola (TELEGRAM_OWNER_CHAT_ID kosong): perintah admin bot tidak bisa dipakai");
    }

    let data_dir = PathBuf::from(&cfg.doorbell_data_dir);
    std::fs::create_dir_all(&data_dir)?;
    let detector = Detector::new(&dcfg.zone, Some(data_dir.join("zone_empty.jpg")));

    let device_ip: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
    let tg = if cfg.telegram_bot_token.is_empty() { None } else { Some(TelegramApi::new(&cfg.telegram_bot_token)) };
    let notifier: Arc<dyn Notifier> = match &tg {
        Some(api) => Arc::new(TelegramNotifier(api.clone())),
        None => {
            tracing::warn!("TELEGRAM_BOT_TOKEN kosong: notifikasi hanya dicetak ke log (mode uji)");
            Arc::new(ConsoleNotifier)
        }
    };

    let (mqtt_client, eventloop) =
        mqtt::build_client(&cfg.mqtt_broker_host, cfg.mqtt_broker_port, &cfg.mqtt_username, &cfg.mqtt_password);

    let hub = Arc::new(Hub::new(
        dcfg.clone(),
        store,
        detector,
        managers,
        data_dir.join("photos"),
        HubDeps {
            camera: Arc::new(HttpCamera::new(dcfg.camera.clone(), device_ip.clone())),
            notifier,
            publisher: Arc::new(MqttPublisher(mqtt_client.clone())),
            clock: Arc::new(SystemClock),
            device_ip,
        },
    )?);
    hub.refresh_rooms().await;

    // --- MQTT ---
    tokio::spawn(mqtt::run(eventloop, mqtt_client, hub.clone(), dcfg.door_id.clone()));

    // --- bot Telegram (long polling) ---
    if let Some(api) = tg {
        let runner = BotRunner::new(Arc::new(api.clone()), hub.clone());
        tokio::spawn(async move { runner.run_forever(api).await });
    }

    // --- tick verifikasi/pengingat/retensi ---
    let tick_hub = hub.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            tick_hub.tick().await;
        }
    });

    // --- gRPC (dipanggil service api) ---
    let addr = format!("0.0.0.0:{}", cfg.doorbell_grpc_port).parse()?;
    tracing::info!("doorbell grpc listening di {addr}");
    Server::builder()
        .add_service(DoorbellServiceServer::new(grpc::DoorbellServiceImpl { hub }))
        .serve(addr)
        .await?;
    Ok(())
}
