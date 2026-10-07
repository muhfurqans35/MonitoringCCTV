use reqwest::Client;

/// Client tipis untuk InfluxDB 2.x, dipakai lewat HTTP API langsung (write & query)
/// alih-alih SDK resmi supaya lebih ringan & dependency lebih sedikit — cocok untuk
/// footprint kecil di Raspberry Pi.
#[derive(Clone)]
pub struct InfluxClient {
    http: Client,
    base_url: String,
    org: String,
    bucket: String,
    token: String,
}

impl InfluxClient {
    pub fn new(base_url: &str, org: &str, bucket: &str, token: &str) -> Self {
        Self {
            http: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            org: org.to_string(),
            bucket: bucket.to_string(),
            token: token.to_string(),
        }
    }

    /// Tulis satu baris data dalam format InfluxDB Line Protocol.
    /// Contoh: write_point("power", &[("room_code","K01")], &[("watt", 123.4)], ts_nanos)
    pub async fn write_line(&self, line: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/api/v2/write?org={}&bucket={}&precision=ns",
            self.base_url, self.org, self.bucket
        );
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Token {}", self.token))
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(line.to_string())
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("influx write gagal ({status}): {body}");
        }
        Ok(())
    }

    /// Jalankan query Flux dan kembalikan response mentah (CSV) dari InfluxDB.
    /// Parsing CSV -> struct dilakukan di sisi caller (repository layer) sesuai
    /// kebutuhan masing-masing query.
    ///
    /// Body dikirim sebagai raw Flux text (Content-Type: application/vnd.flux),
    /// sesuai format yang diharapkan endpoint /api/v2/query InfluxDB.
    pub async fn query_flux(&self, flux: &str) -> anyhow::Result<String> {
        let url = format!("{}/api/v2/query?org={}", self.base_url, self.org);

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Token {}", self.token))
            .header("Accept", "application/csv")
            .header("Content-Type", "application/vnd.flux")
            .body(flux.to_string())
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("influx query gagal ({status}): {body}");
        }

        Ok(resp.text().await?)
    }
}

/// Helper untuk escape tag value pada line protocol (spasi & koma perlu di-escape).
pub fn escape_tag(value: &str) -> String {
    value.replace(' ', "\\ ").replace(',', "\\,").replace('=', "\\=")
}
