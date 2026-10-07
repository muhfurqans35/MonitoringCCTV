//! Deteksi gerak sederhana per kamera: bandingkan frame sekarang dengan frame sebelumnya
//! (grayscale, diperkecil). Murah di CPU, sehingga model YOLO yang mahal hanya dijalankan saat
//! benar-benar ada gerakan.
//!
//! Langkah:
//! 1. decode JPEG -> grayscale -> diperkecil ke `width` px (redam noise sensor & kompresi)
//! 2. kompensasi kecerahan global: selisih rata-rata dua frame dikurangkan dulu, sehingga
//!    perubahan cahaya merata (awan, auto-exposure) tidak dianggap gerakan
//! 3. hitung porsi piksel di dalam ROI yang berubah > `pixel_threshold`
//! 4. porsi >= `max_fraction` -> perubahan cahaya besar (lampu/IR malam) -> diabaikan
//!    porsi >= `min_fraction` selama `consecutive` frame berturut-turut -> GERAKAN

use image::imageops::FilterType;

use crate::settings::MotionSettings;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionResult {
    /// Porsi piksel berubah di ROI (0-1).
    pub score: f32,
    pub triggered: bool,
    /// true bila frame ini dianggap perubahan cahaya global (diabaikan).
    pub lighting_change: bool,
}

pub struct MotionDetector {
    cfg: MotionSettings,
    roi: [f32; 4],
    prev: Option<(Vec<u8>, u32, u32)>,
    hits: u32,
}

impl MotionDetector {
    pub fn new(cfg: MotionSettings, roi: Option<[f32; 4]>) -> Self {
        Self { cfg, roi: roi.unwrap_or([0.0, 0.0, 1.0, 1.0]), prev: None, hits: 0 }
    }

    /// Proses satu frame JPEG. Frame pertama hanya menjadi referensi (tidak pernah memicu).
    pub fn feed(&mut self, jpeg: &[u8]) -> anyhow::Result<MotionResult> {
        let img = image::load_from_memory(jpeg).map_err(|e| anyhow::anyhow!("gagal decode frame: {e}"))?;
        let w = self.cfg.width.max(32);
        let h = ((img.height() as f32 / img.width().max(1) as f32) * w as f32).round().max(16.0) as u32;
        let gray = image::imageops::resize(&img.to_luma8(), w, h, FilterType::Triangle).into_raw();
        Ok(self.feed_gray(gray, w, h))
    }

    /// Inti algoritma (dipisah agar bisa diuji tanpa JPEG).
    pub fn feed_gray(&mut self, gray: Vec<u8>, w: u32, h: u32) -> MotionResult {
        // Frame pertama, atau resolusi stream berubah: jadikan referensi saja.
        let prev = match self.prev.take() {
            Some((p, pw, ph)) if pw == w && ph == h => p,
            _ => {
                self.prev = Some((gray, w, h));
                self.hits = 0;
                return MotionResult { score: 0.0, triggered: false, lighting_change: false };
            }
        };

        // ROI dalam piksel kerja.
        let [rx, ry, rw, rh] = self.roi;
        let x0 = ((rx.clamp(0.0, 1.0) * w as f32) as u32).min(w.saturating_sub(1));
        let y0 = ((ry.clamp(0.0, 1.0) * h as f32) as u32).min(h.saturating_sub(1));
        let x1 = (((rx + rw).clamp(0.0, 1.0) * w as f32).ceil() as u32).clamp(x0 + 1, w);
        let y1 = (((ry + rh).clamp(0.0, 1.0) * h as f32).ceil() as u32).clamp(y0 + 1, h);

        // Kompensasi kecerahan global (rata-rata selisih bertanda di ROI).
        let mut sum_diff: i64 = 0;
        let mut n: i64 = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * w + x) as usize;
                sum_diff += gray[i] as i64 - prev[i] as i64;
                n += 1;
            }
        }
        let mean_shift = if n > 0 { sum_diff / n } else { 0 };

        let thr = self.cfg.pixel_threshold as i64;
        let mut changed: i64 = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * w + x) as usize;
                if (gray[i] as i64 - prev[i] as i64 - mean_shift).abs() > thr {
                    changed += 1;
                }
            }
        }
        let score = if n > 0 { changed as f32 / n as f32 } else { 0.0 };
        self.prev = Some((gray, w, h));

        if score >= self.cfg.max_fraction {
            self.hits = 0;
            return MotionResult { score, triggered: false, lighting_change: true };
        }
        if score >= self.cfg.min_fraction {
            self.hits += 1;
        } else {
            self.hits = 0;
        }
        MotionResult { score, triggered: self.hits >= self.cfg.consecutive.max(1), lighting_change: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 160;
    const H: u32 = 120;

    fn scene(bg: u8, obj: Option<(u32, u32)>) -> Vec<u8> {
        let mut v = vec![bg; (W * H) as usize];
        // tekstur ringan agar tidak seragam
        for (i, p) in v.iter_mut().enumerate() {
            *p = p.saturating_add(((i * 7) % 5) as u8);
        }
        if let Some((ox, oy)) = obj {
            for y in oy..(oy + 30).min(H) {
                for x in ox..(ox + 20).min(W) {
                    v[(y * W + x) as usize] = 230;
                }
            }
        }
        v
    }

    fn det(consecutive: u32) -> MotionDetector {
        MotionDetector::new(MotionSettings { consecutive, ..Default::default() }, None)
    }

    #[test]
    fn first_frame_never_triggers_and_static_scene_is_quiet() {
        let mut d = det(1);
        assert!(!d.feed_gray(scene(80, None), W, H).triggered);
        let r = d.feed_gray(scene(80, None), W, H);
        assert!(!r.triggered && r.score < 0.001);
    }

    #[test]
    fn moving_object_triggers_after_consecutive_frames() {
        let mut d = det(2);
        d.feed_gray(scene(80, None), W, H);
        let r1 = d.feed_gray(scene(80, Some((10, 40))), W, H);
        assert!(r1.score > 0.02 && !r1.triggered, "butuh 2 frame berturut-turut: {r1:?}");
        let r2 = d.feed_gray(scene(80, Some((60, 40))), W, H);
        assert!(r2.triggered, "{r2:?}");
    }

    #[test]
    fn global_brightness_change_is_not_motion() {
        let mut d = det(1);
        d.feed_gray(scene(80, None), W, H);
        // seluruh frame lebih terang 40 (awan/auto-exposure) -> dikompensasi
        let r = d.feed_gray(scene(120, None), W, H);
        assert!(!r.triggered && r.score < 0.01, "{r:?}");
    }

    #[test]
    fn roi_excludes_motion_outside_area() {
        let mut d = MotionDetector::new(MotionSettings { consecutive: 1, ..Default::default() }, Some([0.5, 0.0, 0.5, 1.0]));
        d.feed_gray(scene(80, None), W, H);
        // objek di separuh kiri, ROI separuh kanan
        assert!(!d.feed_gray(scene(80, Some((10, 40))), W, H).triggered);
        assert!(d.feed_gray(scene(80, Some((110, 40))), W, H).triggered);
    }

    #[test]
    fn decodes_real_jpeg() {
        let img = image::GrayImage::from_raw(W, H, scene(80, None)).unwrap();
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(img).write_to(&mut buf, image::ImageFormat::Jpeg).unwrap();
        let mut d = det(1);
        assert!(!d.feed(buf.get_ref()).unwrap().triggered);
    }
}
