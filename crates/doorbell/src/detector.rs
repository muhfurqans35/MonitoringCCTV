//! Deteksi status zona paket: baseline klasik (selisih frame) pada region of interest.
//! Kelas: kosong | ada_paket | terhalang (+ tidak_terkalibrasi bila referensi belum diambil).
//!
//! - Hanya ROI yang dianalisis dan dikirim sebagai foto (bagian frame lain dibuang).
//! - Normalisasi cahaya memakai median piksel DI LUAR ROI, jadi perubahan auto-exposure
//!   kamera tidak dianggap perubahan isi zona dan objek besar di ROI tidak mengacaukan normalisasi.

use image::imageops::{self, FilterType};
use image::{GrayImage, ImageBuffer, Luma};
use std::path::PathBuf;
use std::sync::RwLock;

const W: u32 = 64;
const H: u32 = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneState {
    Kosong,
    AdaPaket,
    Terhalang,
    TidakTerkalibrasi,
}

impl ZoneState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ZoneState::Kosong => "kosong",
            ZoneState::AdaPaket => "ada_paket",
            ZoneState::Terhalang => "terhalang",
            ZoneState::TidakTerkalibrasi => "tidak_terkalibrasi",
        }
    }
}

/// (x0, y0, x1, y1) dalam piksel, dibulatkan seperti implementasi referensi.
fn roi_box(w: u32, h: u32, roi: &[f64; 4]) -> (u32, u32, u32, u32) {
    let x0 = (roi[0] * w as f64).round() as u32;
    let y0 = (roi[1] * h as f64).round() as u32;
    let x1 = (((roi[0] + roi[2]) * w as f64).round() as u32).max(x0 + 1).min(w);
    let y1 = (((roi[1] + roi[3]) * h as f64).round() as u32).max(y0 + 1).min(h);
    (x0.min(w.saturating_sub(1)), y0.min(h.saturating_sub(1)), x1, y1)
}

fn outside_median(gray: &GrayImage, b: (u32, u32, u32, u32)) -> f64 {
    let (sw, sh) = (80u32, 60u32);
    let (w, h) = gray.dimensions();
    let small = imageops::resize(gray, sw, sh, FilterType::Triangle);
    let bx0 = b.0 * sw / w;
    let by0 = b.1 * sh / h;
    let bx1 = (b.2 * sw + w - 1) / w;
    let by1 = (b.3 * sh + h - 1) / h;
    let mut vals: Vec<u8> = Vec::with_capacity((sw * sh) as usize);
    for y in 0..sh {
        for x in 0..sw {
            if !(bx0 <= x && x < bx1 && by0 <= y && y < by1) {
                vals.push(small.get_pixel(x, y).0[0]);
            }
        }
    }
    if vals.is_empty() {
        vals = small.pixels().map(|p| p.0[0]).collect();
    }
    vals.sort_unstable();
    vals[vals.len() / 2] as f64
}

fn prep(jpeg: &[u8], roi: &[f64; 4]) -> anyhow::Result<GrayImage> {
    let gray = image::load_from_memory(jpeg)
        .map_err(|e| anyhow::anyhow!("gambar tidak valid: {e}"))?
        .to_luma8();
    let (w, h) = gray.dimensions();
    let b = roi_box(w, h, roi);
    let med = outside_median(&gray, b);
    let crop = imageops::crop_imm(&gray, b.0, b.1, b.2 - b.0, b.3 - b.1).to_image();
    let small = imageops::resize(&crop, W, H, FilterType::Triangle);
    let small = imageops::blur(&small, 1.0);
    let scale = 128.0 / med.max(1.0);
    let mut lut = [0u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        *v = ((i as f64 * scale + 0.5).min(255.0)) as u8;
    }
    let out: GrayImage = ImageBuffer::from_fn(W, H, |x, y| Luma([lut[small.get_pixel(x, y).0[0] as usize]]));
    Ok(out)
}

fn changed_fraction(a: &GrayImage, b: &GrayImage, pix_thr: u32) -> f64 {
    let mut n = 0u32;
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        let d = (pa.0[0] as i32 - pb.0[0] as i32).unsigned_abs();
        if d >= pix_thr {
            n += 1;
        }
    }
    n as f64 / (W * H) as f64
}

