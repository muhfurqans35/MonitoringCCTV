use serde::Deserialize;

/// Config dibaca dari environment variable (lihat .env.example di root project).
/// Tiap binary (api/ingest/alertengine) cukup memanggil `Config::load()` lalu
/// pakai field yang relevan untuknya.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_sqlite_path")]
    pub sqlite_path: String,

    #[serde(default = "default_influx_url")]
    pub influx_url: String,
    #[serde(default)]
    pub influx_token: String,
    #[serde(default = "default_influx_org")]
    pub influx_org: String,
    #[serde(default = "default_influx_bucket")]
    pub influx_bucket: String,

    #[serde(default = "default_mqtt_broker")]
    pub mqtt_broker_host: String,
    #[serde(default = "default_mqtt_port")]
    pub mqtt_broker_port: u16,
    #[serde(default)]
    pub mqtt_username: String,
    #[serde(default)]
    pub mqtt_password: String,

    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,
    #[serde(default = "default_ingest_grpc_addr")]
    pub ingest_grpc_addr: String,

    #[serde(default = "default_api_port")]
    pub api_port: u16,

    #[serde(default = "default_jwt_secret")]
    pub jwt_secret: String,

    #[serde(default)]
    pub telegram_bot_token: String,
    #[serde(default)]
    pub telegram_owner_chat_id: String,

    // --- modul doorbell (bel pintu interaktif) ---
    #[serde(default = "default_doorbell_grpc_port")]
    pub doorbell_grpc_port: u16,
    #[serde(default = "default_doorbell_grpc_addr")]
    pub doorbell_grpc_addr: String,
    #[serde(default = "default_doorbell_config")]
    pub doorbell_config: String,
    #[serde(default = "default_doorbell_data_dir")]
    pub doorbell_data_dir: String,
}

fn default_sqlite_path() -> String { "sqlite://./smartkost.db".into() }
fn default_influx_url() -> String { "http://localhost:8086".into() }
fn default_influx_org() -> String { "smartkost".into() }
fn default_influx_bucket() -> String { "telemetry".into() }
fn default_mqtt_broker() -> String { "localhost".into() }
fn default_mqtt_port() -> u16 { 1883 }
fn default_grpc_port() -> u16 { 9090 }
fn default_ingest_grpc_addr() -> String { "http://localhost:9090".into() }
fn default_api_port() -> u16 { 8080 }
fn default_doorbell_grpc_port() -> u16 { 9091 }
fn default_doorbell_grpc_addr() -> String { "http://localhost:9091".into() }
fn default_doorbell_config() -> String { "config/doorbell.toml".into() }
fn default_doorbell_data_dir() -> String { "data/doorbell".into() }
fn default_jwt_secret() -> String { "dev-secret-change-me".into() }

impl Config {
    /// Load .env (jika ada, dev only) lalu deserialize dari environment variables.
    /// Menggunakan crate `envy` yang otomatis mencocokkan env var (case-insensitive,
    /// SQLITE_PATH) ke field struct (sqlite_path).
    pub fn load() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();
        let cfg = envy::from_env::<Config>()
            .map_err(|e| anyhow::anyhow!("gagal load config dari env: {e}"))?;
        Ok(cfg)
    }
}
