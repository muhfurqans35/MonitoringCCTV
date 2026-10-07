#!/usr/bin/env python3
"""Periksa cara playback rekaman NVR (XMEye/Xiongmai/Xihancam) yang bisa dipakai web Smart Kost.
Tanpa library tambahan; bisa dijalankan di Raspberry Pi atau PC Windows.

    python3 tools/nvr/nvr_playback_probe.py --host 192.168.1.200 --channel 5
    python3 tools/nvr/nvr_playback_probe.py --host 192.168.1.200 --channel 5 --minutes-ago 30

Yang diperiksa:
  1. RTSP live (baseline) + beberapa pola URL RTSP playback (XM, Dahua, Hikvision) dengan
     jam mulai/selesai. DESCRIBE 200 belum tentu playback: lihat baris "range" di SDP —
     playback biasanya punya rentang waktu/durasi, live biasanya "npt=0-".
  2. ONVIF di beberapa port: layanan Search/Replay/Recording (Profile G). Bila ada, NVR bisa
     memberi daftar rekaman, DAFTAR EVENT MOTION, dan URL replay secara standar.
Kirimkan seluruh output ke pengembang.
"""
import argparse
import base64
import datetime as dt
import hashlib
import os
import re
import socket
import urllib.request

# ----------------------------------------------------------------------------- RTSP

def rtsp_request(host, port, method, url, cseq, auth=None, timeout=6):
    lines = [f"{method} {url} RTSP/1.0", f"CSeq: {cseq}", "User-Agent: smartkost-probe", "Accept: application/sdp"]
    if auth:
        lines.append(f"Authorization: {auth}")
    req = ("\r\n".join(lines) + "\r\n\r\n").encode()
    with socket.create_connection((host, port), timeout=timeout) as s:
        s.settimeout(timeout)
        s.sendall(req)
        data = b""
        try:
            while True:
                chunk = s.recv(8192)
                if not chunk:
                    break
                data += chunk
                head, sep, body = data.partition(b"\r\n\r\n")
                if sep:
                    m = re.search(rb"Content-Length:\s*(\d+)", head, re.I)
                    if not m or len(body) >= int(m.group(1)):
                        break
        except socket.timeout:
            pass
    text = data.decode("utf-8", "replace")
    status = re.match(r"RTSP/1\.0 (\d+)", text)
    return (int(status.group(1)) if status else 0), text


def digest_auth(www, user, pw, method, uri):
    realm = re.search(r'realm="([^"]*)"', www)
    nonce = re.search(r'nonce="([^"]*)"', www)
    if not realm or not nonce:
        return None
    ha1 = hashlib.md5(f"{user}:{realm.group(1)}:{pw}".encode()).hexdigest()
    ha2 = hashlib.md5(f"{method}:{uri}".encode()).hexdigest()
    resp = hashlib.md5(f"{ha1}:{nonce.group(1)}:{ha2}".encode()).hexdigest()
    return f'Digest username="{user}", realm="{realm.group(1)}", nonce="{nonce.group(1)}", uri="{uri}", response="{resp}"'


def describe(host, port, url, user, pw):
    try:
        code, text = rtsp_request(host, port, "DESCRIBE", url, 2)
        if code == 401:
            www = re.search(r"WWW-Authenticate:\s*(.+)", text, re.I)
            www = www.group(1).strip() if www else ""
            auth = digest_auth(www, user, pw, "DESCRIBE", url) if www.lower().startswith("digest") \
                else "Basic " + base64.b64encode(f"{user}:{pw}".encode()).decode()
            if auth:
                code, text = rtsp_request(host, port, "DESCRIBE", url, 3, auth)
        return code, text
    except OSError as e:
        return -1, str(e)


def sdp_summary(text):
    body = text.partition("\r\n\r\n")[2]
    keep = [l for l in body.splitlines() if l.startswith(("a=range", "a=control", "m=", "s=", "a=x-", "t="))]
    return "; ".join(keep[:8]) or "(tanpa SDP)"

# ----------------------------------------------------------------------------- ONVIF

