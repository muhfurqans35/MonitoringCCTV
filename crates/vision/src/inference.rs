use image::imageops::FilterType;
use image::GenericImageView;
use tract_onnx::prelude::*;

/// Hasil 1 deteksi objek, sudah disaring ke kelas yang dikonfigurasi ([vision.model].classes).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Detection {
    /// Nama kelas COCO, mis. "person", "motorcycle".
    pub class: String,
    /// Kelompok untuk alert dashboard: "person" | "vehicle" | nama kelas lain.
    pub label: String,
    pub confidence: f32,
    /// Kotak [x1, y1, x2, y2] dalam piksel gambar ASLI (untuk analisis ulang / dataset).
    pub bbox: [f32; 4],
}

/// 80 kelas COCO berurutan (dataset standar pelatihan YOLOv8/YOLO11).
pub const COCO: [&str; 80] = [
    "person", "bicycle", "car", "motorcycle", "airplane", "bus", "train", "truck", "boat", "traffic light",
    "fire hydrant", "stop sign", "parking meter", "bench", "bird", "cat", "dog", "horse", "sheep", "cow",
    "elephant", "bear", "zebra", "giraffe", "backpack", "umbrella", "handbag", "tie", "suitcase", "frisbee",
    "skis", "snowboard", "sports ball", "kite", "baseball bat", "baseball glove", "skateboard", "surfboard",
    "tennis racket", "bottle", "wine glass", "cup", "fork", "knife", "spoon", "bowl", "banana", "apple",
    "sandwich", "orange", "broccoli", "carrot", "hot dog", "pizza", "donut", "cake", "chair", "couch",
    "potted plant", "bed", "dining table", "toilet", "tv", "laptop", "mouse", "remote", "keyboard",
    "cell phone", "microwave", "oven", "toaster", "sink", "refrigerator", "book", "clock", "vase",
    "scissors", "teddy bear", "hair drier", "toothbrush",
];

fn class_name(id: usize) -> String {
    COCO.get(id).map(|s| s.to_string()).unwrap_or_else(|| format!("class_{id}"))
}

fn group_of(class: &str) -> String {
    match class {
        "person" => "person".into(),
        "bicycle" | "car" | "motorcycle" | "bus" | "truck" => "vehicle".into(),
        other => other.into(),
    }
}

/// Box mentah hasil parsing output model, sebelum non-max suppression.
/// cx, cy, w, h dalam skala pixel gambar ASLI (sudah di-scale balik dari input_size).
#[derive(Clone, Copy)]
struct RawBox {
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    class_id: usize,
    score: f32,
}

pub struct Detector {
    model: TypedRunnableModel<TypedModel>,
    input_size: u32,
    confidence_threshold: f32,
    /// Indeks kelas COCO yang dilaporkan.
    allowed: Vec<usize>,
}

impl Detector {
    /// Load model ONNX sekali di awal startup service (dipakai berulang kali untuk
    /// tiap snapshot — di-load ulang tiap kali akan sangat mahal & jangan dilakukan).
    pub fn load(model_path: &str, input_size: u32, confidence_threshold: f32, classes: &[String]) -> anyhow::Result<Self> {
        let allowed: Vec<usize> = classes
            .iter()
            .filter_map(|c| COCO.iter().position(|n| n.eq_ignore_ascii_case(c.trim())))
            .collect();
        if allowed.is_empty() {
            anyhow::bail!("[vision.model].classes tidak berisi nama kelas COCO yang dikenal: {classes:?}");
        }
        // Bentuk input tetap (1,3,S,S) agar tract bisa mengoptimasi graf untuk ukuran itu.
        let s = input_size as usize;
        let model = tract_onnx::onnx()
            .model_for_path(model_path)
            .map_err(|e| anyhow::anyhow!("gagal load model ONNX di {model_path}: {e}"))?
            .with_input_fact(0, f32::fact([1, 3, s, s]).into())
            .map_err(|e| anyhow::anyhow!("ukuran input {input_size} tidak cocok dengan model: {e}"))?
            .into_optimized()
            .map_err(|e| anyhow::anyhow!("gagal optimasi model: {e}"))?
            .into_runnable()
            .map_err(|e| anyhow::anyhow!("gagal siapkan model untuk inference: {e}"))?;

        Ok(Self { model, input_size, confidence_threshold, allowed })
    }

