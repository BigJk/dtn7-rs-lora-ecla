use crate::agents::{Command, LoRaAgent, RxPacket};
use crate::global::{get_channels, get_node_id, publish_position, CloseType, Event, LoRaConfig, LoRaParsedPacket, Peer, PEERS, record_airtime};
use crate::protocol::packet::Content;
use crate::protocol::PacketType::TypePingPong;
use crate::protocol::{LatLngPos, Packet, PingPongType};
use crate::{agents, protocol};
use bp7::helpers::unix_timestamp;
use lazy_static::lazy_static;
use log::{debug, error, info};
use parking_lot::Mutex;
use prost::Message;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use crate::airtime::AirtimeConfig;

lazy_static! {
    static ref LORA_RUNNING: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    static ref LORA_FIRST_START: Arc<Mutex<bool>> = Arc::new(Mutex::new(true));
}

async fn start_lora_agent(config: LoRaConfig) {
    // Wait for former instance to stop
    loop {
        let mut running = LORA_RUNNING.lock();
        if !*running {
            *running = true;
            break;
        }

        sleep(Duration::from_millis(100)).await;
    }

    tokio::spawn(async move {
        let (broadcast_send, mut broadcast_receive) = get_channels();

        let mut port: Box<dyn LoRaAgent + Send>;
        let (port_tx, mut port_rx) = mpsc::channel::<RxPacket>(100);

        // Create port
        match config.agent_type.as_str() {
            "serial" => {
                port = Box::new(agents::rf95modem::Rf95modem::new(config.agent_arg, port_tx).expect("could not create serial port"));
            }
            "websocket" => {
                let split = config.agent_arg.split(':').collect::<Vec<&str>>();

                port = Box::new(
                    agents::websocket::Websocket::new(split[2].to_string(), split[0].to_string(), u32::from_str(split[1]).expect("can't parse port"), port_tx)
                        .expect("could not create websocket"),
                );
            }
            _ => {
                panic!("unknown agent type")
            }
        }

        // Open port and check for error on first start
        let res = port.open().await;

        if res.is_err() {
            info!("LoRa port connection not possible");

            if *LORA_FIRST_START.lock() {
                panic!("could not connect to LoRa port on first start! Aborting...");
            }

            *LORA_RUNNING.lock() = false;
        } else {
            *LORA_FIRST_START.lock() = false;
        }

        // Wait for command channel to get ready
        while port.command_channel().is_none() {
            sleep(Duration::from_millis(100)).await;
        }

        // Message handler
        let command_channel = port.command_channel().unwrap();
        let reader_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    res = broadcast_receive.recv() => {
                        if let Ok(msg) = res {
                            match msg {
                                Event::LoRaStart(_, _)=> {
                                    let _ = command_channel.send(Command::Close());
                                    return;
                                }
                                Event::Close(target) => match target {
                                    CloseType::All | CloseType::LoRa => {
                                        let _ = command_channel.send(Command::Close());
                                        return;
                                    }
                                    _ => {}
                                }
                                Event::LoRaPacketTx(packet) => {
                                    debug!("Outbound LoRaPacketTx received of len {}", packet.len());
                                    if (command_channel.send(Command::Send(packet)).await).is_err() {
                                        error!("Error while sending to lora command channel!");
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    res = port_rx.recv() => {
                        if let Some(msg) = res {
                            let _ = broadcast_send.send(Event::LoRaPacketRx(msg));
                        }
                    }
                }
            }
        });

        info!("LoRa started");

        // Periodic position fetcher
        loop {
            if let Ok(latlng) = port.lat_lng().await {
                publish_position(protocol::advertise::Position::LatLng(LatLngPos {
                    lat: latlng.0 as f32,
                    lng: latlng.1 as f32,
                }));
                info!("Fetched new LatLng: {} {}", latlng.0, latlng.1);
            }

            if reader_handle.is_finished() {
                break;
            }

            sleep(Duration::from_secs(5)).await;
        }

        info!("LoRa shutdown finished");

        *LORA_RUNNING.lock() = false;
    });
}

pub async fn start_lora_actor() {
    let (send, mut receive) = get_channels();
    let mut latest_config: LoRaConfig = LoRaConfig::default();
    let mut airtime_config: AirtimeConfig = AirtimeConfig::default();

    info!("LoRa Actor started");

    loop {
        if let Ok(msg) = receive.recv().await {
            match msg {
                Event::LoRaStart(start, airtime) => {
                    info!("received LoRa start request. Starting...");

                    latest_config = start.clone();
                    airtime_config = airtime.clone();
                    tokio::spawn(start_lora_agent(start));
                }
                // Parse LoRa Packets
                //
                // Globally try to parse all the LoRa packets and convert it to parsed ones.
                Event::LoRaPacketRx(packet) => match LoRaParsedPacket::try_from(packet) {
                    Ok(packet) => {
                        let _ = send.send(Event::LoRaPacketParsedRx(packet));
                    }
                    Err(err) => {
                        error!("wasn't able to decode LoRa Protocol Buffer packet: {}", err);
                    }
                },
                Event::LoRaPacketTx(packet) => {
                    let airtime = airtime_config.time_total(packet.len() as i32);
                    info!("sending LoRa packet of len {} with airtime {}ms", packet.len(), airtime);
                    record_airtime(airtime);
                },
                // PingPong Handler
                //
                // Wait for ping messages that contain our node_id or are broadcast (e.g. no id) and answer with a pong.
                Event::LoRaPacketParsedRx(packet) => {
                    if let Some(packet) = packet.packet.content {
                        match packet {
                            Content::PingPong(packet) => {
                                // TODO: add sleep
                                if packet.r#type == PingPongType::TypePing as i32 && (packet.node_id.is_empty() || packet.node_id == get_node_id()) {
                                    info!("received ping from '{}'", packet.node_id.clone());

                                    let proto_pack = Packet {
                                        r#type: TypePingPong.into(),
                                        content: Some(Content::PingPong(protocol::PingPong {
                                            r#type: PingPongType::TypePong as i32,
                                            node_id: get_node_id(),
                                        })),
                                    };

                                    let _ = send.send(Event::LoRaPacketTx(proto_pack.encode_to_vec()));
                                }
                            }
                            Content::Advertise(packet) => {
                                info!("Peer advertise received: {}", &packet.node_name);
                                let mut peers = PEERS.lock();
                                if let Some(peer) = peers.get_mut(packet.node_name.as_str()) {
                                    peer.last_seen = unix_timestamp();
                                    peer.last_location = packet.position;
                                    peer.last_data = packet.data;
                                } else {
                                    peers.insert(
                                        packet.node_name.clone(),
                                        Peer {
                                            node_name: packet.node_name,
                                            meta: HashMap::new(),
                                            last_data: packet.data,
                                            last_seen: unix_timestamp(),
                                            last_location: packet.position,
                                        },
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Event::LoRaFetchConfig(resp) => {
                    if let Some(resp) = resp.lock().take() {
                        let _ = resp.send(latest_config.clone());
                    }
                }
                Event::Close(target) => match target {
                    CloseType::All => {
                        return;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}
