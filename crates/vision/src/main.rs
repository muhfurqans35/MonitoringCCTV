//! Service vision: pantau kamera CCTV NVR lewat go2rtc, deteksi gerak, (opsional) deteksi objek
//! dengan model ONNX ringan, simpan snapshot + hasil deteksi, dan kirim event ke MQTT.
//!
//!   nvr.toml [[channels]] -> go2rtc /api/frame.jpeg -> deteksi gerak (murah, tiap frame)
//!        -> ada gerakan -> model ONNX (mahal, hanya saat perlu) -> simpan JPEG+JSON -> MQTT event
//!
//! Konfigurasi: `NVR_CONFIG` (default config/nvr.toml), bagian `[vision]` opsional.

mod alarm;
mod frames;
mod inference;
mod motion;
mod publisher;
mod settings;
mod sink;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::nvr_config::NvrChannel;
use inference::{Detection, Detector};
use motion::MotionDetector;
use rumqttc::{AsyncClient, MqttOptions};
use alarm::Trigger;
use settings::VisionSettings;
use tokio::sync::Semaphore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config_path = std::env::var("NVR_CONFIG").unwrap_or_else(|_| "config/nvr.toml".to_string());
    let loaded = settings::load(&config_path)?;
    let cfg = Arc::new(loaded.settings);
    tracing::info!(
        "vision mulai: {} kamera dari {config_path} ({}), frame tiap {} ms lewat {}",
        loaded.channels.len(),
        loaded.channels.iter().map(|c| c.id.as_str()).collect::<Vec<_>>().join(", "),
        cfg.interval_ms,
        cfg.go2rtc_url
    );

    // Model opsional: tanpa file model, service tetap jalan dan melaporkan "motion".
    let detector: Option<Arc<Detector>> = if std::path::Path::new(&cfg.model.path).exists() {
        match Detector::load(&cfg.model.path, cfg.model.input_size, cfg.model.confidence, &cfg.model.classes) {
            Ok(d) => {
                tracing::info!(
                    "model {} dimuat (input {}, kelas: {})",
                    cfg.model.path,
                    cfg.model.input_size,
                    cfg.model.classes.join(", ")
                );
                Some(Arc::new(d))
            }
            Err(e) => {
                tracing::error!("model {} gagal dimuat: {e} — lanjut TANPA deteksi objek", cfg.model.path);
                None
            }
        }
    } else {
        tracing::warn!("file model {} tidak ada — lanjut TANPA deteksi objek", cfg.model.path);
        None
    };
    if detector.is_none() && cfg.only_objects {
        tracing::error!(
            "only_objects = true tetapi model tidak tersedia: TIDAK ADA event yang akan dikirim. \
             Salin model ONNX ke {} (lihat docs/VISION.md), atau set [vision] only_objects = false \
             untuk melaporkan semua gerakan.",
            cfg.model.path
        );
    }

    // --- MQTT ---
    let mqtt_host = std::env::var("MQTT_BROKER_HOST").unwrap_or_else(|_| "localhost".to_string());
    let mqtt_port: u16 = std::env::var("MQTT_BROKER_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(1883);
    let mut opts = MqttOptions::new("smartkost-vision", mqtt_host, mqtt_port);
    opts.set_keep_alive(Duration::from_secs(30));
    let user = std::env::var("MQTT_USERNAME").unwrap_or_default();
    if !user.is_empty() {
        opts.set_credentials(user, std::env::var("MQTT_PASSWORD").unwrap_or_default());
    }
    let (mqtt, mut eventloop) = AsyncClient::new(opts, 64);
    tokio::spawn(async move {
        loop {
            if let Err(e) = eventloop.poll().await {
                tracing::warn!("mqtt (vision) error: {e}, coba lagi 3 detik");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    });

    // --- Retensi snapshot (sekali sehari) ---
    let root = sink::root(&cfg.snapshot_dir);
    std::fs::create_dir_all(&root).map_err(|e| anyhow::anyhow!("gagal membuat {}: {e}", root.display()))?;
    {
        let (root, keep) = (root.clone(), cfg.keep_days);
        tokio::spawn(async move {
            loop {
                let n = tokio::task::spawn_blocking({
                    let root = root.clone();
                    move || sink::purge(&root, keep)
                })
                .await
                .unwrap_or(0);
                if n > 0 {
                    tracing::info!("retensi: {n} folder snapshot lebih tua dari {keep} hari dihapus");
                }
                tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            }
        });
    }

    // --- Pemicu per kamera + alarm NVR ---
    let triggers: HashMap<u32, (String, Arc<Trigger>)> = loaded
        .channels
        .iter()
        .map(|c| (c.channel_number, (c.id.clone(), Arc::new(Trigger::default()))))
        .collect();
    if cfg.nvr_alarm {
        alarm::spawn(loaded.nvr.clone(), triggers.clone(), mqtt.clone(), Duration::from_secs(cfg.alarm_boost_seconds));
    } else {
        tracing::info!("alarm NVR dimatikan ([vision] nvr_alarm = false)");
    }

    // --- Satu task per kamera ---
    let http = reqwest::Client::builder().pool_max_idle_per_host(8).build()?;
    let limiter = Arc::new(Semaphore::new(cfg.model.max_concurrent.max(1)));
    let mut handles = Vec::new();
    for (i, ch) in loaded.channels.into_iter().enumerate() {
        let trigger = triggers.get(&ch.channel_number).map(|(_, t)| t.clone()).unwrap_or_default();
        let ctx = CameraCtx {
            trigger,
            channel: ch,
            cfg: cfg.clone(),
            detector: detector.clone(),
            limiter: limiter.clone(),
            mqtt: mqtt.clone(),
            http: http.clone(),
            root: root.clone(),
        };
        // Sebar awal pengambilan agar ffmpeg di go2rtc tidak jalan bersamaan untuk semua kamera.
        let stagger = Duration::from_millis(cfg.interval_ms * i as u64 / 4);
        handles.push(tokio::spawn(async move {
            tokio::time::sleep(stagger).await;
            watch(ctx).await;
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    Ok(())
}

struct CameraCtx {
    trigger: Arc<Trigger>,
    channel: NvrChannel,
    cfg: Arc<VisionSettings>,
    detector: Option<Arc<Detector>>,
    limiter: Arc<Semaphore>,
    mqtt: AsyncClient,
    http: reqwest::Client,
    root: PathBuf,
}

async fn watch(ctx: CameraCtx) {
    let id = ctx.channel.id.clone();
    let roi = ctx.cfg.motion.roi.get(&id).copied();
    let mut motion = MotionDetector::new(ctx.cfg.motion.clone(), roi);
    let interval = Duration::from_millis(ctx.cfg.interval_ms.max(250));
    let fast = Duration::from_millis(ctx.cfg.alarm_interval_ms.max(250));
    let cooldown = Duration::from_secs(ctx.cfg.cooldown_seconds);
    let mut last_event: Option<Instant> = None;
    let mut fails: u32 = 0;
    let mut online: Option<bool> = None;
    let mut last_status: Option<Instant> = None;
    let mut last_score = 0.0f32;
    let mut events: u64 = 0;
    tracing::info!("kamera {id} ({}): dipantau{}", ctx.channel.label, roi.map(|r| format!(", ROI {r:?}")).unwrap_or_default());

    loop {
        let started = Instant::now();
        // Alarm NVR sejak frame sebelumnya: analisis frame ini dengan model walau deteksi gerak
        // sendiri belum terkonfirmasi (consecutive) -> tidak kehilangan detik-detik awal.
        let forced = ctx.trigger.pending.swap(false, Ordering::SeqCst);
        match frames::fetch_frame(&ctx.http, &ctx.cfg.go2rtc_url, &format!("{id}{}", ctx.cfg.stream_suffix)).await {
            Ok(jpeg) => {
                if online != Some(true) {
                    tracing::info!("kamera {id}: frame diterima ({} byte)", jpeg.len());
                }
                fails = 0;
                online = Some(true);

                // Decode + deteksi gerak di thread blocking (decode JPEG ~20-40 ms di Pi).
                let (m, jpeg) = match tokio::task::spawn_blocking(move || {
                    let r = motion.feed(&jpeg);
                    (motion, r, jpeg)
                })
                .await
                {
                    Ok((det, r, jpeg)) => {
                        motion = det;
                        (r, jpeg)
                    }
                    Err(e) => {
                        tracing::error!("kamera {id}: task deteksi gerak panik: {e}");
                        motion = MotionDetector::new(ctx.cfg.motion.clone(), roi);
                        tokio::time::sleep(interval).await;
                        continue;
                    }
                };
                match m {
                    Ok(r) => {
                        last_score = r.score;
                        if r.lighting_change {
                            tracing::debug!("kamera {id}: perubahan cahaya besar ({:.2}), diabaikan", r.score);
                        }
                        let cooled = last_event.map(|t| t.elapsed() >= cooldown).unwrap_or(true);
                        let by_nvr = forced && !r.triggered && ctx.detector.is_some();
                        if (r.triggered || by_nvr) && cooled {
                            last_event = Some(Instant::now());
                            if by_nvr {
                                tracing::debug!("kamera {id}: dianalisis karena alarm motion NVR");
                            }
                            if handle_motion(&ctx, jpeg, r.score).await {
                                events += 1;
                            }
                        }
                    }
                    Err(e) => tracing::warn!("kamera {id}: {e}"),
                }
            }
            Err(e) => {
                fails += 1;
                // Log hanya saat mulai gagal dan tiap 30 kegagalan, supaya log tidak banjir.
                if fails == 1 || fails % 30 == 0 {
                    tracing::warn!("kamera {id}: gagal ambil frame ({fails}x): {e}");
                }
                if fails >= 3 {
                    online = Some(false);
                }
            }
        }

        // Status retained tiap 60 dtk (MQTT Explorer: kost/cam/<id>/vision).
        if last_status.map(|t| t.elapsed() >= Duration::from_secs(60)).unwrap_or(true) {
            last_status = Some(Instant::now());
            let st = serde_json::json!({
                "online": online.unwrap_or(false),
                "label": ctx.channel.label,
                "nvr_channel": ctx.channel.channel_number,
                "motion_score": last_score,
                "nvr_alarms": ctx.trigger.alarms.load(Ordering::Relaxed),
                "events_since_start": events,
                "model": ctx.detector.is_some(),
                "timestamp": chrono::Utc::now(),
            });
            let _ = publisher::publish_status(&ctx.mqtt, &id, &st).await;
        }

        let spent = started.elapsed();
        let step = if ctx.trigger.boosted() { fast } else { interval };
        let wait = if fails >= 3 { Duration::from_secs(10) } else { step.saturating_sub(spent) };
        // Bangun lebih awal bila NVR melapor gerakan di kamera ini.
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = ctx.trigger.notify.notified() => {}
        }
    }
}

/// Gerakan terdeteksi: jalankan model (bila ada), simpan snapshot, publish event.
/// Mengembalikan true bila event dikirim ke MQTT.
async fn handle_motion(ctx: &CameraCtx, jpeg: Vec<u8>, motion_score: f32) -> bool {
    let id = &ctx.channel.id;
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(&jpeg))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .unwrap_or((0, 0));

    let detections: Vec<Detection> = match &ctx.detector {
        Some(det) => {
            let _permit = ctx.limiter.acquire().await.ok();
            let det = det.clone();
            let img = jpeg.clone();
            let t0 = Instant::now();
            match tokio::task::spawn_blocking(move || det.detect(&img)).await {
                Ok(Ok(d)) => {
                    tracing::debug!("kamera {id}: inference {} ms, {} objek", t0.elapsed().as_millis(), d.len());
                    d
                }
                Ok(Err(e)) => {
                    tracing::warn!("kamera {id}: inference gagal: {e}");
                    Vec::new()
                }
                Err(e) => {
                    tracing::error!("kamera {id}: task inference panik: {e}");
                    Vec::new()
                }
            }
        }
        None => Vec::new(),
    };

    // Mode only_objects: gerakan tanpa orang/kendaraan dibuang (tidak disimpan, tidak dikirim).
    if ctx.cfg.only_objects && detections.is_empty() {
        tracing::debug!("kamera {id}: gerakan (skor {motion_score:.3}) tanpa objek target, diabaikan");
        return false;
    }

    // Label & keputusan publish.
    let top = detections.iter().max_by(|a, b| a.confidence.total_cmp(&b.confidence));
    let (label, confidence, publish) = match (top, ctx.detector.is_some()) {
        (Some(t), _) => (t.label.clone(), t.confidence, true),
        (None, false) => ("motion".to_string(), 1.0, true),
        (None, true) => ("motion".to_string(), 1.0, ctx.cfg.publish_motion_without_detection),
    };

    // Simpan snapshot (gerakan tanpa objek ikut disimpan bila save_empty_motion).
    let mut snapshot_rel: Option<String> = None;
    if !detections.is_empty() || (!ctx.cfg.only_objects && ctx.cfg.save_empty_motion) {
        let rec = sink::EventRecord {
            camera_id: id,
            camera_label: &ctx.channel.label,
            nvr_channel: ctx.channel.channel_number,
            timestamp: chrono::Utc::now(),
            label: &label,
            motion_score,
            detections: &detections,
            model: ctx.detector.as_ref().map(|_| ctx.cfg.model.path.as_str()),
            model_input_size: ctx.detector.as_ref().map(|_| ctx.cfg.model.input_size),
            image_width: w,
            image_height: h,
            published: publish,
        };
        match sink::save(&ctx.root, &rec, &jpeg) {
            Ok(p) => snapshot_rel = Some(p),
            Err(e) => tracing::warn!("kamera {id}: gagal simpan snapshot: {e}"),
        }
    }

    let objs = detections.iter().map(|d| format!("{} {:.2}", d.class, d.confidence)).collect::<Vec<_>>().join(", ");
    tracing::info!(
        "kamera {id}: GERAK (skor {motion_score:.3}) -> {label}{}{}",
        if objs.is_empty() { String::new() } else { format!(" [{objs}]") },
        snapshot_rel.as_deref().map(|p| format!(" -> {p}")).unwrap_or_default()
    );

    if !publish {
        return false;
    }
    if let Err(e) =
        publisher::publish_event(&ctx.mqtt, id, &label, confidence, snapshot_rel.as_deref(), &detections, motion_score).await
    {
        tracing::warn!("kamera {id}: gagal publish event: {e}");
        return false;
    }
    true
}
