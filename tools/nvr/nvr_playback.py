#!/usr/bin/env python3
"""Playback & event motion langsung dari NVR XMEye/Xiongmai lama (protokol DVRIP BINER, port 34567).

Protokol ini direkonstruksi dari rekaman Wireshark aplikasi CMS (bukan varian JSON; itu sebabnya
login JSON tidak pernah dijawab NVR). Tanpa library tambahan; butuh ffmpeg hanya untuk .mp4.

    # daftar rekaman CAM5 hari ini ([M] = motion, [R] = biasa, [A] = alarm)
    python3 tools/nvr/nvr_playback.py list --channel 5
    python3 tools/nvr/nvr_playback.py list --channel 5 --date 2026-10-06 --motion

    # ambil klip (otomatis memilih file yang memuat jam itu) -> mp4
    python3 tools/nvr/nvr_playback.py clip --channel 5 --start "2026-10-06 07:56:23" --seconds 20 -o klip.mp4

    # dengarkan alarm langsung dari NVR (gerakkan sesuatu di depan kamera)
    python3 tools/nvr/nvr_playback.py alarm

Struktur pesan (little-endian, header 20 byte: FF ver 00 00 | session | seq | tot cur | msg_id | len):
  1000 login     user[32] + sofia_hash[32] + u32 1 + u32 2           -> 1001 (ret 100 = OK)
  1440 cari file sess,0,ch,ch,"h264"[..],0x2a,..,0xffff,T_awal,T_akhir -> 1441 (maks 64 file/halaman)
  1424 (koneksi DATA baru) klaim playback file                        -> 1425, lalu 1422 = isi file XM
  1420 (koneksi kontrol) a,b: 0,0 mulai | 0,4 seek | 4,0e jeda | 4,0f lanjut | 0,1 stop
  1500 berlangganan alarm, 1006 keepalive
  Waktu = 8 x u32: tahun, bulan, tanggal, hari, jam, menit, detik, 0
"""
import argparse
import datetime as dt
import hashlib
import os
import re
import shutil
import socket
import struct
import subprocess
import sys
import threading
import time

# ----------------------------------------------------------------------------- dasar

def sofia_hash(password: str) -> bytes:
    d = hashlib.md5(password.encode()).digest()
    out = ""
    for i in range(0, 16, 2):
        c = (d[i] + d[i + 1]) % 62
        out += chr(48 + c) if c < 10 else chr(65 + c - 10) if c < 36 else chr(97 + c - 36)
    return out.encode()


def pack_time(t: dt.datetime) -> bytes:
    return struct.pack("<8I", t.year, t.month, t.day, 0, t.hour, t.minute, t.second, 0)


def unpack_time(b: bytes, off: int = 0) -> dt.datetime | None:
    y, mo, d, _w, h, mi, s, _ = struct.unpack_from("<8I", b, off)
    if not (2000 <= y <= 2099):
        return None
    try:
        return dt.datetime(y, mo, d, h, mi, s)
    except (ValueError, OverflowError):
        return None


def cstr(b: bytes) -> str:
    return b.split(b"\0", 1)[0].decode("utf-8", "replace")


class Conn:
    def __init__(self, host, port, timeout=8):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.session = 0
        self.seq = 0
        self.lock = threading.Lock()

    def send(self, msg_id: int, body: bytes = b"", seq: int | None = None):
        if seq is None:
            seq = self.seq
            self.seq += 1
        hdr = struct.pack("<BB2xIIBBHI", 0xFF, 0, self.session, seq, 0, 0, msg_id, len(body))
        with self.lock:
            self.sock.sendall(hdr + body)

    def _exact(self, n):
        buf = bytearray()
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise ConnectionError("NVR menutup koneksi")
            buf += chunk
        return bytes(buf)

    def recv(self):
        h = self._exact(20)
        if h[0] != 0xFF:
            raise ConnectionError(f"header tidak dikenal: {h.hex()}")
        msg_id, length = struct.unpack_from("<HI", h, 14)
        return msg_id, self._exact(length)

    def recv_until(self, want: int):
        while True:
            mid, body = self.recv()
            if mid == want:
                return body

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


def ret_code(body: bytes) -> int:
    return struct.unpack_from("<I", body, 4)[0] if len(body) >= 8 else -1


def login(host, port, user, password) -> Conn:
    c = Conn(host, port)
    body = user.encode()[:31].ljust(32, b"\0") + sofia_hash(password).ljust(32, b"\0") + struct.pack("<II", 1, 2)
    c.send(1000, body, seq=0)
    mid, rsp = c.recv()
    if mid != 1001 or len(rsp) < 20:
        raise RuntimeError(f"balasan login tidak dikenal: msg {mid} {rsp.hex()}")
    ret = struct.unpack_from("<I", rsp, 8)[0]
    if ret != 100:
        raise RuntimeError(f"LOGIN GAGAL (kode {ret}). Cek user/password NVR.")
    c.session = struct.unpack_from("<I", rsp, 16)[0]
    c.seq = 1
    return c


