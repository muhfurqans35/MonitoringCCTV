# Struktur Folder (Stack Rust)

```
smart-kost/
├── Cargo.toml                       # workspace root, daftar member crate + shared dependency versions
├── docker-compose.yml
├── .env.example
├── .dockerignore
│
├── deploy/
│   ├── mosquitto/mosquitto.conf     # config broker MQTT
│   ├── caddy/Caddyfile              # reverse proxy (proxy /api,/ws ke axum; sisanya ke nginx frontend)
│   ├── Dockerfile.ingest            # multi-stage build binary ingest
│   ├── Dockerfile.api               # multi-stage build binary api
│   ├── Dockerfile.alertengine       # multi-stage build binary alertengine
│   ├── Dockerfile.doorbell          # multi-stage build binary doorbell
│   ├── systemd/smartkost-doorbell.service
│   └── Dockerfile.frontend          # build wasm via trunk, serve dengan nginx
│
├── crates/
│   ├── common/                      # shared lib dipakai semua binary
│   │   ├── Cargo.toml
│   │   ├── migrations/0001_init.sql # skema SQLite (rooms, tenants, devices, billing, alerts, users)
│   │   ├── migrations/0002_doorbell.sql # tabel doorbell_* (pelanggan notifikasi, undangan, kiriman/paket, event)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs            # load Config dari env var (pakai crate envy)
│   │       ├── db.rs                # init SQLite pool (sqlx) + auto-run migration
│   │       ├── influx.rs            # client tipis InfluxDB (reqwest: write line protocol + query flux)
│   │       ├── auth.rs              # sign/verify JWT (jsonwebtoken)
│   │       ├── error.rs             # AppError terintegrasi ke axum::response::IntoResponse
│   │       └── models.rs            # struct data: Room, Tenant, Device, BillingRecord, AlertEvent,
│   │                                 #   RoomTelemetryPayload, CamEventPayload (payload MQTT)
│   │
│   ├── doorbell/                    # binary: bel pintu interaktif (paket kost) — lihat docs/DOORBELL.md
│   │   └── src/                     # hub.rs, detector.rs, store.rs, mqtt.rs, telegram.rs, bot.rs, grpc.rs, tests.rs
│   │
│   ├── proto/                       # kontrak gRPC (tonic)
│   │   ├── Cargo.toml
│   │   ├── build.rs                 # codegen .proto -> Rust via tonic-build
│   │   ├── proto/telemetry.proto    # TelemetryService: GetLiveSnapshot, StreamRoomTelemetry
│   │   ├── proto/alert.proto        # AlertService: StreamAlerts
│   │   ├── proto/doorbell.proto     # DoorbellService: GetStatus, SetMode, CreateInvite, ClosePackage, Calibrate
│   │   └── src/lib.rs               # expose modul hasil generate (tonic::include_proto!)
│   │
│   ├── ingest/                      # binary: subscriber MQTT + gRPC server
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs              # setup db/influx/mqtt/grpc, jalankan paralel (tokio::spawn)
│   │       ├── state.rs             # SharedState: snapshot in-memory + broadcast channel
│   │       ├── mqtt.rs              # rumqttc client, subscribe topic, parsing payload, tulis influx+sqlite
│   │       └── grpc.rs              # implementasi TelemetryService & AlertService (tonic)
│   │
│   ├── api/                         # binary: REST + WebSocket gateway (axum)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs              # setup db/influx/grpc-client, jalankan axum server
│   │       ├── state.rs             # AppState (pool sqlx, influx client, grpc client, jwt secret)
│   │       ├── router.rs            # susunan seluruh route (public vs protected by JWT middleware)
│   │       ├── middleware_auth.rs   # verifikasi Authorization: Bearer <jwt>
│   │       └── routes/
│   │           ├── mod.rs
│   │           ├── auth.rs          # POST /api/auth/login
│   │           ├── rooms.rs         # GET /api/rooms, /api/rooms/:code, /api/rooms/live (via grpc)
│   │           ├── history.rs       # GET /api/rooms/:code/history/power|occupancy (query Flux)
│   │           ├── alerts.rs        # GET /api/alerts, POST /api/alerts/:id/resolve
│   │           ├── billing.rs       # GET/POST /api/billing/:room_code[...]
│   │           ├── doorbell.rs      # /api/doorbell/* (status, paket, mode, kode pendaftaran, kalibrasi)
│   │           └── ws.rs            # GET /ws — relay gRPC stream ingest -> browser
│   │
│   └── alertengine/                 # binary: worker evaluasi alert + notifier
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs              # consume grpc alert stream + periodic rule check (tokio::spawn)
│           ├── rules.rs             # aturan anomali (power vs occupancy) query InfluxDB
│           └── notifier.rs          # kirim pesan ke Telegram Bot API (reqwest)
│
└── frontend-leptos/                 # dashboard client-side Rust -> WASM (build dengan trunk)
    ├── Cargo.toml
    ├── index.html                   # entry trunk
    ├── public/style.css
    └── src/
        ├── main.rs                  # mount App ke <body>
        ├── app.rs                   # root component: LoginView, DashboardView (routing state sederhana)
        ├── api.rs                   # client REST ke backend axum (gloo-net, simpan JWT di localStorage)
        ├── ws.rs                    # koneksi WebSocket /ws untuk update realtime
        ├── components.rs            # RoomCard, AlertFeed
        └── doorbell.rs              # halaman "Bel & Paket"
```

