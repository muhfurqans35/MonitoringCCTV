#!/usr/bin/env bash
#
# deploy.sh — build semua komponen (Rust binary via cross-compile + frontend WASM)
# di PC/laptop, lalu kirim hasilnya ke Raspberry Pi 4B via scp.
#
# Raspi TIDAK melakukan compile apa pun — cuma menerima file jadi dan menjalankannya.
#
# Prasyarat di PC/laptop:
#   - Docker terinstal (dipakai `cross` untuk toolchain ARM, bukan untuk runtime)
#   - `cargo install cross --git https://github.com/cross-rs/cross`
#   - `cargo install trunk` (atau download binary release trunk)
#   - `rustup target add wasm32-unknown-unknown`
#
# Pemakaian:
#   ./deploy.sh <user>@<ip-raspi> [remote-path]
#
# Contoh:
#   ./deploy.sh pi@192.168.1.50 /home/supriadi/Dev/smartkost

set -euo pipefail

TARGET="aarch64-unknown-linux-gnu"
REMOTE="${1:?Usage: ./deploy.sh <user>@<ip-raspi> [remote-path]}"
REMOTE_PATH="${2:-/home/supriadi/Dev/smartkost}"

echo "=== 1/5: Cross-compile binary Rust (ingest, api, alertengine, vision, doorbell) untuk $TARGET ==="
cross build --release --target "$TARGET" -p ingest
cross build --release --target "$TARGET" -p api
cross build --release --target "$TARGET" -p alertengine
cross build --release --target "$TARGET" -p vision
cross build --release --target "$TARGET" -p doorbell

echo "=== 2/5: Build frontend (Leptos -> WASM) ==="
( cd frontend-leptos && trunk build --release )

echo "=== 3/5: Siapkan folder remote di Raspi ==="
ssh "$REMOTE" "mkdir -p $REMOTE_PATH/bin $REMOTE_PATH/frontend $REMOTE_PATH/data $REMOTE_PATH/deploy/go2rtc $REMOTE_PATH/config $REMOTE_PATH/models"

echo "=== 4/5: Transfer file ke Raspi ($REMOTE:$REMOTE_PATH) ==="
scp "target/$TARGET/release/ingest" "$REMOTE:$REMOTE_PATH/bin/"
scp "target/$TARGET/release/api" "$REMOTE:$REMOTE_PATH/bin/"
scp "target/$TARGET/release/alertengine" "$REMOTE:$REMOTE_PATH/bin/"
scp "target/$TARGET/release/vision" "$REMOTE:$REMOTE_PATH/bin/"
scp "target/$TARGET/release/doorbell" "$REMOTE:$REMOTE_PATH/bin/"
scp -r frontend-leptos/dist/. "$REMOTE:$REMOTE_PATH/frontend/"
scp deploy/go2rtc/go2rtc.yaml "$REMOTE:$REMOTE_PATH/deploy/go2rtc/"

curl -L https://github.com/AlexxIT/go2rtc/releases/latest/download/go2rtc_linux_arm64 -o /tmp/go2rtc_linux_arm64
scp /tmp/go2rtc_linux_arm64 "$REMOTE:$REMOTE_PATH/bin/go2rtc"

# .env tidak otomatis di-overwrite kalau sudah ada di Raspi (supaya secret tidak ketimpa).
if ssh "$REMOTE" "[ -f $REMOTE_PATH/.env ]"; then
  echo "File .env sudah ada di Raspi, tidak ditimpa."
else
  echo "Belum ada .env di Raspi, mengirim .env.example sebagai referensi awal..."
  scp .env.example "$REMOTE:$REMOTE_PATH/.env"
  echo "PENTING: edit $REMOTE_PATH/.env di Raspi sebelum menjalankan service!"
fi

if ssh "$REMOTE" "[ -f $REMOTE_PATH/config/cameras.toml ]"; then
  echo "config/cameras.toml sudah ada di Raspi, tidak ditimpa."
else
  scp config/cameras.toml "$REMOTE:$REMOTE_PATH/config/"
fi

if ssh "$REMOTE" "[ -f $REMOTE_PATH/config/nvr.toml ]"; then
  echo "config/nvr.toml sudah ada di Raspi, tidak ditimpa."
else
  scp config/nvr.toml "$REMOTE:$REMOTE_PATH/config/"
fi

# Config doorbell (bel pintu): tidak ditimpa kalau sudah ada (teks layar & ambang zona hasil tuning).
if ssh "$REMOTE" "[ -f $REMOTE_PATH/config/doorbell.toml ]"; then
  echo "config/doorbell.toml sudah ada di Raspi, tidak ditimpa."
else
  scp config/doorbell.toml "$REMOTE:$REMOTE_PATH/config/"
  echo "PENTING: cek $REMOTE_PATH/config/doorbell.toml (door_id, teks, zone.roi) sebelum start smartkost-doorbell!"
fi

ssh "$REMOTE" "chmod +x $REMOTE_PATH/bin/*"

echo "=== 5/5: Copy service file ke remote ==="
for svc in deploy/systemd/*.service; do
  scp "$svc" "$REMOTE:/tmp/"
  ssh "$REMOTE" "sudo mv /tmp/$(basename "$svc") /etc/systemd/system/"
done
ssh "$REMOTE" "sudo systemctl daemon-reload"

echo ""
echo "=== Selesai ==="
echo "Binary, config, frontend, dan go2rtc sudah ada di $REMOTE:$REMOTE_PATH"
echo "Langkah selanjutnya (sekali saja, di Raspi):"
echo "  1. Edit $REMOTE_PATH/.env (isi token, secret, dsb)"
echo "  2. Edit $REMOTE_PATH/config/cameras.toml jika perlu"
echo "  3. sudo systemctl enable --now smartkost-go2rtc"
echo "  (doorbell) tambahkan DOORBELL_CAMERA_TOKEN ke .env dan cek config/doorbell.toml — lihat docs/DOORBELL.md"
echo "  4. sudo systemctl enable --now smartkost-ingest smartkost-api smartkost-alertengine smartkost-vision smartkost-doorbell"
