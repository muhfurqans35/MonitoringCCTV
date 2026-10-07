//! Penangan perintah & tombol bot Telegram.

use std::sync::Arc;

use serde_json::Value;
use tonic::async_trait;

use crate::hub::{Actor, Hub};
use crate::telegram::TelegramApi;

/// Bagian API Telegram yang dipakai bot (bisa ditiru saat uji).
#[async_trait]
pub trait BotApi: Send + Sync {
    async fn send_message(&self, chat_id: i64, text: &str);
    async fn answer_callback(&self, id: &str, text: &str);
    async fn clear_buttons(&self, chat_id: i64, message_id: i64);
}

#[async_trait]
impl BotApi for TelegramApi {
    async fn send_message(&self, chat_id: i64, text: &str) {
        if let Err(e) = TelegramApi::send_message(self, chat_id, text, None).await {
            tracing::warn!("balas Telegram gagal: {e}");
        }
    }
    async fn answer_callback(&self, id: &str, text: &str) {
        let _ = TelegramApi::answer_callback(self, id, text).await;
    }
    async fn clear_buttons(&self, chat_id: i64, message_id: i64) {
        let _ = TelegramApi::clear_buttons(self, chat_id, message_id).await;
    }
}

const HELP_RESIDENT: &str = "Halo! Bot ini memberi tahu Anda saat ada paket untuk kamar Anda.\n\
Untuk mendaftar, minta kode ke pengelola lalu kirim: /start KODE\n\
Data yang disimpan: nomor kamar dan ID Telegram Anda. Ketik /berhenti kapan saja untuk berhenti.";
const HELP_MANAGER: &str = "\n\nPerintah pengelola:\n/kode <kamar> - buat kode pendaftaran penghuni\n\
/paket - daftar paket menunggu\n/tutup <id> - tutup catatan paket\n\
/mode [nama] - lihat/ganti mode\n/kalibrasi - simpan referensi zona paket KOSONG\n/status - status sistem\n\
/foto - foto terbaru dari kamera pintu";

pub struct BotRunner {
    pub api: Arc<dyn BotApi>,
    pub hub: Arc<Hub>,
}

impl BotRunner {
    pub fn new(api: Arc<dyn BotApi>, hub: Arc<Hub>) -> Self {
        Self { api, hub }
    }

