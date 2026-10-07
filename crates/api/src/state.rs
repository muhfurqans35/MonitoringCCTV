use common::influx::InfluxClient;
use common::nvr_config::NvrConfig;
use proto::doorbell::doorbell_service_client::DoorbellServiceClient;
use proto::telemetry::telemetry_service_client::TelemetryServiceClient;
use sqlx::SqlitePool;
use tonic::transport::Channel;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub influx: InfluxClient,
    pub influx_bucket: String,
    pub telemetry_client: TelemetryServiceClient<Channel>,
    pub doorbell_client: DoorbellServiceClient<Channel>,
    pub jwt_secret: String,
    pub nvr: std::sync::Arc<NvrConfig>,
    pub session_cfg: crate::security::SessionConfig,
    pub limiter: crate::security::Limiter,
}

impl AppState {
    pub fn influx_bucket_name(&self) -> &str {
        &self.influx_bucket
    }
}