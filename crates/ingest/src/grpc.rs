use futures::Stream;
use std::pin::Pin;
use tonic::{Request, Response, Status};

use proto::telemetry::telemetry_service_server::TelemetryService;
use proto::telemetry::{
    LiveSnapshotRequest, LiveSnapshotResponse, RoomTelemetry, StreamRoomRequest,
};
use proto::alert::alert_service_server::AlertService;
use proto::alert::{AlertNotification, StreamAlertsRequest};

use crate::state::SharedState;

pub struct TelemetryServiceImpl {
    pub state: SharedState,
}

#[tonic::async_trait]
impl TelemetryService for TelemetryServiceImpl {
    async fn get_live_snapshot(
        &self,
        _request: Request<LiveSnapshotRequest>,
    ) -> Result<Response<LiveSnapshotResponse>, Status> {
        let snapshot = self.state.snapshot.read().await;
        let rooms: Vec<RoomTelemetry> = snapshot.values().cloned().collect();
        Ok(Response::new(LiveSnapshotResponse { rooms }))
    }

    type StreamRoomTelemetryStream =
        Pin<Box<dyn Stream<Item = Result<RoomTelemetry, Status>> + Send + 'static>>;

    async fn stream_room_telemetry(
        &self,
        request: Request<StreamRoomRequest>,
    ) -> Result<Response<Self::StreamRoomTelemetryStream>, Status> {
        let room_code = request.into_inner().room_code;
        let mut rx = self.state.telemetry_tx.subscribe();

        let stream = async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(data) => {
                        if room_code.is_empty() || data.room_code == room_code {
                            yield Ok(data);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }
}

pub struct AlertServiceImpl {
    pub state: SharedState,
}

#[tonic::async_trait]
impl AlertService for AlertServiceImpl {
    type StreamAlertsStream =
        Pin<Box<dyn Stream<Item = Result<AlertNotification, Status>> + Send + 'static>>;

    async fn stream_alerts(
        &self,
        _request: Request<StreamAlertsRequest>,
    ) -> Result<Response<Self::StreamAlertsStream>, Status> {
        let mut rx = self.state.alert_tx.subscribe();

        let stream = async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(data) => yield Ok(data),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }
}
