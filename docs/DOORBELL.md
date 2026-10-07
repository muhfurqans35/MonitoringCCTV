# Modul Doorbell — bel pintu interaktif untuk paket kost

Node ESP32-S3 di pintu (layar sentuh + kamera + PIR) + service Rust `doorbell` di Raspberry Pi.
Kurir memilih tujuan dan kode kamar di layar → kamera memverifikasi ada perubahan di **zona paket** →
penghuni kamar itu dinotifikasi lewat Telegram (foto hanya potongan zona) → catatan ditutup saat diambil.

Modul ini memakai infrastruktur smart-kost yang sudah ada, bukan sistem terpisah:

| Yang dipakai ulang | Cara |
|---|---|
| Broker Mosquitto | topic `kost/door/{door_id}/...` (sejajar `kost/room/...`, `kost/cam/...`) |
| SQLite + migrasi bersama | `0002_doorbell.sql`; kamar diambil dari tabel `rooms` |
| Tabel `devices` | node pintu tampil sebagai `door_node` (online/offline) |
| Tabel `alert_events` + AlertFeed dashboard | paket tak terverifikasi, paket dibantah, node offline |
| Telegram bot & `TELEGRAM_OWNER_CHAT_ID` | pemilik otomatis menjadi pengelola |
| Pola gRPC internal | `doorbell.proto`; `api` memanggil `doorbell` seperti memanggil `ingest` |
| JWT + dashboard Leptos | halaman "Bel & Paket"; aksi ubah-keadaan hanya untuk role `owner` |

```
ESP32-S3 (pintu) --MQTT--> Mosquitto <--MQTT--> doorbell (Rust) --gRPC--> api --> dashboard
   layar+kamera+PIR   \                            |   |  \
                       `--HTTP GET /capture <------'   |   `--> Telegram (long polling + kirim)
                                                       `--> SQLite (doorbell_*, rooms, devices, alert_events)
```

## Status pengujian (baca dulu)

Diverifikasi pada pohon proyek terpadu ini, di atas `Cargo.lock` Anda, dengan Rust 1.91.

| Bagian | Status |
|---|---|
| **Pembaruan live view kamera (v2)** | Proto `Snapshot`, rute `/api/doorbell/camera/snapshot`, status kamera, `/foto`, panel dashboard, dan 3 tes baru **ditinjau manual tetapi BELUM dikompilasi/dijalankan** — registry crates.io tidak terjangkau dari lingkungan pengembangan saat pembaruan ini dibuat. Jalankan `cargo test -p doorbell` dan `cargo check -p api` lebih dulu; laporkan error kompilasinya bila ada |
| Crate `doorbell` | **27 tes lulus** (`cargo test -p doorbell`); satu pengaman kritis diuji dengan mutasi (dirusak sengaja → tes gagal) |
| `common`, `proto`, `ingest`, `api`, `alertengine`, `doorbell` | **lolos `cargo check` dan build** (termasuk rute CCTV/NVR milik Anda di `api`) |
| `vision` | **tidak diperiksa dan tidak diubah.** `Cargo.lock` Anda memuat `kstring` yang butuh Rust 1.96, lebih tinggi dari toolchain yang tersedia untuk pengujian |
| Uji end-to-end | **Dijalankan** dengan Mosquitto asli + biner `api` dan `doorbell` dari pohon ini + node palsu: konfigurasi (kamar dari tabel `rooms`) sampai ke node, paket terverifikasi, node offline, rute doorbell dengan JWT (owner 200, penghuni 403, tanpa token 401), dan rute Anda (`/api/rooms`, `/api/cctv/channels`) tetap 200 |
| Frontend Leptos | **Type-check target native lolos tanpa peringatan**, `Cargo.lock` frontend Anda tidak berubah. Belum dibangun ke WASM (`trunk`) dan belum dilihat di browser |
| Docker | Sintaks compose divalidasi; **image belum pernah dibangun.** Catatan: Dockerfile Anda memakai `rust-1.78`, sedangkan `Cargo.lock` Anda butuh Rust ≥ 1.88 (`image 0.25.10`, `time 0.3.54`); ini sudah begitu sebelum doorbell, dan `Dockerfile.doorbell` mengikuti gaya yang sama. Naikkan tag image di semua `deploy/Dockerfile.*` bila memakai jalur Docker |
| Telegram nyata | **Belum diuji**; logika bot diuji dengan API tiruan, notifikasi di uji E2E dicetak ke log |
| Firmware ESP32 | **Dipindah ke paket terpisah** (`doorbell-firmware-esp-idf.zip`), ditulis ulang native ESP-IDF (C), bukan Arduino lagi. Ditinjau manual (4 bug ditemukan & diperbaiki), **belum dikompilasi** (toolchain ESP-IDF tidak terpasang di lingkungan pengembangan). Protokol MQTT/JSON tidak berubah. Lihat README di dalam zip firmware |
| Pin kamera (`pins.h`, dalam zip firmware) | **Dikonfirmasi** cocok untuk board N16R8 CAM + OV5640 (pola Freenove/ESP32S3_EYE) |
| Ambang zona (`[zone]`) | Nilai awal dari gambar sintetis; **harus disetel** dengan paket dan cahaya nyata |

