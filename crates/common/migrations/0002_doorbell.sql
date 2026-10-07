-- Doorbell interaktif (paket kost). Waktu disimpan sebagai epoch detik (REAL) untuk logika
-- verifikasi/pengingat; gunakan datetime(ts,'unixepoch') untuk tampilan.
-- Privasi: hanya kode kamar + chat_id Telegram (tanpa nama), foto hanya potongan ROI dengan retensi.

CREATE TABLE IF NOT EXISTS doorbell_subscribers (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    room_code   TEXT NOT NULL,
    chat_id     INTEGER NOT NULL,
    active      INTEGER NOT NULL DEFAULT 1,
    consent_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(room_code, chat_id)
);
CREATE INDEX IF NOT EXISTS idx_doorbell_subscribers_room ON doorbell_subscribers(room_code);

CREATE TABLE IF NOT EXISTS doorbell_invites (
    code        TEXT PRIMARY KEY,
    room_code   TEXT NOT NULL,
    created_at  REAL NOT NULL,
    used        INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS doorbell_submissions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    sid           TEXT UNIQUE,
    door_id       TEXT,
    ts            REAL NOT NULL,
    purpose       TEXT NOT NULL,
    room_code     TEXT,
    status        TEXT NOT NULL,   -- new | verifying | waiting | taken | disputed | done
    verified      INTEGER NOT NULL DEFAULT 0,
    late          INTEGER NOT NULL DEFAULT 0,
    photo_path    TEXT,
    taken_at      REAL,
    reminded_at   REAL,
    remind_count  INTEGER NOT NULL DEFAULT 0,
    note          TEXT
);
CREATE INDEX IF NOT EXISTS idx_doorbell_submissions_status ON doorbell_submissions(status);
CREATE INDEX IF NOT EXISTS idx_doorbell_submissions_ts ON doorbell_submissions(ts);

CREATE TABLE IF NOT EXISTS doorbell_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          REAL NOT NULL,
    type        TEXT NOT NULL,
    room_code   TEXT,
    zone_state  TEXT,
    score       REAL,
    latency     REAL,
    detail      TEXT
);
CREATE INDEX IF NOT EXISTS idx_doorbell_events_ts ON doorbell_events(ts);

CREATE TABLE IF NOT EXISTS doorbell_settings (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL
);
