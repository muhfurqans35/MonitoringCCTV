//! Akses SQLite (sqlx, query runtime: tidak butuh DATABASE_URL saat compile).

use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Submission {
    pub id: i64,
    pub sid: Option<String>,
    pub door_id: Option<String>,
    pub ts: f64,
    pub purpose: String,
    pub room_code: Option<String>,
    pub status: String,
    pub verified: i64,
    pub late: i64,
    pub photo_path: Option<String>,
    pub taken_at: Option<f64>,
    pub reminded_at: Option<f64>,
    pub remind_count: i64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct DayStats {
    pub total: i64,
    pub paket: i64,
    pub verified: i64,
    pub waiting: i64,
}

#[derive(Clone)]
pub struct Store {
    pub pool: SqlitePool,
}

impl Store {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // ---- pengaturan
    pub async fn get_setting(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT value FROM doorbell_settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO doorbell_settings(key, value) VALUES(?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---- kamar (sumber kebenaran: tabel rooms milik smart-kost)
    pub async fn rooms(&self) -> anyhow::Result<Vec<String>> {
        Ok(sqlx::query_scalar("SELECT code FROM rooms ORDER BY code").fetch_all(&self.pool).await?)
    }

    // ---- undangan & pelanggan notifikasi
    pub async fn add_invite(&self, code: &str, room: &str, ts: f64) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO doorbell_invites(code, room_code, created_at) VALUES(?, ?, ?)")
            .bind(code)
            .bind(room)
            .bind(ts)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Ambil undangan yang masih berlaku dan tandai terpakai. Mengembalikan kode kamar.
    pub async fn take_invite(&self, code: &str, now: f64, ttl_s: f64) -> anyhow::Result<Option<String>> {
        let row: Option<(String, f64)> =
            sqlx::query_as("SELECT room_code, created_at FROM doorbell_invites WHERE code = ? AND used = 0")
                .bind(code)
                .fetch_optional(&self.pool)
                .await?;
        let Some((room, created)) = row else { return Ok(None) };
        if now - created > ttl_s {
            return Ok(None);
        }
        // UPDATE bersyarat: menjamin sekali pakai walau dua permintaan bersamaan.
        let res = sqlx::query("UPDATE doorbell_invites SET used = 1 WHERE code = ? AND used = 0")
            .bind(code)
            .execute(&self.pool)
            .await?;
        Ok(if res.rows_affected() == 1 { Some(room) } else { None })
    }

    pub async fn add_subscriber(&self, room: &str, chat_id: i64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO doorbell_subscribers(room_code, chat_id, active, consent_at) \
             VALUES(?, ?, 1, datetime('now')) \
             ON CONFLICT(room_code, chat_id) DO UPDATE SET active = 1, consent_at = datetime('now')",
        )
        .bind(room)
        .bind(chat_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn deactivate_chat(&self, chat_id: i64) -> anyhow::Result<Vec<String>> {
        let rooms: Vec<String> =
            sqlx::query_scalar("SELECT room_code FROM doorbell_subscribers WHERE chat_id = ? AND active = 1")
                .bind(chat_id)
                .fetch_all(&self.pool)
                .await?;
        sqlx::query("UPDATE doorbell_subscribers SET active = 0 WHERE chat_id = ?")
            .bind(chat_id)
            .execute(&self.pool)
            .await?;
        Ok(rooms)
    }

    pub async fn active_chats(&self, room: &str) -> anyhow::Result<Vec<i64>> {
        Ok(sqlx::query_scalar("SELECT chat_id FROM doorbell_subscribers WHERE room_code = ? AND active = 1")
            .bind(room)
            .fetch_all(&self.pool)
            .await?)
    }

    pub async fn is_subscriber(&self, room: &str, chat_id: i64) -> anyhow::Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM doorbell_subscribers WHERE room_code = ? AND chat_id = ? AND active = 1",
        )
        .bind(room)
        .bind(chat_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    // ---- kiriman / paket
    /// None bila `sid` sudah pernah tercatat (kiriman ganda / kirim ulang antrean).
    pub async fn add_submission(
        &self,
        sid: &str,
        door_id: &str,
        ts: f64,
        purpose: &str,
        room: Option<&str>,
        late: bool,
    ) -> anyhow::Result<Option<i64>> {
        let res = sqlx::query(
            "INSERT INTO doorbell_submissions(sid, door_id, ts, purpose, room_code, status, late) \
             VALUES(?, ?, ?, ?, ?, 'new', ?)",
        )
        .bind(sid)
        .bind(door_id)
        .bind(ts)
        .bind(purpose)
        .bind(room)
        .bind(late as i64)
        .execute(&self.pool)
        .await;
        match res {
            Ok(r) => Ok(Some(r.last_insert_rowid())),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn get_submission(&self, id: i64) -> anyhow::Result<Option<Submission>> {
        Ok(sqlx::query_as("SELECT * FROM doorbell_submissions WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn mark_verifying(&self, id: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE doorbell_submissions SET status = 'verifying' WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_done(&self, id: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE doorbell_submissions SET status = 'done' WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_waiting(
        &self,
        id: i64,
        verified: bool,
        photo_path: Option<&str>,
        note: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE doorbell_submissions SET status = 'waiting', verified = ?, photo_path = ?, note = ? WHERE id = ?")
            .bind(verified as i64)
            .bind(photo_path)
            .bind(note)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_taken(&self, id: i64, now: f64, note: Option<&str>) -> anyhow::Result<()> {
        sqlx::query("UPDATE doorbell_submissions SET status = 'taken', taken_at = ?, note = COALESCE(?, note) WHERE id = ?")
            .bind(now)
            .bind(note)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_disputed(&self, id: i64, note: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE doorbell_submissions SET status = 'disputed', note = ? WHERE id = ?")
            .bind(note)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn clear_photo(&self, id: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE doorbell_submissions SET photo_path = NULL WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_reminded(&self, id: i64, now: f64, count: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE doorbell_submissions SET reminded_at = ?, remind_count = ? WHERE id = ?")
            .bind(now)
            .bind(count)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn waiting(&self, verified_only: bool) -> anyhow::Result<Vec<Submission>> {
        let sql = if verified_only {
            "SELECT * FROM doorbell_submissions WHERE status = 'waiting' AND verified = 1 ORDER BY ts"
        } else {
            "SELECT * FROM doorbell_submissions WHERE status = 'waiting' ORDER BY ts"
        };
        Ok(sqlx::query_as(sql).fetch_all(&self.pool).await?)
    }

    pub async fn due_reminders(&self, now: f64, after_s: f64, max_n: i64) -> anyhow::Result<Vec<Submission>> {
        Ok(sqlx::query_as(
            "SELECT * FROM doorbell_submissions WHERE status = 'waiting' AND remind_count < ? AND ts <= ? \
             AND (reminded_at IS NULL OR reminded_at <= ?)",
        )
        .bind(max_n)
        .bind(now - after_s)
        .bind(now - after_s)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn old_photos(&self, cutoff: f64) -> anyhow::Result<Vec<(i64, String)>> {
        Ok(sqlx::query_as("SELECT id, photo_path FROM doorbell_submissions WHERE photo_path IS NOT NULL AND ts < ?")
            .bind(cutoff)
            .fetch_all(&self.pool)
            .await?)
    }

    pub async fn stats_since(&self, ts: f64) -> anyhow::Result<DayStats> {
        let one = |sql: &'static str| {
            let pool = self.pool.clone();
            async move { sqlx::query_scalar::<_, i64>(sql).bind(ts).fetch_one(&pool).await }
        };
        Ok(DayStats {
            total: one("SELECT COUNT(*) FROM doorbell_submissions WHERE ts >= ?").await?,
            paket: one("SELECT COUNT(*) FROM doorbell_submissions WHERE ts >= ? AND purpose = 'paket'").await?,
            verified: one("SELECT COUNT(*) FROM doorbell_submissions WHERE ts >= ? AND verified = 1").await?,
            waiting: sqlx::query_scalar("SELECT COUNT(*) FROM doorbell_submissions WHERE status = 'waiting'")
                .fetch_one(&self.pool)
                .await?,
        })
    }

    // ---- log event
    pub async fn add_event(
        &self,
        ts: f64,
        type_: &str,
        room: Option<&str>,
        zone_state: Option<&str>,
        score: Option<f64>,
        latency: Option<f64>,
        detail: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO doorbell_events(ts, type, room_code, zone_state, score, latency, detail) \
             VALUES(?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(ts)
        .bind(type_)
        .bind(room)
        .bind(zone_state)
        .bind(score)
        .bind(latency)
        .bind(detail)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---- integrasi dengan tabel milik smart-kost
    /// Daftarkan/perbarui node pintu di tabel `devices` (dilihat dashboard bersama sensor lain).
    pub async fn upsert_door_device(&self, uid: &str, location: &str, online: bool) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO devices(device_uid, device_type, location, status, last_seen_at) \
             VALUES(?, 'door_node', ?, ?, datetime('now')) \
             ON CONFLICT(device_uid) DO UPDATE SET status = excluded.status, location = excluded.location, \
             last_seen_at = datetime('now'), updated_at = datetime('now')",
        )
        .bind(uid)
        .bind(location)
        .bind(if online { "online" } else { "offline" })
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Tulis ke `alert_events` supaya muncul di AlertFeed dashboard yang sudah ada.
    pub async fn insert_alert(&self, source: &str, alert_type: &str, severity: &str, message: &str) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO alert_events (source, alert_type, severity, message) VALUES (?, ?, ?, ?)")
            .bind(source)
            .bind(alert_type)
            .bind(severity)
            .bind(message)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