## Yang ditambahkan ke repo

| Path | Isi |
|---|---|
| `crates/doorbell/` | service Rust: `hub.rs` (logika), `detector.rs` (zona paket), `store.rs`, `mqtt.rs`, `telegram.rs`, `bot.rs`, `grpc.rs`, `tests.rs` |
| `crates/common/migrations/0002_doorbell.sql` | tabel `doorbell_*` |
| `crates/proto/proto/doorbell.proto` | `DoorbellService` (GetStatus, SetMode, CreateInvite, ClosePackage, Calibrate) |
| `crates/api/src/routes/doorbell.rs` | rute REST `/api/doorbell/*` |
| `frontend-leptos/src/doorbell.rs` | halaman "Bel & Paket" |
| `config/doorbell.toml` | tujuan menu, teks layar per mode, ambang zona, retensi |
| `deploy/Dockerfile.doorbell`, `deploy/systemd/smartkost-doorbell.service` | deploy Docker & native |
| `firmware/README.md` | penunjuk: firmware kini ada di paket terpisah `doorbell-firmware-esp-idf.zip` (ESP-IDF native, bukan PlatformIO/Arduino lagi) |
| `tools/doorbell/` | `fake_node`, `check_node`, `collect_dataset` (Python) |

Perubahan kecil pada kode yang sudah ada: `common` (varian `DoorNode`, `PackageUnverified`, `PackageDisputed`,
`AppError::Forbidden`, field konfigurasi doorbell, `build.rs` untuk migrasi), `alertengine/rules.rs`
(memakai `AlertType::as_str()`), `api` (klien gRPC doorbell, middleware menyimpan claims JWT, rute `/api/doorbell/*`),
`frontend-leptos` (tab "Bel & Paket"; `components.rs` memberi label "Bel Pintu" pada alert bersumber `door_*` dan tidak
menghitungnya sebagai kejadian kamera; tab "Deteksi" tidak menampilkan alert `door_*`), `deploy.sh`/`deploy.ps1`
(build + kirim biner dan `config/doorbell.toml`), `docker-compose.yml`, `.env.example`. Kode CCTV/NVR/vision Anda tidak diubah.

## Protokol MQTT (`door_id` = `depan`)

