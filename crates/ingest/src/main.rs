mod grpc;
mod mqtt;
mod state;

use common::config::Config;
use common::db::init_pool;
use common::influx::InfluxClient;
use proto::telemetry::telemetry_service_server::TelemetryServiceServer;
use proto::alert::alert_service_server::AlertServiceServer;
use state::SharedState;
use tonic::transport::Server;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cfg = Config::load()?;

    tracing::info!("ingest service starting...");

    let pool = init_pool(&cfg.sqlite_path).await?;
    let influx = InfluxClient::new(&cfg.influx_url, &cfg.influx_org, &cfg.influx_bucket, &cfg.influx_token);
    let shared_state = SharedState::new();

    // --- MQTT subscriber task ---
    let (mqtt_client, eventloop) = mqtt::build_client(
        &cfg.mqtt_broker_host,
        cfg.mqtt_broker_port,
        &cfg.mqtt_username,
        &cfg.mqtt_password,
    );
    mqtt::subscribe_all(&mqtt_client).await?;

    let mqtt_pool = pool.clone();
    let mqtt_influx = influx.clone();
    let mqtt_state = shared_state.clone();
    tokio::spawn(async move {
        mqtt::run_event_loop(eventloop, mqtt_pool, mqtt_influx, mqtt_state).await;
    });

    // --- gRPC server task ---
    let grpc_addr = format!("0.0.0.0:{}", cfg.grpc_port).parse()?;
    let telemetry_svc = grpc::TelemetryServiceImpl { state: shared_state.clone() };
    let alert_svc = grpc::AlertServiceImpl { state: shared_state.clone() };

    tracing::info!("grpc server listening di {grpc_addr}");
    Server::builder()
        .add_service(TelemetryServiceServer::new(telemetry_svc))
        .add_service(AlertServiceServer::new(alert_svc))
        .serve(grpc_addr)
        .await?;

    Ok(())
}
