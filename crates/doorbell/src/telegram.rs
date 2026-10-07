//! Klien Telegram Bot API tipis (reqwest) + notifier. Long polling dari server,
//! jadi tidak ada port yang dibuka ke internet.
//!
//! PENTING: hanya SATU proses boleh memanggil getUpdates untuk satu token bot.
//! Service alertengine hanya mengirim (sendMessage), jadi aman berbagi token.
//! Catatan privasi: pesan & foto melewati server Telegram.

use serde_json::{json, Value};
use tonic::async_trait;

use crate::traits::{Buttons, Notifier};

#[derive(Clone)]
pub struct TelegramApi {
    http: reqwest::Client,
    base: String,
}

fn markup(buttons: &Buttons) -> Value {
    let rows: Vec<Value> = buttons
        .iter()
        .map(|row| Value::Array(row.iter().map(|b| json!({ "text": b.text, "callback_data": b.data })).collect()))
        .collect();
    json!({ "inline_keyboard": rows })
}

impl TelegramApi {
    pub fn new(token: &str) -> Self {
        Self {
            http: reqwest::Client::builder().timeout(std::time::Duration::from_secs(45)).build().unwrap_or_default(),
            base: format!("https://api.telegram.org/bot{token}"),
        }
    }

    async fn parse(resp: reqwest::Response, method: &str) -> anyhow::Result<Value> {
        let status = resp.status();
        let j: Value = resp.json().await.map_err(|e| anyhow::anyhow!("{method}: respons bukan JSON (HTTP {status}): {e}"))?;
        if j.get("ok").and_then(Value::as_bool) != Some(true) {
            anyhow::bail!("{method}: {}", j.get("description").and_then(Value::as_str).unwrap_or("gagal"));
        }
        Ok(j["result"].clone())
    }

    async fn call(&self, method: &str, body: Value) -> anyhow::Result<Value> {
        let resp = self.http.post(format!("{}/{method}", self.base)).json(&body).send().await?;
        Self::parse(resp, method).await
    }

    pub async fn send_message(&self, chat_id: i64, text: &str, buttons: Option<&Buttons>) -> anyhow::Result<()> {
        let mut body = json!({ "chat_id": chat_id, "text": text });
        if let Some(b) = buttons {
            body["reply_markup"] = markup(b);
        }
        self.call("sendMessage", body).await.map(|_| ())
    }

    pub async fn send_photo(&self, chat_id: i64, photo: &[u8], caption: &str, buttons: Option<&Buttons>) -> anyhow::Result<()> {
        let part = reqwest::multipart::Part::bytes(photo.to_vec()).file_name("zona.jpg").mime_str("image/jpeg")?;
        let mut form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .text("caption", caption.chars().take(1000).collect::<String>())
            .part("photo", part);
        if let Some(b) = buttons {
            form = form.text("reply_markup", markup(b).to_string());
        }
        let resp = self.http.post(format!("{}/sendPhoto", self.base)).multipart(form).send().await?;
        Self::parse(resp, "sendPhoto").await.map(|_| ())
    }

    pub async fn answer_callback(&self, id: &str, text: &str) -> anyhow::Result<()> {
        let t: String = text.chars().take(190).collect();
        self.call("answerCallbackQuery", json!({ "callback_query_id": id, "text": t })).await.map(|_| ())
    }

    pub async fn clear_buttons(&self, chat_id: i64, message_id: i64) -> anyhow::Result<()> {
        self.call(
            "editMessageReplyMarkup",
            json!({ "chat_id": chat_id, "message_id": message_id, "reply_markup": { "inline_keyboard": [] } }),
        )
        .await
        .map(|_| ())
    }

    pub async fn get_updates(&self, offset: i64, timeout_s: u64) -> anyhow::Result<Vec<Value>> {
        let body = json!({ "offset": offset, "timeout": timeout_s, "allowed_updates": ["message", "callback_query"] });
        let resp = self
            .http
            .post(format!("{}/getUpdates", self.base))
            .timeout(std::time::Duration::from_secs(timeout_s + 10))
            .json(&body)
            .send()
            .await?;
        Ok(Self::parse(resp, "getUpdates").await?.as_array().cloned().unwrap_or_default())
    }
}

/// Notifier produksi: kegagalan Telegram hanya dicatat, tidak pernah menjatuhkan hub.
pub struct TelegramNotifier(pub TelegramApi);

#[async_trait]
impl Notifier for TelegramNotifier {
    async fn send(&self, chat_id: i64, text: &str, photo: Option<&[u8]>, buttons: Option<&Buttons>) {
        let res = match photo {
            Some(p) => self.0.send_photo(chat_id, p, text, buttons).await,
            None => self.0.send_message(chat_id, text, buttons).await,
        };
        if let Err(e) = res {
            tracing::warn!("kirim Telegram ke {chat_id} gagal: {e}");
        }
    }
}

/// Untuk mode tanpa token (uji lokal): cetak notifikasi ke log.
pub struct ConsoleNotifier;

#[async_trait]
impl Notifier for ConsoleNotifier {
    async fn send(&self, chat_id: i64, text: &str, photo: Option<&[u8]>, buttons: Option<&Buttons>) {
        let extra = photo.map(|p| format!(" [foto {} B]", p.len())).unwrap_or_default();
        let btn = buttons
            .map(|b| format!(" tombol={:?}", b.iter().flatten().map(|x| x.data.as_str()).collect::<Vec<_>>()))
            .unwrap_or_default();
        println!("[NOTIF -> {chat_id}]{extra} {text}{btn}");
    }
}