def keepalive(c: Conn, stop: threading.Event, every=15):
    while not stop.wait(every):
        try:
            c.send(1006, b"", seq=0)
        except OSError:
            return

# ----------------------------------------------------------------------------- cari file

NAME_RE = re.compile(r"/(\d{4}-\d{2}-\d{2})/(\d{3})/(\d{2}\.\d{2}\.\d{2})-(\d{2}\.\d{2}\.\d{2})\[(\w)\]")
EVENTS = {1: "alarm input", 4: "MOTION", 5: "video hilang", 6: "kamera ditutup"}
KIND = {"M": "motion", "A": "alarm", "R": "biasa", "H": "manual"}


def find_files(c: Conn, channel0: int, start: dt.datetime, end: dt.datetime, type_field: int | None = None):
    """Semua file di rentang waktu (dengan halaman 64 file)."""
    files, cursor = [], start
    for _ in range(100):
        body = bytearray(140)
        struct.pack_into("<IIII", body, 0, c.session, 0, channel0, channel0 if type_field is None else type_field)
        body[0x10:0x14] = b"h264"
        struct.pack_into("<I", body, 0x28, 0x2A)
        struct.pack_into("<I", body, 0x48, 0xFFFF)
        body[0x4C:0x6C] = pack_time(cursor)
        body[0x6C:0x8C] = pack_time(end)
        c.send(1440, bytes(body))
        rsp = c.recv_until(1441)
        if ret_code(rsp) != 100:
            if not files:
                print(f"(tidak ada rekaman / kode {ret_code(rsp)})")
            break
        page = []
        off = 0x18
        count = struct.unpack_from("<I", rsp, 0x10)[0] if len(rsp) >= 0x14 else 0
        if os.environ.get("NVR_DEBUG"):
            print(f"[debug] 1441: {len(rsp)} byte, jumlah={count}, awal={rsp[:0x40].hex(' ')}")
        for _ in range(min(count, 64) if count else 64):
            if off + 180 > len(rsp):
                break
            ch, size_kb = struct.unpack_from("<iI", rsp, off)
            name = cstr(rsp[off + 8: off + 116])
            b, e = unpack_time(rsp, off + 116), unpack_time(rsp, off + 148)
            if name and b and e:
                m = NAME_RE.search(name)
                page.append({"name": name, "size_kb": size_kb, "begin": b, "end": e,
                             "kind": KIND.get(m.group(5), m.group(5)) if m else "?", "raw": rsp[off: off + 180]})
            off += 184
        files += page
        if len(page) < 64:
            break
        cursor = page[-1]["end"] + dt.timedelta(seconds=1)
        if cursor >= end:
            break
    # buang duplikat di batas halaman
    seen, out = set(), []
    for f in files:
        if f["name"] not in seen:
            seen.add(f["name"])
            out.append(f)
    return out

# ----------------------------------------------------------------------------- playback

def pb_body(session, a, b, f, t_begin, t_end) -> bytes:
    body = bytearray(200)
    struct.pack_into("<IIII", body, 0, session, 0, a, b)
    name = f["name"].encode()[:107]
    body[0x14:0x14 + len(name)] = name
    body[0x80:0xA0] = pack_time(t_begin)
    body[0xA0:0xC0] = pack_time(t_end)
    return bytes(body)


def xm_time(v: int) -> dt.datetime | None:
    try:
        return dt.datetime((v >> 26) + 2000, (v >> 22) & 15, (v >> 17) & 31, (v >> 12) & 31, (v >> 6) & 63, v & 63)
    except (ValueError, OverflowError):
        return None


class XmDemux:
    """Pecah aliran file XM (.h264 NVR) jadi H.264 Annex-B murni + info fps/ukuran/jam frame."""

    def __init__(self):
        self.buf = bytearray()
        self.fps = 0
        self.size = (0, 0)
        self.last_time = None
        self.frames = 0

    def feed(self, data: bytes) -> bytes:
        self.buf += data
        out = bytearray()
        b = self.buf
        i = 0
        while True:
            if len(b) - i < 8:
                break
            if b[i:i + 3] != b"\x00\x00\x01":
                j = b.find(b"\x00\x00\x01", i + 1)
                if j < 0:
                    i = max(i, len(b) - 3)
                    break
                i = j
                continue
            t = b[i + 3]
            if t == 0xFD:
                if len(b) - i < 16:
                    break
                ln = struct.unpack_from("<I", b, i + 12)[0]
                hl = 16
                self.fps = b[i + 5] or self.fps
                self.size = (b[i + 6] * 8, b[i + 7] * 8)
                self.last_time = xm_time(struct.unpack_from("<I", b, i + 8)[0]) or self.last_time
            elif t == 0xFC:
                ln, hl = struct.unpack_from("<I", b, i + 4)[0], 8
            elif t in (0xFA, 0xF9, 0xF0, 0xF1):
                ln, hl = struct.unpack_from("<H", b, i + 6)[0], 8
            else:  # bukan header XM (mis. NAL di tengah) -> cari header berikutnya
                i += 3
                continue
            if len(b) - i < hl + ln:
                break
            if t in (0xFD, 0xFC):
                out += b[i + hl: i + hl + ln]
                self.frames += 1
            i += hl + ln
        del self.buf[:i]
        return bytes(out)