def onvif_post(url, body, timeout=5):
    env = ('<?xml version="1.0" encoding="UTF-8"?><s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope">'
           f"<s:Body>{body}</s:Body></s:Envelope>")
    req = urllib.request.Request(url, data=env.encode(), headers={"Content-Type": "application/soap+xml; charset=utf-8"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")
    except Exception as e:  # noqa: BLE001
        return -1, str(e)


def onvif_probe(host, ports):
    found = False
    for port in ports:
        url = f"http://{host}:{port}/onvif/device_service"
        code, text = onvif_post(url, '<GetServices xmlns="http://www.onvif.org/ver10/device/wsdl"><IncludeCapability>false</IncludeCapability></GetServices>')
        if code == -1:
            print(f"  port {port}: tidak tersambung ({text[:60]})")
            continue
        ns = re.findall(r"<[^>]*Namespace>([^<]+)<", text)
        xaddr = re.findall(r"<[^>]*XAddr>([^<]+)<", text)
        if code != 200 or not ns:
            # sebagian NVR hanya mendukung GetCapabilities
            code2, text2 = onvif_post(url, '<GetCapabilities xmlns="http://www.onvif.org/ver10/device/wsdl"><Category>All</Category></GetCapabilities>')
            caps = re.findall(r"<(?:\w+:)?(Search|Replay|Recording|Events|Media)>\s*<(?:\w+:)?XAddr>([^<]+)<", text2)
            print(f"  port {port}: GetServices HTTP {code}, GetCapabilities HTTP {code2}")
            if "NotAuthorized" in text + text2 or code in (400, 401) and code2 in (400, 401):
                print("    -> ONVIF ada tetapi butuh autentikasi (catat port ini)")
                found = True
            for name, addr in caps:
                print(f"    {name:9s} {addr}")
                found = True
            continue
        found = True
        print(f"  port {port}: ONVIF aktif, {len(ns)} layanan")
        for n, a in zip(ns, xaddr):
            mark = "  <-- PLAYBACK" if any(k in n for k in ("replay", "search", "recording")) else ""
            print(f"    {n}  {a}{mark}")
    if not found:
        print("  ONVIF tidak ditemukan di port yang dicoba.")

# ----------------------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--host", default="192.168.1.200")
    ap.add_argument("--rtsp-port", type=int, default=554)
    ap.add_argument("--user", default="admin")
    ap.add_argument("--password", default="")
    ap.add_argument("--channel", type=int, default=5, help="nomor CAM di NVR (1 = CAM1)")
    ap.add_argument("--minutes-ago", type=int, default=60, help="awal rentang yang dicari")
    ap.add_argument("--onvif-ports", default="80,8899,8000,8080,5000,2020")
    a = ap.parse_args()

    start = dt.datetime.now() - dt.timedelta(minutes=a.minutes_ago)
    end = start + dt.timedelta(minutes=2)
    u, p, h, port, ch = a.user, a.password, a.host, a.rtsp_port, a.channel
    us = lambda t: t.strftime("%Y_%m_%d_%H_%M_%S")  # noqa: E731
    hik = lambda t: t.strftime("%Y%m%dT%H%M%SZ")    # noqa: E731
    cred = f"{u}:{p}@" if u else ""

    candidates = [
        ("live (baseline)", f"rtsp://{h}:{port}/user={u}&password={p}&channel={ch}&stream=0.sdp?real_stream"),
        ("XM playback A", f"rtsp://{h}:{port}/user={u}&password={p}&channel={ch}&stream=0.sdp?real_stream&starttime={us(start)}&endtime={us(end)}"),
        ("XM playback B", f"rtsp://{h}:{port}/user={u}&password={p}&channel={ch}&stream=0&starttime={us(start)}&endtime={us(end)}.sdp"),
        ("XM playback C", f"rtsp://{h}:{port}/playback?user={u}&password={p}&channel={ch}&starttime={us(start)}&endtime={us(end)}"),
        ("Dahua-style", f"rtsp://{cred}{h}:{port}/cam/playback?channel={ch}&subtype=0&starttime={us(start)}&endtime={us(end)}"),
        ("Hikvision-style", f"rtsp://{cred}{h}:{port}/Streaming/tracks/{ch}01?starttime={hik(start)}&endtime={hik(end)}"),
    ]
    print(f"Rentang uji: {start:%Y-%m-%d %H:%M:%S} s/d {end:%H:%M:%S} (jam NVR harus sama dengan jam PC/Pi)\n")
    print("== 1. RTSP ==")
    for name, url in candidates:
        code, text = describe(h, port, url, u, p)
        shown = url.replace(f"password={p}", "password=***") if p else url
        verdict = {200: "OK", 401: "butuh auth", 404: "tidak ada", 454: "sesi tidak ditemukan", -1: "koneksi gagal"}.get(code, f"HTTP {code}")
        print(f"- {name:16s} {verdict:12s} {shown}")
        if code == 200:
            print(f"    SDP: {sdp_summary(text)}")
    print("\n== 2. ONVIF ==")
    onvif_probe(h, [int(x) for x in a.onvif_ports.split(",") if x.strip()])
    print("\n== 3. Web NVR ==")
    try:
        with urllib.request.urlopen(f"http://{h}/", timeout=5) as r:
            print(f"  HTTP {r.status}, Server: {r.headers.get('Server')}")
    except Exception as e:  # noqa: BLE001
        print(f"  {e}")
    print("\nSelesai. Kirim seluruh output ini.")


if __name__ == "__main__":
    os.environ.setdefault("PYTHONIOENCODING", "utf-8")
    main()
