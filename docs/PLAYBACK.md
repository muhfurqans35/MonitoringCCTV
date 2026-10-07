# Playback & alarm NVR (langsung dari NVR)

NVR XMEye/Xiongmai/Xihancam ini **tidak** mendukung ONVIF, RTSP playback, maupun DVRIP versi JSON.
Yang dipakai aplikasi CMS adalah **DVRIP biner** di port 34567 ("TCP Port" di menu Network NVR).
Protokol itu direkonstruksi dari rekaman Wireshark CMS dan diterapkan di crate `crates/dvrip`
(hanya library standar Rust, tanpa dependensi).

```
                 ┌─ api  : /api/nvr/recordings  daftar rekaman + gerakan NVR ([M]) per hari
NVR :34567 ──────┤         /api/nvr/clip        klip MP4 (H.264 asli, remux ffmpeg tanpa encode)
 (DVRIP biner)   └─ vision: alarm motion seketika (pesan 1504) -> analisis YOLO langsung
                            + MQTT kost/cam/<id>/nvr
```

## Prasyarat

1. `ffmpeg` terpasang di Pi (sudah dibutuhkan go2rtc): `sudo apt install ffmpeg`
2. **Jam NVR harus sama dengan jam Pi.** Saat diuji, jam NVR terlambat ±15 menit. Semua jam di
   halaman Playback adalah jam NVR, sedangkan deteksi vision memakai jam Pi, jadi selisih jam membuat
   tanda orang/kendaraan bergeser dari gerakan NVR.
   NVR: **Main Menu → System → General → Date/Time**: aktifkan NTP (`pool.ntp.org`, GMT+08:00).
   Service vision memberi peringatan di log bila selisihnya lebih dari 1 menit.
3. Opsional di `config/nvr.toml` (nilai di bawah = default, tidak perlu ditulis):
   ```toml
   dvrip_port = 34567

   [vision]
   nvr_alarm = true            # dengarkan alarm motion NVR
   alarm_interval_ms = 700     # jeda frame selama boost setelah alarm
   alarm_boost_seconds = 15
   ```

## Halaman Playback (dashboard)

- **Garis waktu 24 jam**: abu-hijau = ada rekaman, kuning = gerakan dicatat NVR, garis gelap
  bertitik = orang/kendaraan menurut vision. Ketuk untuk memilih jam.
- **Garis waktu 1 jam**: ketuk titik mana pun untuk memutar dari situ.
- **Daftar gerakan NVR**: ketuk untuk memutar. Saring "Ada orang/kendaraan" untuk membuang gerakan
  daun/bayangan (CAM5 mencatat ratusan gerakan sehari).
- Durasi klip 15–120 dtk. NVR mengirim ±2x waktu nyata: klip 30 dtk siap dalam ±15 dtk.
  Klip disimpan sementara di `data/nvr-clips` (dihapus otomatis setelah 24 jam), jadi memutar
  ulang klip yang sama langsung tampil. Tombol **Unduh** menyimpan MP4.

## Alarm motion NVR di service vision

Saat NVR melapor gerakan di sebuah kamera, vision langsung mengambil frame kamera itu dan
menjalankan model (tanpa menunggu interval 2 dtk + 2 frame konfirmasi), lalu selama 15 dtk mengambil
frame tiap 700 ms. Hasilnya: orang tertangkap lebih awal, wajah lebih sering masuk foto.
Alarm NVR **tidak** dikirim sebagai alert karena NVR mudah terpicu daun/bayangan; alert tetap hanya
dari orang/kendaraan yang dikonfirmasi model.

MQTT Explorer → `kost/cam/<id>/nvr`:
```json
{"camera_id":"gate_right","channel":5,"event":"motion","status":"start",
 "nvr_time":"2026-10-06T20:15:37","skew_seconds":-878,"source":"nvr"}
```
`skew_seconds` = jam NVR dikurangi jam Pi (harus mendekati 0). Status retained `kost/cam/<id>/vision`
juga menampilkan `nvr_alarms` (jumlah alarm sejak service mulai).

## Alat uji (PC/Pi, tanpa library tambahan)

```bash
python3 tools/nvr/nvr_playback.py list --channel 5 --motion          # daftar gerakan hari ini
python3 tools/nvr/nvr_playback.py clip --channel 5 --start "2026-10-06 07:56:23" --seconds 20 -o klip.mp4
python3 tools/nvr/nvr_playback.py alarm                              # alarm seketika
```

## Ringkasan protokol (untuk pengembang)

| Pesan | Arah | Isi |
|---|---|---|
| 1000/1001 | login | user[32] + sofia_hash[32] + u32 1 + u32 2 → ret@8 (100 OK), sesi@16 |
| 1440/1441 | cari file | ch, "h264", 0xFFFF, waktu awal/akhir → maks 64 rekaman × 184 byte @0x18 |
| 1424/1425 | klaim playback (koneksi data baru) | lalu 1422 = isi file XM |
| 1420 | kontrol (koneksi login) | 0,0 mulai · 0,4 seek · 4,0e jeda · 4,0f lanjut · 0,1 stop |
| 1500/1504 | alarm | ch@0x10 (0-based), event@0x14 (4 = motion), status@0x18 (0 mulai/1 selesai), jam@0x1C |
| 1006 | keepalive | tiap 15 dtk |

Waktu = 8 × u32 (tahun, bulan, tanggal, hari, jam, menit, detik, 0). File XM: I-frame `00 00 01 FD`
(16 byte header: fps, lebar/8, tinggi/8, jam terkemas, panjang), P-frame `00 00 01 FC` (8 byte).
