# Vision — analisis CCTV (deteksi gerak + model ONNX ringan)

Service `vision` memantau kamera CCTV dari **stream yang sama dengan live view** (NVR → go2rtc),
bukan lewat ONVIF/snapshot.cgi (tidak cocok untuk NVR XMEye/Xihancam) dan tidak bergantung pada
protokol alarm NVR.

```
config/nvr.toml [[channels]] ──> vision ──> go2rtc /api/frame.jpeg?src=<id>   (frame dari RTSP NVR)
                                   │
                                   ├─ deteksi gerak: selisih frame grayscale 160 px, kompensasi cahaya, ROI
                                   ├─ bergerak ──> model ONNX (YOLOv8n/YOLO11n, input 320) — hanya saat perlu
                                   ├─ simpan  data/vision/<id>/<tanggal>/<jam>_<label>.jpg + .json (bbox)
                                   └─ MQTT kost/cam/<id>/event ──> ingest ──> alert dashboard
```

`config/cameras.toml` **tidak dipakai lagi** (boleh dihapus). Daftar kamera = `[[channels]]` di
`config/nvr.toml`; pengaturan analisis = bagian `[vision]` di file yang sama (semua opsional).

## Prasyarat

1. go2rtc berjalan dan nama stream di `deploy/go2rtc/go2rtc.yaml` = `id` channel di `nvr.toml`.
2. `ffmpeg` terpasang (go2rtc memakainya untuk mengubah keyframe H.264 → JPEG):
   ```bash
   sudo apt install ffmpeg
   curl -s -o t.jpg -w "HTTP %{http_code}, %{size_download} byte\n" \
        "http://127.0.0.1:1984/api/frame.jpeg?src=gate_right"      # harus HTTP 200, puluhan KB
   ```

## Model (WAJIB untuk mode "hanya orang/kendaraan")

Default `only_objects = true`: gerakan hanya disimpan & dikirim ke dashboard bila model menemukan
orang atau kendaraan. **Tanpa file model tidak ada event sama sekali** (log menampilkan peringatan
`only_objects = true tetapi model tidak tersedia`). Set `only_objects = false` untuk kembali ke mode
lama yang melaporkan semua gerakan sebagai `motion`.

Ekspor model ringan di PC (Python), lalu salin ke Pi `models/`:
```bash
pip install ultralytics
yolo export model=yolov8n.pt format=onnx imgsz=320 opset=12 simplify=True   # -> yolov8n.onnx
# atau YOLO11n (lebih akurat, ukuran mirip):
yolo export model=yolo11n.pt format=onnx imgsz=320 opset=12 simplify=True
```
`imgsz` **harus sama** dengan `[vision.model].input_size` (default 320; 320 ≈ 4× lebih cepat dari 640).
Parser output mendukung format YOLOv8/YOLO11 (1 × (4+kelas) × N). Model dengan format output lain
(NanoDet, MobileNet-SSD, YOLOv5) perlu penyesuaian parser di `crates/vision/src/inference.rs`.

## Pengaturan penting (`config/nvr.toml` → `[vision]`)

| Kunci | Default | Keterangan |
|---|---|---|
| `interval_ms` | 2000 | Jeda ambil frame per kamera |
| `stream_suffix` | `""` | `"_sub"` = pakai stream `<id>_sub` (substream, jauh lebih ringan) — tambahkan di go2rtc.yaml |
| `cooldown_seconds` | 30 | Satu gerakan panjang = satu event |
| `keep_days` | 14 | Retensi snapshot |
| `only_objects` | true | Hanya orang/kendaraan (butuh model) |
| `save_empty_motion` | false | (only_objects = false) simpan gerakan tanpa objek |
| `[vision.motion].min_fraction` | 0.015 | Sensitivitas: naikkan bila terlalu sering terpicu (daun, hujan) |
| `[vision.motion.roi].<id>` | seluruh frame | `[x, y, lebar, tinggi]` pecahan 0-1, mis. abaikan jalan raya |
| `[vision.model].classes` | person, bicycle, car, motorcycle, bus, truck | Nama kelas COCO |

Substream (disarankan untuk Pi): di `go2rtc.yaml` tambahkan stream kedua per kamera dengan
`stream=1` ↔ `stream=0` sesuai NVR Anda (XMEye: 0 = utama, 1 = sub), mis.
```yaml
  gate_right_sub:
    - "rtsp://192.168.1.200:554/user=admin&password=&channel=5&stream=1.sdp?real_stream"
```
lalu `stream_suffix = "_sub"`.

## Menjalankan

```bash
sudo cp deploy/systemd/smartkost-vision.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl restart smartkost-vision
journalctl -u smartkost-vision -f
```
Log yang diharapkan:
```
vision mulai: 4 kamera dari .../nvr.toml (gate_left, gate_right, main_gate, kost), frame tiap 2000 ms ...
kamera gate_right: frame diterima (83779 byte)
kamera gate_right: GERAK (skor 0.042) -> person [person 0.81] -> gate_right/2026-10-06/132701_412_person.jpg
```

## Yang terlihat

| Tempat | Isi |
|---|---|
| MQTT `kost/cam/<id>/event` | `event_type` (person/vehicle/motion), `confidence`, `snapshot_path`, `detections` (kelas + bbox), `motion_score` |
| MQTT `kost/cam/<id>/vision` (retained) | Status pemantauan tiap 60 dtk: online, motion_score terakhir, jumlah event |
| Dashboard → Ringkasan & Deteksi kamera | Galeri foto dengan kotak deteksi + alert "Orang terdeteksi di kamera gate_right (87%)" |
| `data/vision/<id>/<tanggal>/` | JPEG penuh + JSON (deteksi, bbox, skor gerak, model, ukuran gambar) |

JSON per snapshot (contoh):
```json
{ "camera_id": "gate_right", "nvr_channel": 5, "timestamp": "2026-10-06T05:27:01Z", "label": "person",
  "motion_score": 0.042, "model": "models/yolov8n.onnx", "model_input_size": 320,
  "image_width": 704, "image_height": 576,
  "detections": [ { "class": "person", "label": "person", "confidence": 0.81, "bbox": [312, 140, 380, 330] } ] }
```
Snapshot + bbox ini bisa langsung dipakai untuk analisis ulang dengan model lain atau dilabeli ulang
menjadi dataset (format bbox piksel x1,y1,x2,y2).

## Alarm motion langsung dari NVR

Default `nvr_alarm = true`: vision berlangganan alarm motion NVR (DVRIP port 34567). Saat NVR
melapor gerakan, kamera itu langsung dianalisis model dan frame diambil lebih rapat selama
`alarm_boost_seconds`, sehingga orang tertangkap lebih awal. Detail & MQTT `kost/cam/<id>/nvr`:
lihat [PLAYBACK.md](PLAYBACK.md). Log yang diharapkan saat start:
```
alarm NVR: berlangganan ke 192.168.1.200:34567
```
