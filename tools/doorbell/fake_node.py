"""Node pintu palsu untuk menguji service doorbell tanpa ESP32.

Dari root repo:
  python -m tools.doorbell.fake_node --room K01 --place-after 5      # paket ditaruh -> terverifikasi
  python -m tools.doorbell.fake_node --room K01 --no-place           # tak ada perubahan -> belum terverifikasi
Service doorbell harus bisa menjangkau node palsu:  [camera] host = "127.0.0.1", port = 8080 di doorbell.toml
(atau biarkan host kosong dan jalankan node palsu di mesin yang sama; IP dilaporkan lewat status MQTT).
"""
from __future__ import annotations

import argparse
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import paho.mqtt.client as mqtt

from .synth import make_jpeg

STATE = {"scene": "empty", "token": ""}


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/":
            body = b"smartbell node ok"
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path != "/capture":
            self.send_error(404)
            return
        if STATE["token"] and self.headers.get("X-Token") != STATE["token"]:
            self.send_error(401)
            return
        body = make_jpeg(STATE["scene"])
        self.send_response(200)
        self.send_header("Content-Type", "image/jpeg")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mqtt-host", default="127.0.0.1")
    ap.add_argument("--mqtt-port", type=int, default=1883)
    ap.add_argument("--user", default="")
    ap.add_argument("--password", default="")
    ap.add_argument("--door", default="depan")
    ap.add_argument("--http-port", type=int, default=8080)
    ap.add_argument("--purpose", default="paket")
    ap.add_argument("--room", default="K01", help="kode kamar, harus ada di tabel rooms")
    ap.add_argument("--place-after", type=float, default=10, help="detik sampai paket 'ditaruh'")
    ap.add_argument("--no-place", action="store_true")
    ap.add_argument("--hold", type=float, default=30, help="detik node tetap hidup setelah paket ditaruh")
    ap.add_argument("--token", default="", help="wajibkan header X-Token (meniru firmware)")
    a = ap.parse_args()

    STATE["token"] = a.token
    srv = ThreadingHTTPServer(("127.0.0.1", a.http_port), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()

    base = f"kost/door/{a.door}"
    c = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id=f"fake-node-{a.door}")
    if a.user:
        c.username_pw_set(a.user, a.password)
    c.on_message = lambda cl, u, m: print("[node] config diterima:", m.payload.decode()[:140], "...")
    c.will_set(f"{base}/status", json.dumps({"online": False}), retain=True)
    c.connect(a.mqtt_host, a.mqtt_port)
    c.subscribe(f"{base}/cmd/config", 1)
    c.loop_start()
    c.publish(f"{base}/status", json.dumps({"online": True, "ip": "127.0.0.1", "ver": "fake", "cam": True, "cam_sensor": "FAKE", "cam_err": ""}), retain=True)
    time.sleep(1.5)
    sub = {"sid": f"fake-{int(time.time())}", "purpose": a.purpose, "ts": 0}
    if a.purpose in ("paket", "makanan"):
        sub["room"] = a.room
    c.publish(f"{base}/event/submit", json.dumps(sub), qos=1)
    print("[node] submit terkirim:", sub)
    if not a.no_place:
        time.sleep(a.place_after)
        STATE["scene"] = "package"
        print("[node] paket ditaruh di zona")
    time.sleep(a.hold)
    c.publish(f"{base}/status", json.dumps({"online": False}), retain=True)
    time.sleep(0.5)
    c.loop_stop()


if __name__ == "__main__":
    main()
