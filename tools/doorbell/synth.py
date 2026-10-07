"""Gambar sintetis untuk node palsu (bukan untuk produksi)."""
from __future__ import annotations

import io

from PIL import Image, ImageChops, ImageDraw, ImageEnhance


def make_jpeg(scene="empty", brightness=1.0, size=(320, 240), roi=(0.2, 0.3, 0.6, 0.6)) -> bytes:
    w, h = size
    base = Image.linear_gradient("L").resize(size).point(lambda v: int(90 + v * 0.3))
    im = ImageChops.add(base, Image.effect_noise(size, 6), 1.0, -128)
    d = ImageDraw.Draw(im)
    rx, ry, rw, rh = int(roi[0] * w), int(roi[1] * h), int(roi[2] * w), int(roi[3] * h)
    if scene == "package":
        d.rectangle([int(rx + rw * .2), int(ry + rh * .3), int(rx + rw * .5), int(ry + rh * .75)], fill=40)
    elif scene == "person":
        d.rectangle([int(rx + rw * .03), int(ry + rh * .03), int(rx + rw * .97), int(ry + rh * .97)], fill=30)
    if brightness != 1.0:
        im = ImageEnhance.Brightness(im).enhance(brightness)
    buf = io.BytesIO()
    im.convert("RGB").save(buf, "JPEG", quality=85)
    return buf.getvalue()
