//! Penghubung MQTT (rumqttc). Topic mengikuti konvensi smart-kost: kost/door/{door_id}/...

use std::sync::Arc;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use tonic::async_trait;

use crate::hub::Hub;
use crate::traits::Publisher;

pub struct MqttPublisher(pub AsyncClient);

#[async_trait]
impl Publisher for MqttPublisher {
    async fn publish(&self, topic: &str, payload: &serde_json::Value, retain: bool) {
        let bytes = serde_json::to_vec(payload).unwrap_or_default();
        if let Err(e) = self.0.publish(topic, QoS::AtLeastOnce, retain, bytes).await {
            tracing::warn!("publish {topic} gagal: {e}");
        }
    }
}

pub fn build_client(host: &str, port: u16, username: &str, password: &str) -> (AsyncClient, EventLoop) {
    let mut opts = MqttOptions::new("smartkost-doorbell", host, port);
    opts.set_keep_alive(Duration::from_secs(30));
    if !username.is_empty() {
        opts.set_credentials(username, password);
    }
    AsyncClient::new(opts, 100)
}

/// Loop utama: subscribe tiap (re)connect, publish konfigurasi retained, dispatch pesan ke hub.
/// Tiap pesan diproses di task sendiri supaya capture kamera yang lambat tidak menahan event loop MQTT.
pub async fn run(mut eventloop: EventLoop, client: AsyncClient, hub: Arc<Hub>, door_id: String) {
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                tracing::info!("MQTT tersambung, subscribe kost/door/{door_id}/...");
                let (client, hub, door) = (client.clone(), hub.clone(), door_id.clone());
                tokio::spawn(async move {
                    let _ = client.subscribe(format!("kost/door/{door}/event/#"), QoS::AtLeastOnce).await;
                    let _ = client.subscribe(format!("kost/door/{door}/status"), QoS::AtLeastOnce).await;
                    hub.publish_config().await;
                });
            }
            Ok(Event::Incoming(Packet::Publish(p))) => {
                let hub = hub.clone();
                tokio::spawn(async move {
                    hub.handle_message(&p.topic, &p.payload).await;
                });
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!("MQTT eventloop error: {e}, coba lagi 3 detik");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}