def playback(host, port, user, password, f, want_from: dt.datetime, want_to: dt.datetime, sink, idle=8.0, limit_s=600,
             progress=False):
    """Putar `f` dari NVR; H.264 dari want_from..want_to dikirim ke sink(bytes). Mengembalikan XmDemux."""
    ctl = login(host, port, user, password)
    stop = threading.Event()
    threading.Thread(target=keepalive, args=(ctl, stop), daemon=True).start()
    data = Conn(host, port)
    data.session = ctl.session
    dm = XmDemux()
    try:
        data.send(1424, pb_body(ctl.session, 0, 7, f, f["begin"], f["end"]), seq=0)
        mid, rsp = data.recv()
        if mid != 1425 or ret_code(rsp) != 100:
            raise RuntimeError(f"NVR menolak koneksi data playback: msg {mid} kode {ret_code(rsp)}")
        ctl.send(1420, pb_body(ctl.session, 0, 0, f, f["begin"], f["end"]))
        if want_from > f["begin"] + dt.timedelta(seconds=2):
            ctl.send(1420, pb_body(ctl.session, 0, 4, f, want_from, f["end"]))   # seek
        data.sock.settimeout(idle)
        # Durasi dihitung RELATIF dari frame pertama: jam OSD di dalam video bisa beda beberapa
        # detik dari jam di nama file, jadi jangan bandingkan jam absolut.
        want_len = (want_to - want_from).total_seconds()
        t0, first, prev, last_print, total = time.time(), None, None, 0.0, 0
        try:
            while time.time() - t0 < limit_s:
                try:
                    mid, body = data.recv()
                except (socket.timeout, ConnectionError):
                    break                               # NVR berhenti mengirim = akhir file
                if mid != 1422:
                    continue
                if not body:
                    break
                total += len(body)
                h264 = dm.feed(body)
                if h264:
                    sink(h264)
                ft = dm.last_time
                if ft and first is None:
                    first, prev = ft, ft
                elif ft and first and not (0 <= (ft - prev).total_seconds() <= 10):
                    first += ft - prev                  # lompatan karena seek: jangan dihitung durasi
                if ft:
                    prev = ft
                got = (ft - first).total_seconds() if ft and first else 0
                if progress and time.time() - last_print >= 1:
                    last_print = time.time()
                    print(f"  {time.time()-t0:5.1f} dtk: {total/1e6:6.1f} MB, video {got:4.0f}/{want_len:.0f} dtk"
                          f"{f', jam OSD {ft:%H:%M:%S}' if ft else ''}", flush=True)
                if got >= want_len:
                    break
        except KeyboardInterrupt:
            print("  dihentikan; menyimpan yang sudah diterima...")
    finally:
        try:
            ctl.send(1420, pb_body(ctl.session, 0, 1, f, f["begin"], f["end"]))  # stop
        except OSError:
            pass
        stop.set()
        data.close()
        ctl.close()
    return dm

# ----------------------------------------------------------------------------- perintah

def day_range(date_str):
    d = dt.datetime.strptime(date_str, "%Y-%m-%d") if date_str else dt.datetime.now().replace(hour=0, minute=0, second=0, microsecond=0)
    return d, d.replace(hour=23, minute=59, second=59)


def cmd_list(a):
    c = login(a.host, a.port, a.user, a.password)
    print(f"Login OK (sesi 0x{c.session:X})")
    s, e = day_range(a.date)
    files = find_files(c, a.channel - 1, s, e, a.type_field)
    c.close()
    folder = f"/{a.channel:03d}/"
    if files and not any(folder in f["name"] for f in files):
        print(f"PERINGATAN: file bukan dari folder {folder}; coba --type-field 0")
    shown = [f for f in files if not a.motion or f["kind"] == "motion"]
    for f in shown:
        dur = int((f["end"] - f["begin"]).total_seconds())
        print(f"{f['begin']:%H:%M:%S}-{f['end']:%H:%M:%S}  {f['kind']:7s} {dur:5d} dtk {f['size_kb']/1024:8.1f} MB  {f['name']}")
    n_m = sum(1 for f in files if f["kind"] == "motion")
    print(f"\n{len(files)} file, {n_m} event motion")


