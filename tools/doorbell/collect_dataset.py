"""Akuisisi dataset zona paket: simpan HANYA potongan ROI, dengan label kelas.

  python -m tools.doorbell.collect_dataset --config config/doorbell.toml --host 192.168.1.50 \
      --label kosong --count 50 --interval 30 --note siang_cerah
Label: kosong | ada_paket | terhalang. Atur adegan (paket/orang) sebelum menjalankan tiap label.
Variasikan siang/malam/hujan/bayangan. Hormati persetujuan subjek; ROI dipotong agar wajah tidak ikut.
"""
from __future__ import annotations

import argparse
import io
import os
import time
import tomllib
from pathlib import Path

import requests
from PIL import Image


def crop_roi(jpeg: bytes, roi) -> bytes:
    im = Image.open(io.BytesIO(jpeg)).convert("RGB")
    w, h = im.size
    x0, y0 = round(roi[0] * w), round(roi[1] * h)
    x1, y1 = max(round((roi[0] + roi[2]) * w), x0 + 1), max(round((roi[1] + roi[3]) * h), y0 + 1)
    buf = io.BytesIO()
    im.crop((x0, y0, x1, y1)).save(buf, "JPEG", quality=90)
    return buf.getvalue()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default="config/doorbell.toml")
    ap.add_argument("--host", required=True, help="IP node")
    ap.add_argument("--label", required=True, choices=["kosong", "ada_paket", "terhalang"])
    ap.add_argument("--count", type=int, default=20)
    ap.add_argument("--interval", type=float, default=30)
    ap.add_argument("--note", default="", help="kondisi, mis. siang_cerah / malam_lampu / hujan")
    ap.add_argument("--out", default="data/doorbell/dataset")
    a = ap.parse_args()
    cfg = tomllib.loads(Path(a.config).read_text())
    roi = cfg.get("zone", {}).get("roi", [0.15, 0.35, 0.70, 0.50])
    cam = cfg.get("camera", {})
    port = cam.get("port", 80)
    token = os.environ.get("DOORBELL_CAMERA_TOKEN") or cam.get("token", "")
    out = Path(a.out) / a.label
    out.mkdir(parents=True, exist_ok=True)
    saved = 0
    for i in range(a.count):
        try:
            r = requests.get(f"http://{a.host}:{port}/capture", headers={"X-Token": token}, timeout=6)
            ok = r.status_code == 200
        except requests.RequestException:
            ok = False
        if ok:
            name = f"{time.strftime('%Y%m%d_%H%M%S')}_{a.note or 'na'}.jpg"
            (out / name).write_bytes(crop_roi(r.content, roi))
            saved += 1
            print(f"[{i + 1}/{a.count}] {name}")
        else:
            print(f"[{i + 1}/{a.count}] gagal ambil frame")
        if i < a.count - 1:
            time.sleep(a.interval)
    print(f"selesai: {saved} gambar di {out}")


if __name__ == "__main__":
    main()
