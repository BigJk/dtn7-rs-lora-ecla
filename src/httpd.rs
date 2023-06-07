use axum::{
    routing::{get, post},
    http::StatusCode,
    Json, Router,
};
use tokio::time::sleep;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;
use axum::extract::Path;
use log::{debug, info};
use prost::Message;
use tokio::select;
use tokio::time::timeout;
use crate::global::{build_request_response, CloseType, DTNDConfig, ECLAConfig, Event, get_channels, get_node_id, get_sender, LoRaConfig, StrategyConfig};
use crate::global::Event::{DTNDFetchConfig, ECLAFetchConfig, LoRaFetchConfig, StrategyFetchConfig};
use crate::protocol;
use crate::protocol::packet::Content;
use crate::protocol::{Packet, PingPongType};
use crate::protocol::PacketType::TypePingPong;

pub async fn start_httpd(port: u32) {
    let app = Router::new()
        .route("/", get(root))
        .route("/close/:target", get(trigger_close_actor))
        .route("/config/ecla", get(get_ecla_config))
        .route("/config/lora", get(get_lora_config))
        .route("/config/strategy", get(get_strategy_config))
        .route("/config/dtnd", get(get_dtnd_config))
        .route("/ping_pong", post(ping_pong));

    let addr = SocketAddr::from(([127, 0, 0, 1], port as u16));
    debug!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn root() -> &'static str {
    "LoRa ECLA Hello!"
}

async fn trigger_close_actor(Path(target): Path<String>) -> (StatusCode, &'static str) {
    if let Ok(target) = CloseType::from_str(target.as_str()) {
        let _ = get_sender().send(Event::Close(target));
        return (StatusCode::OK, "closing request queued");
    }
    (StatusCode::BAD_REQUEST, "unknown target type")
}

async fn get_ecla_config() -> (StatusCode, Json<Option<ECLAConfig>>) {
    let (resp, req) = build_request_response::<ECLAConfig>();
    let _ = get_sender().send(ECLAFetchConfig(req));

    if let Ok(Ok(res)) = timeout(Duration::from_secs(1), resp).await {
        return (StatusCode::OK, Json(Some(res)));
    }

    (StatusCode::BAD_REQUEST, Json(None))
}

async fn get_lora_config() -> (StatusCode, Json<Option<LoRaConfig>>) {
    let (resp, req) = build_request_response::<LoRaConfig>();
    let _ = get_sender().send(LoRaFetchConfig(req));

    if let Ok(Ok(res)) = timeout(Duration::from_secs(1), resp).await {
        return (StatusCode::OK, Json(Some(res)));
    }

    (StatusCode::BAD_REQUEST, Json(None))
}

async fn get_strategy_config() -> (StatusCode, Json<Option<StrategyConfig>>) {
    let (resp, req) = build_request_response::<StrategyConfig>();
    let _ = get_sender().send(StrategyFetchConfig(req));

    if let Ok(Ok(res)) = timeout(Duration::from_secs(1), resp).await {
        return (StatusCode::OK, Json(Some(res)));
    }

    (StatusCode::BAD_REQUEST, Json(None))
}

async fn get_dtnd_config() -> (StatusCode, Json<Option<DTNDConfig>>) {
    let (resp, req) = build_request_response::<DTNDConfig>();
    let _ = get_sender().send(DTNDFetchConfig(req));

    if let Ok(Ok(res)) = timeout(Duration::from_secs(1), resp).await {
        return (StatusCode::OK, Json(Some(res)));
    }

    (StatusCode::BAD_REQUEST, Json(None))
}

#[derive(Serialize, Deserialize)]
pub struct PingPongRequest {
    pub node_id: String,
    pub timeout_ms: u32,
}

async fn ping_pong(Json(payload): Json<PingPongRequest>) -> Result<Json<Vec<String>>, StatusCode> {
    let mut res: Vec<String> = vec![];

    if payload.timeout_ms == 0 {
        return Err(StatusCode::BAD_REQUEST)
    }

    let (send, mut recv) = get_channels();

    // Send ping pong request
    let proto_pack = Packet {
        r#type: TypePingPong.into(),
        content: Some(Content::PingPong(protocol::PingPong {
            r#type: PingPongType::TypePing as i32,
            node_id: payload.node_id.clone(),
        })),
    };

    let _ = send.send(Event::LoRaPacketTx(proto_pack.encode_to_vec()));

    info!("sending PingPong for '{}'", payload.node_id);

    // Wait for responses
    let our_node_id = get_node_id();
    loop {
        select! {
            _ = sleep(Duration::from_millis(payload.timeout_ms as u64)) => {
                break; // timeout done
            }
            Ok(Event::LoRaPacketParsedRx(packet)) = recv.recv() => {
                if let Some(Content::PingPong(pong)) = packet.packet.content {
                    if pong.r#type != PingPongType::TypePong as i32 {
                        continue;
                    }

                    res.push(pong.node_id);
                    if payload.node_id == our_node_id {
                        break;
                    }
                }
            }
        }
    }

    Ok(Json(res))
}