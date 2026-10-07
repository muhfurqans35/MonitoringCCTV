mod authcore;
mod cli;
mod middleware_auth;
mod router;
mod routes;
mod security;
mod state;


use common::config::Config;
use common::db::init_pool;
use common::influx::InfluxClient;
use common::nvr_config::NvrConfig;
use proto::doorbell::doorbell_service_client::DoorbellServiceClient;
use proto::telemetry::telemetry_service_client::TelemetryServiceClient;
use state::AppState;
use tonic::transport::Endpoint;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cfg = Config::load()?;

    tracing::info!("api-gateway starting...");

    let db = init_pool(&cfg.sqlite_path).await?;

    // Mode CLI kelola akun: `api user ...`
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("user") {
        return cli::run(&args[2..], &db).await;
    }

    let influx = InfluxClient::new(&cfg.influx_url, &cfg.influx_org, &cfg.influx_bucket, &cfg.influx_token);

    // Koneksi gRPC ke ingest service. Pakai `connect_lazy` supaya api tetap bisa start
    // walau ingest belum siap saat startup — koneksi TCP baru benar-benar dibuka
    // saat request gRPC pertama dipanggil, otomatis retry sesuai kebutuhan.
    let endpoint = Endpoint::from_shared(cfg.ingest_grpc_addr.clone())?;
    let channel = endpoint.connect_lazy();
    let telemetry_client = TelemetryServiceClient::new(channel);
    let nvr_config_path = std::env::var("NVR_CONFIG").unwrap_or_else(|_| "config/nvr.toml".to_string());
    let nvr = NvrConfig::load(&nvr_config_path).unwrap_or_else(|e| {
        panic!(
            "gagal load config NVR dari {nvr_config_path}: {e}. Pastikan file config/nvr.toml ada di working directory dan berisi [[channels]] yang valid."
        );
    });

    // Idem untuk service doorbell (aksi dashboard: mode, kode pendaftaran, tutup paket, kalibrasi).
    let doorbell_channel = Endpoint::from_shared(cfg.doorbell_grpc_addr.clone())?.connect_lazy();
    let doorbell_client = DoorbellServiceClient::new(doorbell_channel);

    let state = AppState {
        db,
        influx,
        influx_bucket: cfg.influx_bucket.clone(),
        telemetry_client,
        doorbell_client,
        jwt_secret: cfg.jwt_secret.clone(),
        nvr: std::sync::Arc::new(nvr),
        session_cfg: security::SessionConfig::from_env(),
        limiter: security::Limiter::default(),
    };

    let users: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users").fetch_one(&state.db).await.unwrap_or((0,));
    if users.0 == 0 {
        tracing::warn!("belum ada akun dashboard. Buat dengan: bin/api user add admin owner");
    }
    // Siapkan hash pembanding login di thread blocking (bcrypt cost 12 ±1 dtk di Pi).
    tokio::task::spawn_blocking(|| {
        routes::auth::dummy_hash();
    });
    tokio::spawn(security::housekeeping(state.db.clone(), state.session_cfg.clone(), state.limiter.clone()));

    let app = router::build_router(state);

    // Default hanya localhost: dari luar Pi, API hanya bisa dicapai lewat nginx (yang menambah
    // header keamanan & IP asli). Docker: set API_BIND=0.0.0.0.
    let bind = std::env::var("API_BIND").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| "127.0.0.1".to_string());
    let addr = format!("{bind}:{}", cfg.api_port);
    tracing::info!("api-gateway listening di {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await?;

    Ok(())
}
