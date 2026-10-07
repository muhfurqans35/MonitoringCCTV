# Keamanan Smart Kost

Ringkasan lapisan keamanan dan langkah migrasi dari versi lama (JWT di localStorage).

## Apa yang dilindungi, dan bagaimana

| Lapisan | Sebelumnya | Sekarang |
|---|---|---|
| Login | JWT 24 jam di `localStorage` (bisa dicuri skrip), rahasia bawaan `dev-secret-change-me` bila `.env` kosong, tidak bisa dicabut | Sesi di server, cookie `HttpOnly; SameSite=Strict` (+`Secure` lewat HTTPS). Token acak 244 bit, database hanya menyimpan SHA-256-nya. Habis bila menganggur 72 jam / maksimal 14 hari |
| Tebak password | Tidak dibatasi | 5 gagal per (IP, akun) → kunci 15 menit (berlipat, maks 24 jam); 20 gagal per IP → 1 jam; nginx 10 percobaan/menit. Waktu respons sama untuk akun ada/tidak ada |
| Password | Hash dibuat manual (htpasswd) | bcrypt cost 12, minimal 10 karakter + campuran, tidak memuat username; ganti dari dashboard (Akun) atau `bin/api user passwd` |
| Pencabutan | Tidak ada | Keluar, **keluar dari semua perangkat**, ganti password (sesi lain dicabut), hapus/ubah peran akun → sesi mati. Daftar perangkat yang login tampil di menu Akun |
| Hak akses | Semua yang login bisa semua | CCTV, playback NVR, deteksi, alert, tagihan, bel pintu: **khusus `owner`**. Penghuni (`tenant`) hanya data kamar |
| CSRF / situs lain | CORS `Any` | CORS dihapus; SameSite=Strict + cek Origin untuk semua permintaan yang mengubah data |
| WebSocket `/ws` | Tanpa login | Wajib sesi + Origin situs sendiri; diputus otomatis bila sesi dicabut |
| Live CCTV `/live/` (go2rtc) | **Terbuka tanpa login, termasuk API pengubah stream** | nginx `auth_request` (sesi owner), hanya `stream.html`, `video-rtc.js`, `video-stream.js`, `api/ws`. API konfigurasi go2rtc tertutup |
| RTSP go2rtc :8554 | Siapa pun di WiFi kost bisa menonton | Dimatikan (`rtsp: listen: ""`) |
| API :8080 | `0.0.0.0` (bisa dilewati tanpa nginx) | `127.0.0.1`; IP asli dari nginx/Cloudflare, tidak bisa dipalsukan dari LAN |
| Header browser | — | CSP, X-Frame-Options, nosniff, Referrer-Policy, Permissions-Policy, HSTS (saat HTTPS), `Cache-Control: no-store` untuk data API |
| Audit | — | Tabel `auth_events` (login berhasil/gagal/diblokir, logout, ganti password), `bin/api user log` |
| systemd | NoNewPrivileges, ProtectSystem | + ProtectHome, PrivateTmp, ProtectKernel*, RestrictNamespaces, tanpa capability, UMask 0077 |

## Migrasi (lakukan berurutan)

1. **Build & pasang** `api` + frontend baru seperti biasa. Migrasi database `0003_security.sql`
   jalan otomatis saat service start.
2. **Akun**: akun lama di tabel `users` tetap berlaku. Untuk membuat/mengganti:
   ```bash
   cd /home/supriadi/Dev/smartkost
   set -a; . ./.env; set +a
   bin/api user list
   bin/api user add admin owner        # bila belum ada akun
   bin/api user passwd admin           # ganti ke password kuat (sesi lama dicabut)
   ```
   Semua orang harus **login ulang** sekali (token JWT lama tidak dipakai lagi).
3. **nginx**:
   ```bash
   sudo cp deploy/nginx/smartkost-proxy.conf /etc/nginx/smartkost-proxy.conf
   sudo cp deploy/nginx/smartkost.conf /etc/nginx/sites-available/smartkost
   sudo nginx -t && sudo systemctl reload nginx
   ```
   `nginx -t` gagal dengan `unknown directive "auth_request"`? Pasang paket lengkap:
   `sudo apt install nginx-full` (Raspberry Pi OS biasanya sudah termasuk).
4. **systemd** (unit api, vision, go2rtc diperketat):
   ```bash
   sudo cp deploy/systemd/smartkost-{api,vision,go2rtc}.service /etc/systemd/system/
   sudo systemctl daemon-reload && sudo systemctl restart smartkost-go2rtc smartkost-api smartkost-vision
   ```
5. **go2rtc**: tambahkan di `deploy/go2rtc/go2rtc.yaml` (di bawah blok `api:`), lalu restart go2rtc:
   ```yaml
   rtsp:
     listen: ""
   ```
