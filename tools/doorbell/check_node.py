"""Periksa node ESP32 asli dari Pi/laptop: HTTP, token, foto, status MQTT.

  python -m tools.doorbell.check_node --ip 192.168.1.50 --token $DOORBELL_CAMERA_TOKEN \
      --mqtt-host 192.168.1.10 --mqtt-user smartkost --mqtt-password ...
Keluar dengan kode 1 bila ada yang gagal. Foto contoh disimpan ke --out (untuk menentukan zone.roi).
"""
from __future__ import annotations

import argparse
import io
import json
import os
import sys
import threading

import paho.mqtt.client as mqtt
import requests
from PIL import Image, ImageStat

fails = 0


def check(name, ok, detail=""):
    global fails
    print(f"[{'PASS' if ok else 'FAIL'}] {name}  {detail}")
    if not ok:
        fails += 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ip", required=True)
    ap.add_argument("--port", type=int, default=80)
    ap.add_argument("--token", default=os.environ.get("DOORBELL_CAMERA_TOKEN", ""))
    ap.add_argument("--door", default="depan")
    ap.add_argument("--mqtt-host", default="127.0.0.1")
    ap.add_argument("--mqtt-port", type=int, default=1883)
    ap.add_argument("--mqtt-user", default=os.environ.get("MQTT_USERNAME", ""))
    ap.add_argument("--mqtt-password", default=os.environ.get("MQTT_PASSWORD", ""))
    ap.add_argument("--out", default="capture.jpg")
    a = ap.parse_args()
    base = f"http://{a.ip}:{a.port}"

    try:
        r = requests.get(base + "/", timeout=4)
        check("HTTP node hidup", r.status_code == 200, r.text.strip()[:40])
    except requests.RequestException as e:
        check("HTTP node hidup", False, str(e)[:80])
        print("Node tidak terjangkau; sisa pemeriksaan dilewati.")
        return 1

    try:
        r = requests.get(base + "/capture", timeout=4)
        check("Token wajib (tanpa token ditolak)", r.status_code == 401, f"HTTP {r.status_code}")
    except requests.RequestException as e:
        check("Token wajib (tanpa token ditolak)", False, str(e)[:80])

    try:
        r = requests.get(base + "/capture", headers={"X-Token": a.token}, timeout=6)
        ok = r.status_code == 200 and r.headers.get("content-type", "").startswith("image/jpeg")
        check("Foto dengan token", ok, f"HTTP {r.status_code}, {len(r.content)} B")
        if ok:
            im = Image.open(io.BytesIO(r.content))
            mean = ImageStat.Stat(im.convert("L")).mean[0]
            check("Gambar valid", True, f"{im.size[0]}x{im.size[1]}, kecerahan rata-rata {mean:.0f}/255")
            good = 15 < mean < 240
            check("Gambar tidak gelap/putih total", good, "" if good else "cek cahaya / tutup lensa")
            open(a.out, "wb").write(r.content)
            print(f"      foto contoh disimpan: {a.out} (pakai untuk menentukan zone.roi)")
        elif r.status_code == 401:
            print("      token salah: samakan DOORBELL_CAMERA_TOKEN dengan DEVICE_TOKEN di secrets.h")
        elif r.status_code == 503:
            print("      kamera node mati: cek pinout di pins.h / log serial")
    except requests.RequestException as e:
        check("Foto dengan token", False, str(e)[:80])

    got, ev = {}, threading.Event()
    c = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id="smartkost-check-node")
    if a.mqtt_user:
        c.username_pw_set(a.mqtt_user, a.mqtt_password)

    def on_connect(cl, u, f, rc, p):
        if rc == 0:
            cl.subscribe(f"kost/door/{a.door}/status")
        else:
            got["err"] = f"ditolak broker: {rc}"
            ev.set()

    def on_message(cl, u, m):
        try:
            got["status"] = json.loads(m.payload)
        except ValueError:
            got["err"] = "status bukan JSON"
        ev.set()

    c.on_connect, c.on_message = on_connect, on_message
    try:
        c.connect(a.mqtt_host, a.mqtt_port, 10)
        c.loop_start()
        ev.wait(6)
        c.loop_stop()
    except OSError as e:
        got["err"] = str(e)
    st = got.get("status")
    if st is None:
        check("Status MQTT dari node", False, got.get("err", "tidak ada pesan status (node belum tersambung ke broker?)"))
    else:
        check("Status MQTT dari node", bool(st.get("online")), json.dumps(st))
        if st.get("online"):
            check("Kamera node OK", bool(st.get("cam", True)),
                  f"{st.get('cam_sensor', '')} {st.get('cam_err', '')}".strip())
            if st.get("stream_url"):
                print(f"   live view LAN: {st['stream_url']}?token=<DEVICE_TOKEN>")
            if st.get("temp") is not None:
                check("Suhu chip wajar", st["temp"] < 75, f"{st['temp']:.1f} C")
            if "psram" in st:
                check("PSRAM tersedia", st["psram"] > 1_000_000, f"{st['psram']} B bebas")
    print("SEMUA LULUS" if not fails else f"{fails} pemeriksaan GAGAL")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
