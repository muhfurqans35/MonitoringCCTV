# Smart Kost — Integrated Monitoring & Security System (Rust Stack)

Sistem monitoring kost terintegrasi: deteksi hunian kamar (mmWave), pemakaian daya (sensor tegangan),
CCTV koridor/tangga/gerbang, bel pintu interaktif untuk paket (layar sentuh + verifikasi kamera),
dan notifikasi via Telegram/WA bot.

**Stack:** Rust + gRPC (tonic) + Axum + Leptos (WASM) + Tokio + SQLx + rumqttc, storage SQLite
(relational) + InfluxDB (time-series), broker MQTT (Mosquitto), reverse proxy Caddy.
Didesain untuk jalan di Raspberry Pi 4B (8GB RAM).

## Arsitektur singkat

```
ESP32 (per kamar) ---MQTT---> Mosquitto ---> ingest (rumqttc subscriber)
CCTV (event)       ---MQTT---/                    |
ESP32 bel pintu    ---MQTT---> doorbell (Rust) <--HTTP /capture-- node   (paket, Telegram, gRPC ke api)
                                                    v
                                     SQLite (sqlx, state/relational)
                                     InfluxDB (time-series)
                                                    |
                                          gRPC (tonic, internal)
                                                    v
                                    api-gateway (axum REST + WebSocket)
                                                    |
                              +---------------------+----------------------+
                              v                                            v
                   Leptos (WASM) dashboard                     alertengine -> Telegram bot
```

- **ingest** (`crates/ingest`): subscribe topic MQTT dari ESP32/CCTV via `rumqttc`, tulis raw
  telemetry ke InfluxDB, update status device di SQLite. Expose gRPC server (`tonic`) untuk
  live snapshot & streaming ke service lain.
- **api** (`crates/api`): REST + WebSocket (`axum`) untuk frontend & auth. Jadi gRPC client ke
  ingest untuk data live tanpa re-query InfluxDB.
- **alertengine** (`crates/alertengine`): consumer gRPC alert stream dari ingest (event CCTV) +
  worker periodik (evaluasi anomali daya vs occupancy dari InfluxDB), simpan `AlertEvent` ke SQLite,
  kirim notifikasi via Telegram Bot API.
- **doorbell** (`crates/doorbell`): modul bel pintu interaktif. Menerima kiriman dari layar sentuh node
  ESP32 (`kost/door/{id}/...`), memverifikasi lewat kamera bahwa paket benar-benar ditaruh di zona paket,
  memberi tahu penghuni yang tepat lewat Telegram, dan menutup catatan saat paket diambil. Memakai tabel
  `rooms`, `devices`, `alert_events` yang sudah ada; `api` memanggilnya lewat gRPC. **Lihat
  [`docs/DOORBELL.md`](docs/DOORBELL.md)** (protokol, konfigurasi, bring-up node, privasi).
- **frontend-leptos**: dashboard client-side Rust→WASM (Leptos), realtime lewat WebSocket,
  di-build dengan `trunk`, disajikan sebagai static file oleh nginx dalam container.
- **common** (`crates/common`): shared config, model (sqlx `FromRow`), error handling, client
  InfluxDB tipis (reqwest, line protocol + Flux), JWT.
- **proto** (`crates/proto`): kontrak gRPC (`telemetry.proto`, `alert.proto`) di-generate dengan
  `tonic-build` (ditambah `doorbell.proto`).

## Dua cara deploy

1. **Docker Compose** (lihat langkah di bawah) — gampang untuk development/testing cepat di
   laptop, tapi untuk Raspberry Pi dengan SD card, build image Docker bisa memakan storage besar.
2. **Native production di Raspi, tanpa Docker sama sekali di Raspi** (direkomendasikan) — compile
   dilakukan di PC/laptop via cross-compilation (`cross`), hasil binary dikirim ke Raspi lewat
   `scp`, Raspi cuma menjalankan proses biasa (systemd), tidak pernah compile apa pun.
   **Lihat panduan lengkap di [`docs/DEPLOY_NATIVE.md`](docs/DEPLOY_NATIVE.md)** — termasuk
   `Cross.toml`, `deploy.sh` (script otomatis build+scp), file systemd, dan config nginx.

## Menjalankan (development, via Docker)

```bash
cp .env.example .env
# edit .env: isi INFLUX_TOKEN, JWT_SECRET, TELEGRAM_BOT_TOKEN, dsb (gunakan random string panjang)

docker compose up -d --build
```

Layanan setelah naik:
- Frontend: http://localhost (via Caddy, proxy ke nginx yang serve build Leptos)
- API REST: http://localhost:8080/api
- InfluxDB UI: http://localhost:8086
- MQTT broker: localhost:1883

## Menjalankan tanpa Docker (dev cepat, native)

Butuh Rust toolchain (`rustup`), `trunk` (`cargo install trunk`), target wasm32:
```bash
rustup target add wasm32-unknown-unknown

# jalankan mosquitto & influxdb lokal atau via docker compose up mosquitto influxdb -d

cargo run -p ingest     # terminal 1
cargo run -p api        # terminal 2
cargo run -p alertengine # terminal 3

cd frontend-leptos && trunk serve --open  # terminal 4, dev server dengan hot-reload
```

## Catatan penting sebelum build pertama kali

1. **`Cargo.lock` belum di-generate** di package ini (dibuat sandbox tanpa akses ke crates.io
   selama pembuatan). Jalankan `cargo build` sekali di mesin dengan akses internet supaya
   `Cargo.lock` ter-generate dan versi dependency terkunci — setelah itu commit `Cargo.lock` ke git.
2. Perhatikan versi dependency di `Cargo.toml` workspace (`tonic 0.11`, `axum 0.7`, `leptos 0.6`,
   dst) — cek compatibility matrix masing-masing kalau nanti upgrade, karena tonic/prost/axum
   sering breaking change antar minor version.
3. Akun dashboard dibuat lewat CLI (password bcrypt, tidak perlu SQL manual):
   `bin/api user add admin owner`. Login memakai sesi cookie HttpOnly; lihat `docs/SECURITY.md`.
4. Kalkulasi kWh billing di `crates/api/src/routes/billing.rs` masih placeholder (return 0) —
   perlu diisi query Flux `first()`/`last()` pada `kwh_cumulative` per periode sebelum dipakai
   produksi untuk tagihan riil.

## Struktur folder

Lihat `docs/STRUCTURE.md` untuk detail lengkap tiap folder & tanggung jawabnya.

## Catatan resource RPi4

Karena semua service jalan di 1 RPi4 8GB, `ingest`, `api`, dan `alertengine` adalah binary Rust
terpisah (masing-masing footprint kecil, khas Rust) — jauh lebih ringan dibanding runtime
lain (Node/JVM/Python) untuk beban yang sama. InfluxDB tetap komponen paling berat memory-wise;
pantau dengan `docker stats` setelah beberapa hari jalan produksi.
