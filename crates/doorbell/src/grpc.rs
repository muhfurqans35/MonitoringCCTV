//! gRPC (tonic) untuk aksi dari dashboard (lewat service api).

use std::sync::Arc;

use proto::doorbell::doorbell_service_server::DoorbellService;
use proto::doorbell::{
    ActionResponse, CalibrateRequest, ClosePackageRequest, CreateInviteRequest, CreateInviteResponse,
    SetModeRequest, SnapshotRequest, SnapshotResponse, StatusRequest, StatusResponse,
};
use tonic::{Request, Response, Status};

use crate::hub::{Actor, Hub};

pub struct DoorbellServiceImpl {
    pub hub: Arc<Hub>,
}

#[tonic::async_trait]
impl DoorbellService for DoorbellServiceImpl {
    async fn get_status(&self, _r: Request<StatusRequest>) -> Result<Response<StatusResponse>, Status> {
        let s = self.hub.status().await;
        let mut modes: Vec<String> = self.hub.cfg.texts.keys().cloned().collect();
        modes.sort();
        Ok(Response::new(StatusResponse {
            door_id: s.door_id,
            mode: s.mode,
            node_online: s.node_online,
            node_ip: s.node_ip,
            node_cfg_ver: s.node_cfg_ver,
            hub_cfg_ver: s.hub_cfg_ver,
            calibrated: s.calibrated,
            waiting: s.waiting,
            verifying: s.verifying,
            modes,
            camera_ok: s.camera_ok,
            camera_sensor: s.camera_sensor,
            camera_error: s.camera_error,
            stream_url: s.stream_url,
        }))
    }

    async fn set_mode(&self, r: Request<SetModeRequest>) -> Result<Response<ActionResponse>, Status> {
        let mode = r.into_inner().mode;
        let ok = self.hub.set_mode(&mode).await;
        Ok(Response::new(ActionResponse {
            ok,
            message: if ok { format!("Mode diganti ke '{mode}'.") } else { format!("Mode '{mode}' tidak dikenal.") },
        }))
    }

    async fn create_invite(&self, r: Request<CreateInviteRequest>) -> Result<Response<CreateInviteResponse>, Status> {
        let room = r.into_inner().room_code;
        let ttl = self.hub.cfg.retention.invite_ttl_h as i32;
        Ok(Response::new(match self.hub.create_invite(&room).await {
            Some(code) => CreateInviteResponse { ok: true, code, message: String::new(), ttl_hours: ttl },
            None => CreateInviteResponse { ok: false, code: String::new(), message: format!("Kamar '{room}' tidak ada."), ttl_hours: ttl },
        }))
    }

    async fn close_package(&self, r: Request<ClosePackageRequest>) -> Result<Response<ActionResponse>, Status> {
        let (ok, message) = self.hub.on_taken(r.into_inner().id, Actor::Dashboard).await;
        Ok(Response::new(ActionResponse { ok, message }))
    }

    async fn calibrate(&self, _r: Request<CalibrateRequest>) -> Result<Response<ActionResponse>, Status> {
        let (ok, message) = self.hub.calibrate().await;
        Ok(Response::new(ActionResponse { ok, message }))
    }

    async fn snapshot(&self, _r: Request<SnapshotRequest>) -> Result<Response<SnapshotResponse>, Status> {
        Ok(Response::new(match self.hub.snapshot().await {
            Ok((jpeg, at)) => SnapshotResponse { ok: true, jpeg, message: String::new(), captured_at_ms: (at * 1000.0) as i64 },
            Err(message) => SnapshotResponse { ok: false, jpeg: Vec::new(), message, captured_at_ms: 0 },
        }))
    }
}