def cmd_clip(a):
    start = dt.datetime.strptime(a.start, "%Y-%m-%d %H:%M:%S")
    end = start + dt.timedelta(seconds=a.seconds)
    c = login(a.host, a.port, a.user, a.password)
    files = find_files(c, a.channel - 1, start - dt.timedelta(hours=2), end, a.type_field)
    c.close()
    f = next((x for x in files if x["begin"] <= start < x["end"]), None)
    if not f:
        sys.exit("Tidak ada rekaman yang memuat jam itu. Jalankan perintah list.")
    print(f"File: {f['name']}  ({f['kind']})")
    raw = os.path.splitext(a.output)[0] + ".h264"
    with open(raw, "wb") as fh:
        t = time.time()
        print("Menerima video (NVR mengirim kira-kira secepat waktu nyata)...")
        dm = playback(a.host, a.port, a.user, a.password, f, start, end, fh.write, progress=True)
    print(f"{dm.frames} frame, {dm.size[0]}x{dm.size[1]} @ {dm.fps} fps, {os.path.getsize(raw)/1e6:.1f} MB dalam {time.time()-t:.1f} dtk")
    if a.output.endswith(".mp4"):
        if not shutil.which("ffmpeg"):
            print(f"ffmpeg tidak ditemukan di PATH; video mentah tersimpan di {raw} (bisa diputar VLC).\n"
                  "Pasang ffmpeg (Windows: winget install Gyan.FFmpeg, Pi: sudo apt install ffmpeg) untuk .mp4.")
            return
        r = subprocess.run(["ffmpeg", "-v", "error", "-y", "-framerate", str(dm.fps or 15), "-i", raw,
                            "-c", "copy", "-movflags", "+faststart", a.output])
        if r.returncode == 0:
            os.remove(raw)
            print(f"Tersimpan: {a.output}")
        else:
            print(f"ffmpeg gagal; H.264 mentah ada di {raw}")


def cmd_alarm(a):
    c = login(a.host, a.port, a.user, a.password)
    stop = threading.Event()
    threading.Thread(target=keepalive, args=(c, stop), daemon=True).start()
    c.send(1500, struct.pack("<III", c.session, 0x5DC, 0))
    c.sock.settimeout(None)
    print("Berlangganan alarm. Gerakkan sesuatu di depan kamera... (Ctrl+C berhenti)")
    try:
        while True:
            mid, body = c.recv()
            if mid in (1007,):
                continue
            if mid == 1504 and len(body) >= 0x3C:
                ch, ev, st = struct.unpack_from("<III", body, 0x10)
                t = unpack_time(body, 0x1C)
                name = EVENTS.get(ev, f"event {ev}")
                skew = ""
                if t:
                    d = (dt.datetime.now() - t).total_seconds()
                    if abs(d) > 30:
                        skew = f"  (jam NVR selisih {d/60:+.1f} menit dari PC)"
                print(f"[{time.strftime('%H:%M:%S')}] CAM{ch + 1} {name} {'MULAI' if st == 0 else 'SELESAI'}  jam NVR {t:%H:%M:%S}{skew}" if t
                      else f"[{time.strftime('%H:%M:%S')}] CAM{ch + 1} {name} status {st}")
            else:
                print(f"[{time.strftime('%H:%M:%S')}] msg {mid} len {len(body)}: {body[:96].hex(' ')}")
    except KeyboardInterrupt:
        pass
    finally:
        stop.set()
        c.close()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--host", default="192.168.1.200")
    ap.add_argument("--port", type=int, default=34567)
    ap.add_argument("--user", default="admin")
    ap.add_argument("--password", default="")
    ap.add_argument("--type-field", type=int, default=None, help=argparse.SUPPRESS)
    sub = ap.add_subparsers(dest="cmd", required=True)
    lp = sub.add_parser("list")
    lp.add_argument("--channel", type=int, default=5, help="nomor CAM (1 = CAM1)")
    lp.add_argument("--date", help="YYYY-MM-DD (default hari ini)")
    lp.add_argument("--motion", action="store_true", help="hanya event motion")
    cp = sub.add_parser("clip")
    cp.add_argument("--channel", type=int, default=5)
    cp.add_argument("--start", required=True, help='"YYYY-MM-DD HH:MM:SS" (jam NVR)')
    cp.add_argument("--seconds", type=int, default=20)
    cp.add_argument("-o", "--output", default="klip.mp4")
    sub.add_parser("alarm")
    a = ap.parse_args()
    {"list": cmd_list, "clip": cmd_clip, "alarm": cmd_alarm}[a.cmd](a)


if __name__ == "__main__":
    main()
