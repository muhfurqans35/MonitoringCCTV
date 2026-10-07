/// Kirim pesan teks ke chat Telegram tertentu lewat Bot API.
/// Dipakai untuk notifikasi pemilik kost (alert kondisi anomali) maupun
/// penghuni kamar (reminder billing, dll — dipanggil dengan chat_id berbeda).
pub async fn send_telegram_message(bot_token: &str, chat_id: &str, text: &str) -> anyhow::Result<()> {
    if bot_token.is_empty() || chat_id.is_empty() {
        tracing::warn!("telegram bot_token/chat_id kosong, notifikasi dilewati: {text}");
        return Ok(());
    }

    let url = format!("https://api.telegram.org/bot{bot_token}/sendMessage");
    let client = reqwest::Client::new();

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "Markdown",
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("gagal kirim telegram ({status}): {body}");
    }

    Ok(())
}