/// Potong hanya area zona (privasi: bagian lain frame dibuang). Keluaran JPEG.
pub fn crop_roi_jpeg(jpeg: &[u8], roi: &[f64; 4], quality: u8) -> anyhow::Result<Vec<u8>> {
    let rgb = image::load_from_memory(jpeg)
        .map_err(|e| anyhow::anyhow!("gambar tidak valid: {e}"))?
        .to_rgb8();
    let (w, h) = rgb.dimensions();
    let b = roi_box(w, h, roi);
    let crop = imageops::crop_imm(&rgb, b.0, b.1, b.2 - b.0, b.3 - b.1).to_image();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&crop)
        .map_err(|e| anyhow::anyhow!("gagal encode JPEG: {e}"))?;
    Ok(out)
}

pub struct Detector {
    roi: [f64; 4],
    pix_thr: u32,
    occupied: f64,
    obstructed: f64,
    reference_path: Option<PathBuf>,
    reference: RwLock<Option<GrayImage>>,
}

impl Detector {
    pub fn new(zone: &crate::config::ZoneCfg, reference_path: Option<PathBuf>) -> Self {
        let reference = reference_path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| prep(&bytes, &zone.roi).ok());
        Self {
            roi: zone.roi,
            pix_thr: zone.pix_thr,
            occupied: zone.occupied_frac,
            obstructed: zone.obstructed_frac,
            reference_path,
            reference: RwLock::new(reference),
        }
    }

    pub fn calibrated(&self) -> bool {
        self.reference.read().map(|r| r.is_some()).unwrap_or(false)
    }

    pub fn set_reference(&self, jpeg: &[u8]) -> anyhow::Result<()> {
        let refimg = prep(jpeg, &self.roi)?;
        if let Some(path) = &self.reference_path {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, jpeg)?;
            std::fs::rename(&tmp, path)?;
        }
        *self.reference.write().map_err(|_| anyhow::anyhow!("lock rusak"))? = Some(refimg);
        Ok(())
    }

    /// (state, score) dengan score = fraksi piksel ROI yang berubah dari referensi kosong.
    pub fn classify(&self, jpeg: &[u8]) -> anyhow::Result<(ZoneState, f64)> {
        let cur = prep(jpeg, &self.roi)?;
        let guard = self.reference.read().map_err(|_| anyhow::anyhow!("lock rusak"))?;
        let Some(refimg) = guard.as_ref() else {
            return Ok((ZoneState::TidakTerkalibrasi, 0.0));
        };
        let frac = changed_fraction(refimg, &cur, self.pix_thr);
        let state = if frac >= self.obstructed {
            ZoneState::Terhalang
        } else if frac >= self.occupied {
            ZoneState::AdaPaket
        } else {
            ZoneState::Kosong
        };
        Ok((state, frac))
    }

    /// Fraksi piksel ROI yang berubah antara dua frame.
    pub fn change(&self, a: &[u8], b: &[u8]) -> anyhow::Result<f64> {
        Ok(changed_fraction(&prep(a, &self.roi)?, &prep(b, &self.roi)?, self.pix_thr))
    }
}

/// Gambar sintetis untuk uji dan alat pengembangan (bukan untuk produksi).
#[cfg(test)]
pub mod synth {
    use image::{ImageBuffer, Rgb};

    /// scene: "empty" | "package" | "person". Frame 320x240 dengan ROI (0.2,0.3,0.6,0.6).
    pub fn make_jpeg(scene: &str, brightness: f64, seed: u32) -> Vec<u8> {
        let (w, h) = (320u32, 240u32);
        let mut state = seed.wrapping_mul(2654435761).wrapping_add(12345);
        let mut noise = move || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            ((state >> 24) as f64 / 255.0 - 0.5) * 12.0 // +-6
        };
        let (rx, ry, rw, rh) = (64.0, 72.0, 192.0, 144.0);
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(w, h, |x, y| {
            let mut v = 90.0 + (y as f64 / h as f64) * 76.0 + noise();
            let (fx, fy) = (x as f64, y as f64);
            let inside = |x0: f64, y0: f64, x1: f64, y1: f64| fx >= x0 && fx < x1 && fy >= y0 && fy < y1;
            match scene {
                "package" if inside(rx + rw * 0.2, ry + rh * 0.3, rx + rw * 0.5, ry + rh * 0.75) => v = 40.0,
                "person" if inside(rx + rw * 0.03, ry + rh * 0.03, rx + rw * 0.97, ry + rh * 0.97) => v = 30.0,
                _ => {}
            }
            let p = (v * brightness).clamp(0.0, 255.0) as u8;
            Rgb([p, p, p])
        });
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img).unwrap();
        out
    }
}