Di luar workspace Rust: `tools/doorbell/` (alat bantu Python). Firmware node (ESP32-S3, ESP-IDF native) ada di
paket terpisah `doorbell-firmware-esp-idf.zip` — lihat `firmware/README.md` di sini untuk penunjuknya.
Konfigurasi baru: `config/doorbell.toml`.

## Alur data end-to-end

1. ESP32 kamar publish payload JSON ke topic `kost/room/{code}/telemetry` setiap beberapa detik.
2. CCTV/edge box publish event (motion/loitering) ke topic `kost/cam/{camera_id}/event`.
3. `ingest` (subscriber via `rumqttc`) parse payload → tulis point ke InfluxDB (measurement
   `occupancy`, `power`, `cctv_event`) → update `devices.status`/`last_seen_at` di SQLite (sqlx)
   → update in-memory snapshot & broadcast lewat `tokio::sync::broadcast` channel.
4. Event kamera bertipe `loitering`/`line_cross` langsung di-emit sebagai kandidat alert lewat
   channel terpisah, di-stream ke `alertengine` via gRPC (`AlertService::StreamAlerts`).
5. `alertengine` juga menjalankan pengecekan periodik (tiap 5 menit) terhadap pola "kamar kosong
   tapi daya tinggi" dari InfluxDB → kalau match, insert `AlertEvent` ke SQLite + kirim Telegram.
6. `api` expose REST endpoint (rooms, history, alerts, billing) untuk frontend, dan endpoint
   `/api/rooms/live` memanggil `ingest` via gRPC client (`TelemetryService::GetLiveSnapshot`)
   untuk data "sekarang" tanpa query InfluxDB (latensi lebih rendah).
7. Frontend Leptos (WASM) buka WebSocket ke `/ws`; `api` relay stream gRPC
   (`StreamRoomTelemetry`) dari `ingest` ke browser sebagai JSON — dashboard update realtime
   tanpa polling.
8. ESP32 bel pintu publish kiriman dari layar ke `kost/door/{id}/event/submit`. `doorbell` menarik foto zona
   lewat HTTP dari node, memverifikasi perubahan, lalu mengirim notifikasi Telegram ke penghuni kamar itu dan
   menulis `alert_events` (paket tak terverifikasi/dibantah, node offline) yang ikut tampil di dashboard.
   Aksi dari dashboard (mode, kode pendaftaran, tutup paket) lewat `api` → gRPC → `doorbell`.