    /// Long polling tanpa henti, dengan backoff saat jaringan bermasalah.
    pub async fn run_forever(&self, tg: TelegramApi) {
        let (mut offset, mut backoff) = (0i64, 1u64);
        loop {
            match tg.get_updates(offset, 25).await {
                Ok(updates) => {
                    backoff = 1;
                    for u in updates {
                        offset = u["update_id"].as_i64().unwrap_or(offset) + 1;
                        self.handle_update(&u).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("polling Telegram gagal: {e} (coba lagi {backoff}s)");
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    }

    pub async fn handle_update(&self, u: &Value) {
        if let Some(cb) = u.get("callback_query") {
            self.on_callback(cb).await;
        } else if let Some(m) = u.get("message") {
            self.on_message(m).await;
        }
    }

    async fn reply(&self, chat_id: i64, text: &str) {
        self.api.send_message(chat_id, text).await;
    }

    async fn on_message(&self, m: &Value) {
        // Abaikan grup: notifikasi berisi foto hanya ke chat pribadi.
        if m["chat"]["type"].as_str() != Some("private") {
            return;
        }
        let Some(chat_id) = m["chat"]["id"].as_i64() else { return };
        let text = m["text"].as_str().unwrap_or("").trim().to_string();
        let mut parts = text.split_whitespace();
        let Some(first) = parts.next() else { return };
        let cmd = first.split('@').next().unwrap_or("").to_lowercase();
        let args: Vec<&str> = parts.collect();
        let is_mgr = self.hub.is_manager(chat_id);
        let help = format!("{HELP_RESIDENT}{}", if is_mgr { HELP_MANAGER } else { "" });

        match cmd.as_str() {
            "/start" => {
                let Some(code) = args.first() else { return self.reply(chat_id, &help).await };
                let (ok, res) = self.hub.register(chat_id, code).await;
                if !ok {
                    return self.reply(chat_id, &res).await;
                }
                return self
                    .reply(chat_id, &format!(
                        "Terdaftar untuk kamar {res}. Anda akan menerima notifikasi paket (termasuk foto area paket) untuk kamar ini. \
                         Ketik /berhenti kapan saja untuk berhenti dan data Anda dinonaktifkan."))
                    .await;
            }
            "/berhenti" => {
                let rooms = self.hub.unregister(chat_id).await;
                let msg = if rooms.is_empty() { "Anda tidak sedang terdaftar.".to_string() } else { format!("Notifikasi dihentikan untuk kamar: {}", rooms.join(", ")) };
                return self.reply(chat_id, &msg).await;
            }
            "/help" => return self.reply(chat_id, &help).await,
            _ => {}
        }

        let mgr_cmds = ["/kode", "/paket", "/tutup", "/mode", "/kalibrasi", "/status", "/foto"];
        if !mgr_cmds.contains(&cmd.as_str()) {
            return self.reply(chat_id, "Perintah tidak dikenal. Ketik /help.").await;
        }
        if !is_mgr {
            return self.reply(chat_id, "Perintah ini hanya untuk pengelola.").await;
        }
        match cmd.as_str() {
            "/kode" => {
                let Some(room) = args.first() else { return self.reply(chat_id, "Format: /kode <kode kamar>").await };
                match self.hub.create_invite(room).await {
                    None => self.reply(chat_id, &format!("Kamar '{room}' tidak ada di tabel rooms.")).await,
                    Some(code) => {
                        self.reply(chat_id, &format!(
                            "Kode kamar {room}: {code} (sekali pakai, berlaku {} jam).\nPenghuni mengirim ke bot ini: /start {code}",
                            self.hub.cfg.retention.invite_ttl_h)).await
                    }
                }
            }
            "/paket" => self.reply(chat_id, &self.hub.waiting_text().await).await,
            "/tutup" => match args.first().and_then(|a| a.parse::<i64>().ok()) {
                Some(id) => {
                    let (_, msg) = self.hub.on_taken(id, Actor::Chat(chat_id)).await;
                    self.reply(chat_id, &msg).await
                }
                None => self.reply(chat_id, "Format: /tutup <id>").await,
            },
            "/mode" => {
                let mut modes: Vec<&String> = self.hub.cfg.texts.keys().collect();
                modes.sort();
                let list = modes.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
                match args.first() {
                    None => self.reply(chat_id, &format!("Mode aktif: {}. Pilihan: {list}", self.hub.mode().await)).await,
                    Some(m) => {
                        if self.hub.set_mode(m).await {
                            self.reply(chat_id, &format!("Mode diganti ke '{m}'. Layar diperbarui saat node idle.")).await
                        } else {
                            self.reply(chat_id, &format!("Mode tidak dikenal. Pilihan: {list}")).await
                        }
                    }
                }
            }
            "/kalibrasi" => {
                let (_, msg) = self.hub.calibrate().await;
                self.reply(chat_id, &msg).await
            }
            "/status" => self.reply(chat_id, &self.hub.status_text().await).await,
            "/foto" => {
                if let Err(e) = self.hub.send_snapshot(chat_id).await {
                    self.reply(chat_id, &format!("Gagal mengambil foto kamera pintu: {e}")).await
                }
            }
            _ => {}
        }
    }

    async fn on_callback(&self, cb: &Value) {
        let Some(id) = cb["id"].as_str() else { return };
        let data = cb["data"].as_str().unwrap_or("");
        let msg = &cb["message"];
        let chat_id = msg["chat"]["id"].as_i64().or_else(|| cb["from"]["id"].as_i64());
        let (action, sid) = data.split_once(':').unwrap_or((data, ""));
        let (Some(chat_id), Ok(sub_id)) = (chat_id, sid.parse::<i64>()) else {
            return self.api.answer_callback(id, "Tombol tidak dikenal.").await;
        };
        let (ok, text) = match action {
            "taken" => self.hub.on_taken(sub_id, Actor::Chat(chat_id)).await,
            "notmine" => self.hub.on_notmine(sub_id, Actor::Chat(chat_id)).await,
            _ => return self.api.answer_callback(id, "Tombol tidak dikenal.").await,
        };
        self.api.answer_callback(id, &text).await;
        if ok {
            if let Some(mid) = msg["message_id"].as_i64() {
                self.api.clear_buttons(chat_id, mid).await;
            }
        }
    }
}
