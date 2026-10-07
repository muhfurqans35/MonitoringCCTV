# Deploy Native ke Raspberry Pi 4B (Tanpa Docker di Raspi)

Panduan ini untuk alur: **compile di PC/laptop** (cross-compile) → **binary jadi dikirim ke Raspi**
→ **Raspi cuma menjalankan**, tidak pernah compile apa pun. Docker cuma dipakai di PC/laptop
(sebagai toolchain `cross`), sama sekali tidak perlu ada di Raspi.

> **Catatan path:** contoh di bawah pakai `pi@<ip-raspi>` dan `/home/supriadi/Dev/smartkost` sebagai
> placeholder generik. Sesuaikan dengan user & path kamu sendiri, misal:
> `supriadi@100.96.160.34` dan `/home/supriadi/Dev/smartkost`.

## Windows: pilih salah satu

- **Native Windows (tanpa WSL)** — pakai `deploy.ps1` (PowerShell). Docker Desktop dan OpenSSH
  Client bawaan Windows sudah cukup, tidak perlu install distro Linux apa pun.
```powershell
  # sekali saja, kalau script diblokir:
  Set-ExecutionPolicy RemoteSigned -Scope CurrentUser

  .\deploy.ps1 -Remote pi@<ip-raspi> -RemotePath /home/supriadi/Dev/smartkost
```
- **WSL (Ubuntu)** — pakai `deploy.sh` (Bash), dijalankan dari dalam terminal Ubuntu WSL, bukan
  dari PowerShell langsung. Cocok kalau kamu lebih nyaman kerja di lingkungan Linux penuh.

Linux/macOS otomatis pakai `deploy.sh` (Bash), tidak perlu langkah tambahan.

## Bagian A — Setup sekali di PC/laptop (mesin development)

```bash
# 1. Docker (dipakai `cross` untuk toolchain ARM, bukan runtime)
#    Install seperti biasa: https://docs.docker.com/get-docker/

# 2. Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 3. cross (cross-compilation tool)
cargo install cross --git https://github.com/cross-rs/cross

# 4. target wasm untuk frontend
rustup target add wasm32-unknown-unknown

# 5. trunk (build tool frontend Leptos)
cargo install trunk
# atau, lebih cepat, download binary release:
# https://github.com/trunk-rs/trunk/releases
```

## Bagian B — Setup sekali di Raspberry Pi 4B

### 1. Install dependency native (Mosquitto, InfluxDB, nginx) — via apt, bukan Docker

```bash
sudo apt update
sudo apt install -y mosquitto mosquitto-clients nginx

# InfluxDB 2.x — ikuti instruksi resmi untuk arm64:
# https://docs.influxdata.com/influxdb/v2/install/?t=Linux+ARM
# ringkasnya:
curl -O https://dl.influxdata.com/influxdb/releases/influxdb2-2.7.6-arm64.deb
sudo dpkg -i influxdb2-2.7.6-arm64.deb
sudo systemctl enable --now influxdb
```

