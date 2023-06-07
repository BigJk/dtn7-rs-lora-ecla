use crate::bundlequeue::BundleQueue;
use crate::global::Event::LoRaPacketTx;
use crate::global::{get_channels, get_node_id, peer_names, CloseType, Event};
use crate::protocol;
use crate::protocol::packet::Content;
use crate::strategy::proto::generate_sends;
use crate::strategy::proto::BundleProtoMeta;
use crate::strategy::{local_peer_refresh, StrategyConfig};
use dtn7::cla::ecla;
use dtn7::cla::ecla::Packet;
use dtn7::client::ecla::Command;
use humantime::parse_duration;
use log::{debug, error, info};
use prost::Message;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use tokio::select;
use tokio::time::{interval, sleep};

pub async fn random_routing(config: StrategyConfig) {
    info!("random based routing started");

    // Settings
    let send_interval = parse_duration(config.get("SEND_INTERVAL").unwrap_or(&"10s".to_string())).expect("SEND_INTERVAL is in wrong format");
    let send_packets = i32::from_str(config.get("SEND_PACKETS").unwrap_or(&"2".to_string())).expect("SEND_PACKETS wasn't a parseable number");
    let min_score = i32::from_str(config.get("MIN_SCORE").unwrap_or(&"0".to_string())).expect("MIN_SCORE wasn't a parseable number");
    let max_score = i32::from_str(config.get("MAX_SCORE").unwrap_or(&"100".to_string())).expect("MAX_SCORE wasn't a parseable number");
    let max_peer_age = i32::from_str(config.get("MAX_PEER_AGE").unwrap_or(&"30".to_string())).expect("MAX_PEER_AGE wasn't a parseable number");
    let random_delay_max = u32::from_str(config.get("RANDOM_DELAY_MAX").unwrap_or(&"0".to_string())).expect("RANDOM_DELAY wasn't a parseable number");

    // State
    let mut send_queue: BundleQueue<BundleProtoMeta> = BundleQueue::new();
    let local_peer_handle = local_peer_refresh();
    let mut advertise_packet: Option<protocol::Packet> = None;

    // If a random delay is specified wait
    if random_delay_max > 0 {
        let wait = (1 + (rand::random::<u32>() % random_delay_max)) as u64;
        debug!("Waiting for: {}", wait);
        sleep(Duration::from_secs(wait)).await;
        debug!("Wait time over");
    }

    let (broadcast_sender, mut broadcast_receive) = get_channels();

    // Ticker
    let mut send_ticker = interval(send_interval);

    loop {
        let node_id = get_node_id();

        select! {
            // Sending ticker
            //
            // This will execute the sends each send_interval.
            _ = send_ticker.tick() => {
                let mut pre_send = 0;

                // Track the presence of a advertisement packet separately because we will send it even without the
                // peers present check.
                if advertise_packet.is_some() {
                    let packet = advertise_packet.take().unwrap();

                    let mut buf = Vec::new();
                    buf.reserve(packet.encoded_len());
                    packet.encode(&mut buf).unwrap();

                    broadcast_sender.send(LoRaPacketTx(buf));
                    pre_send += 1;
                }

                // Only send something if we seen any kind of peer in the last max_peer_age period.
                if peer_names(max_peer_age as u64).len() > 0 {
                    generate_sends(&mut send_queue, send_packets - pre_send, false).into_iter().for_each(|event: Event| {
                        if broadcast_sender.send(event).is_err() {
                            error!("error while sending packet to lora_tx channel");
                        }
                    });
                }
            }
            // Event Handling
            //
            // Here we handle the ECLA and LoRa events.
            res = broadcast_receive.recv() => {
                 if let Ok(msg) = res {
                    match msg {
                        Event::StrategyRecommendAdvertise(packet) => {
                            advertise_packet.replace(packet);
                        },
                        Event::ECLAPacketRx(Packet::ForwardData(packet)) => {
                            info!("got ForwardDataPacket from ECLA: {} -> {}", packet.src, packet.dst);

                            // Create packet
                            let proto_pack = protocol::Packet {
                                r#type: protocol::PacketType::TypeBundleForward.into(),
                                content: Some(Content::BundleForward(protocol::BundleForward {
                                    destination: packet.dst.clone(),
                                    sender: node_id.clone(),
                                    bundle_id: packet.bundle_id.clone(),
                                    bundle_data: packet.data.clone(),
                                    data: std::collections::HashMap::new(),
                                })),
                            };

                            // Send packet to send queue
                            let queued = send_queue
                                .push_buf(min_score + (rand::random::<i32>() % max_score), packet.data.clone(), Some(BundleProtoMeta::new(proto_pack)));

                            info!("self forward queue result: queued={} prio={} queue_len={}", queued.0, queued.1, send_queue.len());
                        },
                        Event::LoRaPacketParsedRx(packet) => {
                            let packet = packet.packet;
                            if let Some(Content::BundleForward(packet)) = packet.content {
                                // Pass bundle to DTN
                                if let Err(err) = broadcast_sender.send(Event::ECLAPacketTx(Command::SendPacket(ecla::Packet::ForwardData(ecla::ForwardData {
                                    src: "".to_string(),
                                    dst: "".to_string(),
                                    bundle_id: packet.bundle_id.clone(),
                                    data: packet.bundle_data.clone(),
                                })))) {
                                    error!("error while sending to command channel: {}", err)
                                }

                                // Create packet
                                let proto_pack = protocol::Packet {
                                    r#type: protocol::PacketType::TypeBundleForward.into(),
                                    content: Some(Content::BundleForward(protocol::BundleForward {
                                        destination: packet.destination,
                                        sender: node_id.clone(),
                                        bundle_id: packet.bundle_id.clone(),
                                        bundle_data: packet.bundle_data.clone(),
                                        data: HashMap::new(),
                                    })),
                                };

                                // Sending
                                let queued = send_queue.push_buf(min_score + (rand::random::<i32>() % max_score), packet.bundle_data, Some(BundleProtoMeta::new(proto_pack)));

                                info!("forward queue result: queued={} prio={} queue_len={}", queued.0, queued.1, send_queue.len());
                            }
                        }
                        Event::Close(target) => match target {
                            CloseType::All | CloseType::Strategy => {
                                break;
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
        }
    }

    local_peer_handle.abort();

    info!("random based routing shut down");
}