6. **Izin file rahasia**:
   ```bash
   chmod 600 .env config/nvr.toml deploy/go2rtc/go2rtc.yaml
   chmod 700 data
   ```
7. **Lokasi database**: unit systemd hanya mengizinkan tulis ke `data/`. Pastikan `.env` memakai
   `SQLITE_PATH=sqlite:///home/supriadi/Dev/smartkost/data/smartkost.db` (bukan `/data/...` milik
   Docker). Bila login gagal "gagal membuat sesi", ini penyebabnya.
8. Docker/Caddy: isi `TRUSTED_PROXIES=<ip container caddy>` agar pembatas login memakai IP asli.
9. Tambahkan ke `.env` (opsional, ini nilai bawaannya): lihat `.env.example` bagian *Login dashboard*.
   Jangan menaruh komentar di baris yang sama dengan nilai (`EnvironmentFile` systemd tidak membuangnya).

## MQTT (Mosquitto) — wajib sebelum online

Saat ini broker menerima siapa pun di WiFi kost: penghuni bisa mengirim telemetri/alert palsu,
membaca event kamera, atau memberi perintah ke bel pintu.

```bash
sudo cp deploy/mosquitto/smartkost-native.conf /etc/mosquitto/conf.d/smartkost.conf
sudo cp deploy/mosquitto/smartkost.acl.example /etc/mosquitto/smartkost.acl    # sesuaikan
sudo mosquitto_passwd -c /etc/mosquitto/smartkost.passwd smartkost   # layanan Pi
sudo mosquitto_passwd    /etc/mosquitto/smartkost.passwd door-depan  # node bel pintu
sudo mosquitto_passwd    /etc/mosquitto/smartkost.passwd monitor     # MQTT Explorer (baca saja)
sudo chown mosquitto: /etc/mosquitto/smartkost.{passwd,acl} && sudo chmod 640 /etc/mosquitto/smartkost.{passwd,acl}
sudo systemctl restart mosquitto
```
Lalu isi `MQTT_USERNAME=smartkost` dan `MQTT_PASSWORD=...` di `.env`, restart semua service, dan
isi `MQTT_USER`/`MQTT_PASS` di `secrets.h` firmware bel pintu (flash ulang). Node sensor kamar perlu
akun sendiri (`room-101`, dst.) sesuai ACL.

## NVR — akses paling berbahaya saat ini

NVR (192.168.1.200) **tanpa password**: siapa pun di jaringan yang sama bisa membuka web NVR,
aplikasi CMS/XMEye, menonton & **menghapus rekaman**, atau mematikan perekaman.

1. **Beri password admin NVR** (Main Menu → System → Account). Lalu perbarui di tiga tempat:
   `config/nvr.toml` (`password = "..."`), URL RTSP di `deploy/go2rtc/go2rtc.yaml`
   (`password=...`), dan aplikasi CMS di PC. Klien playback/alarm Smart Kost mendukung password.
2. **IP Filter** NVR (Main Menu → System → NetService → IP FILTER): aktifkan *whitelist* berisi
   hanya IP Raspberry Pi dan PC pemilik. Penghuni tidak bisa lagi menyentuh NVR walau satu WiFi.
3. Matikan layanan NVR yang tidak dipakai: **Cloud/P2P (XMEye)** bila tidak dipakai di HP,
   **UPnP** (membuka port di router secara otomatis), DDNS, FTP, Email.
4. Ideal: CCTV + NVR + Pi di jaringan/VLAN terpisah dari WiFi penghuni.

## Sebelum online lewat Cloudflare Tunnel

- Tunnel hanya ke `http://localhost:80` (nginx). Jangan pernah expose 1883, 1984, 8080, 34567.
- Pasang **Cloudflare Access** (Zero Trust, gratis ≤50 pengguna) untuk hostname dashboard dengan
  aturan email pemilik: lapisan login tambahan sebelum halaman Smart Kost tampil.
- Cookie otomatis `Secure` karena Cloudflare mengirim `X-Forwarded-Proto: https`.
- Batas waktu permintaan Cloudflare 100 dtk: klip playback dibatasi 120 dtk (selesai ±60 dtk).

## Uji cepat setelah pasang

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost/api/rooms              # 401
curl -s -o /dev/null -w "%{http_code}\n" http://localhost/live/stream.html        # 401
curl -s -o /dev/null -w "%{http_code}\n" http://localhost/live/api/streams        # 404
curl -s -o /dev/null -w "%{http_code}\n" http://<ip-pi>:8080/api/rooms           # gagal tersambung
curl -sI http://localhost/ | grep -i "content-security\|x-frame"                  # header ada
bin/api user log 10                                                               # jejak login
```