    /// Jalankan deteksi pada 1 gambar (bytes JPEG/PNG mentah hasil snapshot kamera).
    /// Mengembalikan daftar deteksi person/vehicle yang lolos confidence threshold,
    /// sudah melalui non-max suppression sederhana untuk buang box duplikat.
    pub fn detect(&self, image_bytes: &[u8]) -> anyhow::Result<Vec<Detection>> {
        let img = image::load_from_memory(image_bytes)
            .map_err(|e| anyhow::anyhow!("gagal decode gambar snapshot: {e}"))?;

        let (orig_w, orig_h) = img.dimensions();
        let resized = img.resize_exact(self.input_size, self.input_size, FilterType::Triangle);

        // Preprocessing standar YOLO: HWC->CHW, normalisasi 0-255 -> 0.0-1.0
        let mut input = Tensor::zero::<f32>(&[1, 3, self.input_size as usize, self.input_size as usize])?;
        {
            let arr = input.as_slice_mut::<f32>()?;
            let (w, h) = (self.input_size as usize, self.input_size as usize);
            for y in 0..h {
                for x in 0..w {
                    let px = resized.get_pixel(x as u32, y as u32);
                    let [r, g, b, _] = px.0;
                    arr[0 * h * w + y * w + x] = r as f32 / 255.0;
                    arr[1 * h * w + y * w + x] = g as f32 / 255.0;
                    arr[2 * h * w + y * w + x] = b as f32 / 255.0;
                }
            }
        }

        let outputs = self.model.run(tvec!(input.into()))
            .map_err(|e| anyhow::anyhow!("inference gagal: {e}"))?;

        // Asumsi output format YOLOv8: shape (1, 4 + num_classes, num_boxes) — 4 nilai
        // pertama per box adalah (cx, cy, w, h) dalam skala input_size, sisanya skor
        // per kelas. Kalau kamu pakai model dengan format beda (misal YOLOv5 lama yang
        // outputnya (1, num_boxes, 5+num_classes)), sesuaikan bagian parsing di bawah ini.
        let output = outputs[0].to_array_view::<f32>()
            .map_err(|e| anyhow::anyhow!("gagal baca output model: {e}"))?;

        let shape = output.shape();
        if shape.len() != 3 {
            anyhow::bail!("output model tidak sesuai ekspektasi (shape: {:?})", shape);
        }
        let num_attrs = shape[1]; // 4 + num_classes
        let num_boxes = shape[2];
        let num_classes = num_attrs.saturating_sub(4);

        let scale_x = orig_w as f32 / self.input_size as f32;
        let scale_y = orig_h as f32 / self.input_size as f32;

        let mut raw_boxes: Vec<RawBox> = Vec::new();

        for i in 0..num_boxes {
            let cx = output[[0, 0, i]];
            let cy = output[[0, 1, i]];
            let bw = output[[0, 2, i]];
            let bh = output[[0, 3, i]];

            let mut best_class = 0usize;
            let mut best_score = 0.0f32;
            for c in 0..num_classes {
                let score = output[[0, 4 + c, i]];
                if score > best_score {
                    best_score = score;
                    best_class = c;
                }
            }

            if best_score < self.confidence_threshold || !self.allowed.contains(&best_class) {
                continue;
            }

            raw_boxes.push(RawBox {
                cx: cx * scale_x,
                cy: cy * scale_y,
                w: bw * scale_x,
                h: bh * scale_y,
                class_id: best_class,
                score: best_score,
            });
        }

        let kept = non_max_suppression(raw_boxes, 0.45);

        Ok(kept
            .into_iter()
            .map(|b| {
                let class = class_name(b.class_id);
                Detection {
                    label: group_of(&class),
                    class,
                    confidence: b.score,
                    bbox: [
                        (b.cx - b.w / 2.0).max(0.0),
                        (b.cy - b.h / 2.0).max(0.0),
                        (b.cx + b.w / 2.0).min(orig_w as f32),
                        (b.cy + b.h / 2.0).min(orig_h as f32),
                    ],
                }
            })
            .collect())
    }
}

/// NMS sederhana berbasis IoU (Intersection over Union) — buang box yang tumpang
/// tindih signifikan dengan box lain yang skornya lebih tinggi & kelas sama.
fn non_max_suppression(mut boxes: Vec<RawBox>, iou_threshold: f32) -> Vec<RawBox> {
    boxes.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let mut kept: Vec<RawBox> = Vec::new();
    'outer: for b in boxes {
        for k in &kept {
            if k.class_id == b.class_id && iou(&b, k) > iou_threshold {
                continue 'outer;
            }
        }
        kept.push(b);
    }
    kept
}

fn iou(a: &RawBox, b: &RawBox) -> f32 {
    let (ax1, ay1, ax2, ay2) = (a.cx - a.w / 2.0, a.cy - a.h / 2.0, a.cx + a.w / 2.0, a.cy + a.h / 2.0);
    let (bx1, by1, bx2, by2) = (b.cx - b.w / 2.0, b.cy - b.h / 2.0, b.cx + b.w / 2.0, b.cy + b.h / 2.0);

    let ix1 = ax1.max(bx1);
    let iy1 = ay1.max(by1);
    let ix2 = ax2.min(bx2);
    let iy2 = ay2.min(by2);

    let iw = (ix2 - ix1).max(0.0);
    let ih = (iy2 - iy1).max(0.0);
    let inter = iw * ih;

    let area_a = a.w * a.h;
    let area_b = b.w * b.h;
    let union = area_a + area_b - inter;

    if union <= 0.0 { 0.0 } else { inter / union }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coco_names_and_groups() {
        assert_eq!(COCO.len(), 80);
        assert_eq!(class_name(0), "person");
        assert_eq!(class_name(3), "motorcycle");
        assert_eq!(group_of("motorcycle"), "vehicle");
        assert_eq!(group_of("dog"), "dog");
    }
}
