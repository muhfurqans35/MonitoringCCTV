use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct NvrChannel {
    pub id: String,
    pub label: String,
    pub channel_number: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NvrConfig {
    pub ip: String,
    pub username: String,
    pub password: String,
    pub rtsp_port: u16,
    /// Port DVRIP/"TCP Port" NVR (Network di menu NVR), dipakai playback & alarm. Default 34567.
    #[serde(default = "default_dvrip_port")]
    pub dvrip_port: u16,
    pub live_url_pattern: String,
    pub channels: Vec<NvrChannel>,
}

fn default_dvrip_port() -> u16 {
    34567
}

impl NvrConfig {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let content = fs::read_to_string(path.as_ref())
            .map_err(|e| anyhow::anyhow!("gagal baca config NVR di {:?}: {e}", path.as_ref()))?;
        let cfg: NvrConfig = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("gagal parse config NVR (cek syntax TOML): {e}"))?;
        Ok(cfg)
    }

    pub fn find_channel_by_number(&self, number: u32) -> Option<&NvrChannel> {
        self.channels.iter().find(|c| c.channel_number == number)
    }

    pub fn find_channel(&self, id: &str) -> Option<&NvrChannel> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn build_snapshot_url_for_channel_id(&self, id: &str) -> Option<String> {
        let channel = self.find_channel(id)?;
        Some(format!("http://{}/cgi-bin/snapshot.cgi?channel={}", self.ip, channel.channel_number))
    }

    pub fn build_live_url(&self, channel: &NvrChannel) -> String {
        self.live_url_pattern
            .replace("{username}", &self.username)
            .replace("{password}", &self.password)
            .replace("{ip}", &self.ip)
            .replace("{rtsp_port}", &self.rtsp_port.to_string())
            .replace("{channel}", &channel.channel_number.to_string())
            .replace("{stream}", "1")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_snapshot_url_for_channel_id_returns_valid_nvr_channel_url() {
        let nvr = NvrConfig {
            ip: "192.168.1.200".to_string(),
            username: "admin".to_string(),
            password: String::new(),
            rtsp_port: 554,
            dvrip_port: 34567,
            live_url_pattern: "rtsp://{ip}:{rtsp_port}/user={username}&password={password}&channel={channel}&stream=1.sdp?real_stream".to_string(),
            channels: vec![
                NvrChannel { id: "gate_left".to_string(), label: "Gate Left".to_string(), channel_number: 4 },
                NvrChannel { id: "kost".to_string(), label: "Kost".to_string(), channel_number: 2 },
            ],
        };

        assert_eq!(
            nvr.build_snapshot_url_for_channel_id("gate_left"),
            Some("http://192.168.1.200/cgi-bin/snapshot.cgi?channel=4".to_string())
        );
        assert_eq!(
            nvr.build_snapshot_url_for_channel_id("unknown"),
            None
        );
    }
}