| Topic | Arah | Isi |
|---|---|---|
| `kost/door/depan/event/submit` | node → hub | `{"sid","purpose","room","ts","queued?"}` |
| `kost/door/depan/event/wake` | node → hub | `{}` (PIR) |
| `kost/door/depan/event/zone` | node → hub | `{"state","score"}` (inferensi di ESP32, tahap berikutnya) |
| `kost/door/depan/status` | node → hub | retained + Last Will: `online, ip, rssi, heap, psram, uptime_s, ver, queue, cam, cam_sensor, cam_err, cam_frames_ok, cam_frames_fail, cam_init_attempts, capture_url, stream_url` (firmware v2; dikirim **segera** saat MQTT tersambung, lalu tiap 30 dtk) |
| `kost/door/depan/cmd/camera` | siapa saja → node | `{"action":"reinit"}` init ulang kamera, `{"action":"status"}` publish status sekarang |
| `kost/door/depan/cmd/config` | hub → node | retained: mode, daftar kamar (dari tabel `rooms`), tujuan, teks layar, `ver` |
| `GET http://<ip-node>/capture` (header `X-Token` atau `?token=`) | hub → node | JPEG terbaru; ditarik hanya saat dibutuhkan |
| `GET http://<ip-node>:81/stream?token=` | LAN → node | MJPEG langsung (1 penonton), untuk uji/servis di lokasi |

`sid` unik per kiriman; kiriman ganda/kirim ulang antrean diabaikan. Kiriman `queued: true` (dikirim ulang setelah
node offline) tidak diverifikasi kamera karena paketnya sudah lama ditaruh, dan ditandai "belum terverifikasi".

## Konfigurasi

`config/doorbell.toml` (di-mount read-only ke container; edit lalu restart service `doorbell`):
tujuan menu (maks 4, urutan = urutan tombol), teks layar per mode (`normal`, `sibuk`), ambang zona, retensi.
Validasi otomatis saat start (`doorbell --check` untuk cek tanpa menjalankan):

- teks layar ASCII saja, maks 120 karakter, dan **tidak boleh menyiratkan kost kosong** ("sedang keluar", dst.);
- `roi` tidak boleh > 60% frame; `occupied_frac` < `obstructed_frac`; timeout cukup panjang;
- tujuan `notify = "resident"` wajib `needs_room = true`.

Daftar kamar **tidak** ada di file ini: diambil dari tabel `rooms`. Kode kamar maks 4 karakter ASCII (mis. `K01`);
kamar yang tidak memenuhi syarat itu tidak dikirim ke layar.

Environment (lihat `.env.example`): `DOORBELL_CAMERA_TOKEN` (sama dengan `DEVICE_TOKEN` di firmware),
`DOORBELL_GRPC_PORT`, `TELEGRAM_BOT_TOKEN`, `TELEGRAM_OWNER_CHAT_ID`.

**Hanya satu proses boleh long-polling satu token bot** — itu `doorbell`. `alertengine` hanya mengirim pesan.

## Menjalankan

**Docker Compose:**
```bash
cp .env.example .env     # isi DOORBELL_CAMERA_TOKEN (openssl rand -hex 16), TELEGRAM_*, dst
docker compose up -d --build doorbell mosquitto ingest api
docker compose logs -f doorbell
```
**Native (Raspberry Pi):** `./deploy.sh ...` / `deploy.ps1` sudah membangun & mengirim biner `doorbell`, unit
`smartkost-doorbell.service`, dan `config/doorbell.toml` (tidak menimpa bila sudah ada). Tambahkan
`DOORBELL_CAMERA_TOKEN` ke `.env` di Raspi, lalu: `sudo systemctl enable --now smartkost-doorbell`.

**Tanpa Telegram** (uji lokal): biarkan `TELEGRAM_BOT_TOKEN` kosong → notifikasi dicetak ke log.

## Onboarding penghuni (opt-in)

1. Pastikan kamar ada di tabel `rooms`.
2. Pemilik: dashboard → "Bel & Paket" → isi kode kamar → **Buat kode**, atau kirim `/kode K01` ke bot.
3. Penghuni membuka bot dan mengirim `/start KODE` (sekali pakai, berlaku 24 jam). `/berhenti` menonaktifkan.

