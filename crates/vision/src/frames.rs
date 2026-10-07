//! Ambil 1 frame JPEG dari go2rtc (`/api/frame.jpeg?src=<stream>`). go2rtc mengambil keyframe dari
//! stream RTSP NVR yang sama dengan live view dashboard, lalu mengubahnya ke JPEG lewat ffmpeg.
//! Tidak bergantung pada snapshot.cgi (tidak ada di NVR XMEye) maupun protokol login NVR.

use std::time::Duration;

pub async fn fetch_frame(client: &reqwest::Client, go2rtc_url: &str, src: &str) -> anyhow::Result<Vec<u8>> {
    let url = format!("{}/api/frame.jpeg?src={src}", go2rtc_url.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("go2rtc tidak terjangkau ({url}): {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "go2rtc HTTP {status} untuk {src}: {} (stream ada di go2rtc.yaml? ffmpeg terpasang?)",
            body.chars().take(120).collect::<String>()
        );
    }
    let bytes = resp.bytes().await?;
    if bytes.len() < 1024 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        anyhow::bail!("go2rtc mengembalikan {} byte yang bukan JPEG untuk {src}", bytes.len());
    }
    Ok(bytes.to_vec())
}
