CREATE TABLE IF NOT EXISTS rooms (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    code            TEXT NOT NULL UNIQUE,
    floor           INTEGER NOT NULL DEFAULT 1,
    tariff_per_kwh  REAL NOT NULL DEFAULT 1500,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS tenants (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id           INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    name              TEXT NOT NULL,
    phone             TEXT NOT NULL DEFAULT '',
    telegram_chat_id  TEXT NOT NULL DEFAULT '',
    check_in_date     TEXT NOT NULL DEFAULT (datetime('now')),
    active            INTEGER NOT NULL DEFAULT 1,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at        TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_tenants_room_id ON tenants(room_id);

CREATE TABLE IF NOT EXISTS devices (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    device_uid    TEXT NOT NULL UNIQUE,
    room_id       INTEGER REFERENCES rooms(id) ON DELETE SET NULL,
    device_type   TEXT NOT NULL, -- room_node | corridor_node | gate_cam
    location      TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'offline',
    last_seen_at  TEXT NOT NULL DEFAULT (datetime('now')),
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_devices_room_id ON devices(room_id);

CREATE TABLE IF NOT EXISTS billing_records (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id       INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    period        TEXT NOT NULL, -- "2026-07"
    total_kwh     REAL NOT NULL DEFAULT 0,
    total_amount  REAL NOT NULL DEFAULT 0,
    status        TEXT NOT NULL DEFAULT 'draft',
    generated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(room_id, period)
);

CREATE TABLE IF NOT EXISTS alert_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    source      TEXT NOT NULL,
    alert_type  TEXT NOT NULL,
    severity    TEXT NOT NULL DEFAULT 'info',
    message     TEXT NOT NULL DEFAULT '',
    resolved    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_alert_events_created_at ON alert_events(created_at);

CREATE TABLE IF NOT EXISTS users (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    username       TEXT NOT NULL UNIQUE,
    password_hash  TEXT NOT NULL,
    role           TEXT NOT NULL DEFAULT 'owner', -- owner | tenant
    created_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
