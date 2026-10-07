-- Keamanan login: sesi di server (cookie HttpOnly) menggantikan JWT di localStorage,
-- sehingga sesi bisa dicabut (logout, keluar dari semua perangkat, ganti password).

CREATE TABLE IF NOT EXISTS sessions (
    id_hash     TEXT PRIMARY KEY,          -- SHA-256 hex dari token cookie (token asli tidak disimpan)
    username    TEXT NOT NULL,
    role        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,          -- unix detik
    last_seen   INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,          -- batas absolut
    ip          TEXT NOT NULL DEFAULT '',
    user_agent  TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_sessions_username ON sessions(username);
CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);

-- Jejak audit login/logout/ganti password (dibaca lewat sqlite3 atau `api user log`).
CREATE TABLE IF NOT EXISTS auth_events (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    ts        INTEGER NOT NULL,
    username  TEXT NOT NULL,
    ip        TEXT NOT NULL DEFAULT '',
    event     TEXT NOT NULL,               -- login_ok | login_fail | login_blocked | logout | logout_all | password_changed
    detail    TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_auth_events_ts ON auth_events(ts);