Perintah bot pengelola: `/kode <kamar>`, `/paket`, `/tutup <id>`, `/mode [nama]`, `/kalibrasi`, `/status`, `/foto` (foto penuh kamera pintu saat ini).

## Bring-up node asli (urut, jangan dilompati)

**Pin layar & sentuh** (TFT 2,4" ILI9341 + XPT2046; kabel pendek < 15 cm):

| Modul | GPIO | | Modul | GPIO |
|---|---|---|---|---|
| SCK + T_CLK | 47 | | DC | 42 |
| SDI (MOSI) + T_DIN | 21 | | T_CS | 2 |
| T_DO (MISO sentuh) | 14 | | PIR out | 1 |
| CS | 41 | | RESET | ke EN |
| SDO layar | **jangan disambung** | | LED (backlight) | 3,3 V |

**Pin kamera di `main/pins.h` (dalam zip firmware) sudah dikonfirmasi** cocok untuk board N16R8 CAM + OV5640
(pola Freenove/ESP32S3_EYE), termasuk perbaikan `xclk_freq_hz` untuk bug blank-image OV5640 yang dikenal di board ini.

Firmware sekarang ada di paket terpisah `doorbell-firmware-esp-idf.zip` (proyek ESP-IDF native, bukan lagi
Arduino/PlatformIO). Lihat README di dalam zip itu untuk cara build (`idf.py build`) dan status pengujiannya.
Ringkasnya:

```bash
cd doorbell-esp-idf
cp main/secrets.h.example main/secrets.h     # WiFi, IP Pi, MQTT, DOOR_ID, DEVICE_TOKEN
idf.py set-target esp32s3
idf.py build
idf.py -p /dev/ttyUSB0 flash monitor
```

Program uji hardware bertahap (`bringup`, seperti versi Arduino sebelumnya) **belum diport** ke ESP-IDF.
Untuk sementara, uji satu-satu secara manual lewat log serial (`ESP_LOGI`) tiap tahap `app_main()`:

| Yang diuji | Cara |
|---|---|
| Layar | Log "ILI9341 siap" dari `display_init()` tanpa hang |
| Sentuh | Sentuh layar, cek log kalibrasi dari `touch_xpt2046.c` |
| Kamera | Log "Sensor: PID=0x56" (OV5640) setelah `camera_init()`; PID salah/tidak muncul = pinout `pins.h` salah |
| WiFi/broker | Log "Dapat IP" dari `wifi.c`, lalu "MQTT tersambung" dari `app_mqtt.c` |

**B. Firmware utama** — `idf.py -p <port> flash monitor` (lihat blok build di atas). Titik kecil kanan atas layar:
hijau = tersambung ke hub.

**C. Periksa dari Pi/laptop:**
```bash
pip install -r tools/doorbell/requirements.txt
python -m tools.doorbell.check_node --ip <ip-node> --token $DOORBELL_CAMERA_TOKEN \
    --mqtt-host <ip-pi> --mqtt-user $MQTT_USERNAME --mqtt-password $MQTT_PASSWORD
```
Memeriksa HTTP node, token wajib (tanpa token ditolak), foto valid & tidak gelap total, status MQTT retained.
Foto contoh disimpan ke `capture.jpg` untuk menentukan `zone.roi`.

**D. Alur lengkap:** jalankan `doorbell`, `/kalibrasi` saat zona kosong, daftarkan satu akun Telegram ke satu kamar,
lakukan satu pengantaran nyata di layar. Pantau `/status` dan tabel `doorbell_events` (`zone_poll` mencatat skor perubahan).

Node palsu untuk menguji hub tanpa ESP32 (set `[camera] host="127.0.0.1", port=8080` di `doorbell.toml`):
```bash
python -m tools.doorbell.fake_node --room K01 --place-after 5     # terverifikasi
python -m tools.doorbell.fake_node --room K01 --no-place          # belum terverifikasi
```

## Kalibrasi & tuning zona paket

1. Kamera menghadap zona (alas polos berwarna kontras, batas jelas); posisi dikunci permanen.
2. Setel `zone.roi` = `[x, y, lebar, tinggi]` (pecahan 0–1 dari frame; maks 60% frame).
3. Zona **kosong** → `/kalibrasi` (bot) atau tombol di dashboard.
4. Uji dengan paket nyata (kecil, sedang, plastik lembek), siang dan malam; setel `occupied_frac`, `change_frac`, `pix_thr`.
   Nilai bawaan berasal dari gambar sintetis dan **harus** disetel.
5. Verifikasi paket bekerja tanpa kalibrasi (perubahan sebelum–sesudah); kalibrasi dibutuhkan untuk status
   kosong/ada_paket/terhalang dan penutupan otomatis.
6. Dataset: `python -m tools.doorbell.collect_dataset --host <ip-node> --label kosong --count 50 --interval 30 --note siang_cerah`
   (hanya potongan ROI yang disimpan).

## API (JWT; seluruh rute khusus role `owner`, penghuni mendapat 403)

| Endpoint | Fungsi |
|---|---|
| `GET /api/doorbell/status` | status node, mode, sinkron konfigurasi, kalibrasi, jumlah paket |
| `GET /api/doorbell/packages?scope=waiting\|all` | daftar paket (tanpa foto) |
| `POST /api/doorbell/packages/:id/close` | tandai sudah diambil |
| `POST /api/doorbell/mode` `{"mode"}` | ganti mode; layar diperbarui saat node idle |
| `POST /api/doorbell/invites` `{"room_code"}` | kode pendaftaran penghuni |
| `POST /api/doorbell/calibrate` | simpan referensi zona kosong |
| `GET /api/doorbell/camera/snapshot` | JPEG penuh terbaru dari kamera pintu (live view dashboard). Gagal → `503 {"error": "<alasan>"}` |

`/api/doorbell/status` juga memuat `camera_ok`, `camera_sensor`, `camera_error`, `stream_url`.

## Live view kamera pintu (dashboard)

Halaman **Bel & Paket** punya panel **Kamera pintu**: foto diperbarui tiap 1 detik (tombol Jeda/Live,
"Ambil foto sekarang"), plus kartu status **Kamera Online/Offline** beserta pesan error dari node.

```
browser --JWT--> api  --gRPC Snapshot-->  doorbell (hub)  --HTTP /capture + X-Token-->  ESP32-S3
                                           cache 0,8 dtk: banyak penonton = 1 request ke node
```

Dipilih snapshot lewat hub (bukan stream MJPEG langsung) karena: browser tidak perlu satu jaringan dengan ESP32
(dashboard bisa dibuka dari luar lewat Caddy/nginx), token perangkat tidak pernah sampai ke browser, dan
`esp_http_server` di node hanya bisa melayani satu stream sekaligus.

### Kamera tetap "Offline" — cek berurutan

1. **MQTT Explorer → `kost/door/depan/status`**: harus `"online":true` beberapa detik setelah node menyala.
   Masih `{"online":false}` → node belum tersambung ke broker (cek `MQTT_HOST`/WiFi di `secrets.h`, log serial `mqtt`).
2. **Field `cam` di topic yang sama**: `false` → baca `cam_err` (tabel arti error ada di README firmware).
   Publish `{"action":"reinit"}` ke `kost/door/depan/cmd/camera` untuk mencoba ulang tanpa reset.
3. **`cam:true` tapi panel dashboard error** → pesan di panel menyebut penyebabnya, mis.
   `node menolak token (401)` → samakan `DOORBELL_CAMERA_TOKEN` (env service doorbell) dengan `DEVICE_TOKEN` (firmware);
   `tidak bisa terhubung ke http://.../capture` → Pi dan ESP32 beda jaringan/VLAN, atau firewall.
4. Buka `http://<ip-node>/` dari browser di LAN: menampilkan `kamera : OK (OV5640)` atau `GAGAL`.

## Desain privasi

- Layar hanya menampilkan kode kamar; tidak ada nama, foto, atau riwayat.
- Sistem hanya menyimpan kode kamar + `chat_id` Telegram penghuni (opt-in, `/berhenti` kapan saja); tidak menyimpan nama.
- Foto = **potongan ROI** (bagian frame lain dibuang sebelum disimpan/dikirim), dihapus saat diambil atau setelah 7 hari.
  Foto tidak disajikan lewat API; hanya dikirim ke penghuni kamar terkait dan pengelola.
- Tanpa rekaman kontinu: kamera diambil otomatis hanya saat ada kiriman atau ada paket menunggu.
  **Live view** (dashboard, `/foto`) hanya untuk pemilik/pengelola, tidak disimpan ke disk, dan hanya berjalan
  selama halaman dibuka. Ini foto **penuh** (bukan potongan ROI) — sampaikan ke penghuni bahwa pemilik dapat
  melihat area pintu secara langsung.
- **Kompromi:** pesan & foto Telegram melewati server pihak ketiga.
- Layar mencantumkan "Area ini dilengkapi kamera". Untuk penggunaan komersial/tesis, urus persetujuan
  penghuni/subjek sesuai UU PDP No. 27/2022 dan minta konsultasi hukum.

## Perilaku penting

- **Verifikasi berbasis perubahan:** foto sebelum konfirmasi dibandingkan dengan foto sesudahnya, menunggu pemandangan
  stabil (tanpa gerakan). Perubahan terlalu besar = "terhalang" (orang), bukan paket — juga tanpa kalibrasi.
- **Penutupan otomatis** hanya untuk paket terverifikasi berusia > 5 menit, detektor terkalibrasi, dan zona terlihat
  kosong 2× berturut-turut. Selebihnya pakai tombol Telegram/dashboard.
- Kegagalan kamera tidak menghilangkan catatan: paket tetap dicatat sebagai "belum terverifikasi" dan pengelola diberi tahu.
- Antrean offline di node: kiriman saat Pi/WiFi mati dikirim ulang setelah tersambung.

## Pengujian

```bash
cargo test -p doorbell    # 30 tes: konfigurasi, detektor, alur paket, tombol, auto-close, pengingat, bot, integrasi tabel,
                          # status kamera dari MQTT, cache snapshot, perintah /foto
```
Tes memakai SQLite sementara dengan migrasi asli, kamera/notifikasi/MQTT tiruan, dan gambar sintetis.

## Keterbatasan yang diketahui

- Inferensi zona baru di **Pi** (baseline frame-difference). Perbandingan ESP32 vs Pi, model TinyML/CNN, dan
  pelabelan dataset adalah tahap berikutnya; topic `event/zone` sudah diterima hub.
- MQTT node memakai QoS 0 (PubSubClient): bila koneksi putus persis saat submit, pesan bisa hilang tanpa masuk antrean.
- `mqttConnect()` di firmware memblokir UI hingga ~1 detik tiap 5 detik saat broker tak terjangkau.
- Konfigurasi node hanya satu slot (config tak valid ditolak dan yang lama dipertahankan); slot cadangan dan unduhan
  berhash belum ada.
- Dashboard memperbarui halaman paket tiap 10 detik (belum lewat WebSocket).
- Notifikasi memakai `chat_id` yang didaftarkan lewat kode; belum terhubung ke `tenants.telegram_chat_id`.
- Firmware (paket terpisah `doorbell-firmware-esp-idf.zip`, ESP-IDF native) sudah ditinjau manual dan 4 bug
  ditemukan/diperbaiki, tetapi **belum dikompilasi** (toolchain ESP-IDF tidak terpasang di lingkungan pengembangan)
  dan belum diuji di hardware. Program uji bertahap (`bringup`) versi Arduino belum diport.
