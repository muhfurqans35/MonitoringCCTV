use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;

/// Inisialisasi connection pool SQLite (via sqlx) dan jalankan migration
/// dari folder crates/common/migrations. Dipanggil sekali di startup tiap binary
/// (api, ingest, alertengine) — semua share skema yang sama.
///
/// `database_url` menerima format sqlx standar, misal: "sqlite:///data/smartkost.db".
/// File database otomatis dibuat kalau belum ada (create_if_missing).
pub async fn init_pool(database_url: &str) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(database_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}