Setup awal InfluxDB (buat org, bucket, token) via CLI atau UI (http://<ip-raspi>:8086):
```bash
influx setup \
  --username admin \
  --password "password_kuat_kamu" \
  --org smartkost \
  --bucket telemetry \
  --force
# Catat token yang muncul, ini yang dipakai di .env (INFLUX_TOKEN)
```

Mosquitto native jalan otomatis setelah install (`systemctl status mosquitto`). Config custom
(kalau perlu auth dsb) ada di `/etc/mosquitto/mosquitto.conf` — bisa dicontek dari
`deploy/mosquitto/mosquitto.conf` di project ini.

### 2. Siapkan folder project di Raspi

```bash
mkdir -p /home/supriadi/Dev/smartkost
```
(folder `bin/`, `frontend/`, `data/`, `config/`, `models/` akan otomatis dibuat oleh
`deploy.ps1`/`deploy.sh` saat pertama kali jalan)

### 3. Config nginx

```bash
# Di PC, copy deploy/nginx/smartkost.conf ke Raspi, atau langsung buat manual di Raspi:
sudo nano /etc/nginx/sites-available/smartkost
# paste isi dari deploy/nginx/smartkost.conf — SESUAIKAN path `root` di dalamnya
# dengan folder frontend kamu, misal /home/supriadi/Dev/smartkost/frontend

sudo ln -s /etc/nginx/sites-available/smartkost /etc/nginx/sites-enabled/
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t && sudo systemctl reload nginx
```

## Bagian C — Build & Deploy (dijalankan tiap ada update kode)

Dari PC/laptop, di root folder project:

```bash
./deploy.sh pi@<ip-raspi> /home/supriadi/Dev/smartkost

```
atau di Windows (PowerShell):
```powershell

.\deploy.ps1 -Remote pi@<ip-raspi> -RemotePath /home/supriadi/Dev/smartkost
Set-ExecutionPolicy -ExecutionPolicy Bypass -Scope Process
.\deploy.ps1 -Remote supriadi@100.96.160.34 -RemotePath /home/supriadi/Dev/smartkost
```



Script ini otomatis:
1. Cross-compile `ingest`, `api`, `alertengine`, `vision` untuk `aarch64-unknown-linux-gnu`
2. Build frontend (`trunk build --release`)
3. `scp` semua hasilnya ke Raspi (`bin/`, `frontend/`)
4. Kalau `.env` belum ada di Raspi, kirim `.env.example` sebagai starter (edit manual setelahnya)
5. Kalau `config/cameras.toml` belum ada di Raspi, kirim template-nya (edit manual, isi
   kredensial kamera — file ini **tidak pernah ditimpa otomatis** setelah ada, supaya password
   kamera yang sudah kamu isi aman)

## Bagian D — Install systemd service (sekali saja)

```bash
scp deploy/systemd/*.service pi@<ip-raspi>:/tmp/
ssh pi@<ip-raspi>
sudo mv /tmp/smartkost-*.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now smartkost-ingest
sudo systemctl enable --now smartkost-api
sudo systemctl enable --now smartkost-alertengine
```

**Penting:** file `.service` di `deploy/systemd/` pakai placeholder `User=pi` dan path
`/home/supriadi/Dev/smartkost`. Edit dulu tiap file sebelum di-copy kalau user/path Raspi kamu beda:
```bash
sed -i \
  -e 's|User=pi|User=supriadi|g' \
  -e 's|/home/supriadi/Dev/smartkost|/home/supriadi/Dev/smartkost|g' \
  deploy/systemd/*.service
```

Cek status & log:
```bash
sudo systemctl status smartkost-ingest
sudo journalctl -u smartkost-api -f    # log realtime
```

## Bagian E — Edit `.env` di Raspi

```bash
nano /home/supriadi/Dev/smartkost/.env
```
Isi minimal: `INFLUX_TOKEN` (dari setup InfluxDB di atas), `JWT_SECRET` (`openssl rand -hex 32`),
`TELEGRAM_BOT_TOKEN`, `TELEGRAM_OWNER_CHAT_ID`. `MQTT_BROKER_HOST` ganti jadi `localhost` (karena
Mosquitto native, bukan container dengan nama `mosquitto`). `SQLITE_PATH` arahkan ke folder data:
Setelah edit `.env`, restart service supaya baca env baru:
```bash
sudo systemctl restart smartkost-ingest smartkost-api smartkost-alertengine
```

## Bagian F — Setup Vision Service (deteksi CCTV, opsional tapi direkomendasikan)

> **Diperbarui:** `vision` tidak lagi memakai ONVIF/`cameras.toml`. Kamera diambil dari `config/nvr.toml`
> dan frame dari go2rtc, dengan deteksi gerak sendiri + model ONNX opsional. Lihat **docs/VISION.md**;
> uraian ONVIF di bawah hanya untuk referensi lama.

`vision` service subscribe event motion bawaan CCTV kamu (ONVIF), lalu jalankan deteksi
person/vehicle pakai model YOLO ringan cuma saat ada trigger motion (bukan proses stream
terus-menerus) — ini yang bikin banyak channel CCTV feasible diproses di 1 RPi4.

### 1. Siapkan model (lihat `models/README.md` untuk detail lengkap)

Ringkasnya, di laptop/PC:
```bash
pip install ultralytics
python3 -c "from ultralytics import YOLO; YOLO('yolov8n.pt').export(format='onnx', imgsz=640, opset=12)"
```
Taruh hasil `yolov8n.onnx` di folder `models/` project **sebelum** deploy supaya ikut
ke-transfer otomatis, atau `scp` manual kalau deploy sudah kadung jalan tanpa model itu ada:
```bash
scp yolov8n.onnx supriadi@100.96.160.34:/home/supriadi/Dev/smartkost/models
```

### 2. Edit `config/cameras.toml`

File ini **tidak ditimpa otomatis** oleh `deploy.ps1`/`deploy.sh` kalau sudah ada di Raspi
(supaya kredensial kamera yang kamu isi di sana aman). Edit langsung di Raspi:
```bash
nano /home/supriadi/Dev/smartkost/config/cameras.toml
```
Isi IP kamera, username/password, dan `snapshot_path` sesuai merk CCTV kamu (default di
template mengasumsikan endpoint ISAPI ala Hikvision — cek dokumentasi kamera kamu kalau beda).

### 3. Install systemd service tambahan

```bash
scp deploy/systemd/smartkost-vision.service supriadi@100.96.160.34:/tmp/
ssh supriadi@100.96.160.34
sudo mv /tmp/smartkost-vision.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now smartkost-vision
```

(ingat: edit dulu `User=`/path di file ini juga kalau belum, sama seperti Bagian D)

### 4. Pantau & tuning

```bash
journalctl -u smartkost-vision -f
```

Kalau CPU RPi4 terasa berat (cek `htop`) setelah beberapa kamera aktif bersamaan, turunkan
`max_concurrent_inference` di `config/cameras.toml` (default 2) dan/atau `model_input_size`
(default 640, coba turunkan ke 416), lalu restart service.

## Update berikutnya (workflow harian)

Cukup ulangi Bagian C (`./deploy.sh ...` atau `.\deploy.ps1 ...`) tiap kali ada perubahan kode, lalu:
```bash
ssh pi@<ip-raspi> "sudo systemctl restart smartkost-ingest smartkost-api smartkost-alertengine smartkost-vision smartkost-doorbell"
```

Tidak ada compile di Raspi, tidak ada Docker, tidak ada image yang menumpuk — storage SD card/SSD
kamu cuma dipakai untuk OS + beberapa binary kecil + data InfluxDB/SQLite yang tumbuh natural
sesuai lama pemakaian.

## Bagian G — Doorbell (bel pintu interaktif, opsional)

Detail lengkap: [`DOORBELL.md`](DOORBELL.md). Ringkasnya untuk deploy native:

1. `./deploy.sh ...` (atau `deploy.ps1`) sudah membangun & mengirim biner `doorbell` dan menyalin
   `config/doorbell.toml` bila belum ada di Raspi (tidak menimpa hasil tuning).
2. Tambahkan ke `.env` di Raspi: `DOORBELL_CAMERA_TOKEN` (acak; sama dengan `DEVICE_TOKEN` di firmware),
   `TELEGRAM_BOT_TOKEN`, `TELEGRAM_OWNER_CHAT_ID`. `DOORBELL_GRPC_PORT` opsional (default 9091).
3. `sudo systemctl enable --now smartkost-doorbell` lalu `journalctl -u smartkost-doorbell -f`.
4. Pastikan tabel `rooms` berisi kamar (kode maks 4 karakter ASCII); daftar kamar di layar node diambil dari sana.
5. Hanya `doorbell` yang boleh long-polling token bot Telegram. Jangan jalankan bot lain dengan token yang sama.
6. Firmware node: paket terpisah `doorbell-firmware-esp-idf.zip` (ESP-IDF native) — ikuti README di dalamnya
   dan bagian bring-up di `DOORBELL.md`.
