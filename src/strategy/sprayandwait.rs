use crate::global::{get_channels, get_node_id, has_seen_peer, CloseType, Event, pass_to_dtnd, PEERS, peer_names};
use crate::bundlequeue::{Allower, BundleQueue, Scorer};
use crate::protocol;
use crate::protocol::packet::Content;
use crate::strategy::proto::BundleProtoMeta;
use crate::strategy::{local_peer_refresh, StrategyConfig};
use dtn7::cla::ecla::Packet;
use humantime::parse_duration;
use log::{error, info, debug};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use bp7::Bundle;
use bp7::helpers::unix_timestamp;
use parking_lot::Mutex;
use tokio::select;
use tokio::time::{interval, sleep};

#[derive(Debug)]
pub struct SaWBundleData {
    /// the number of copies we have left to spread
    remaining_copies: usize,
    /// the bundle to send
    bundle: Bundle,
    /// the list of nodes that have already received the bundle
    nodes: Vec<String>,
}

pub struct SaWAllowScore {
    max_peer_age: i32,
    history: Arc<Mutex<HashMap<String, SaWBundleData>>>,
}

impl SaWAllowScore {
    pub fn new(max_peer_age: i32, history: Arc<Mutex<HashMap<String, SaWBundleData>>>) -> SaWAllowScore {
        SaWAllowScore { max_peer_age, history }
    }
}

impl Allower<BundleProtoMeta> for SaWAllowScore {
    fn allow(&self, bndl: &mut Option<Bundle>, _: &mut Option<BundleProtoMeta>) -> bool {
        if let Some(bndl) = bndl {
            let mut history = self.history.lock();
            if let Some(his) = history.get_mut(bndl.id().as_str()) {
                // no remaining copies means no sending!
                if his.remaining_copies == 0 {
                    return false;
                }

                // see how many peers are available that didn't receive this yet
                let mut new_peers_available = 0;
                let peers = PEERS.lock();
                for peer in peers.iter() {
                    if peer.1.last_seen_secs() > self.max_peer_age as u64 || his.nodes.contains(peer.0) {
                        continue;
                    }

                    new_peers_available += 1;
                }

                // only allow if at least one new node is there
                return new_peers_available > 0;
            }
        }

        // allow packets that don't correspond to a bundle => advertise
        true
    }
}

impl Scorer<BundleProtoMeta> for SaWAllowScore {
    fn score(&self, _: &mut Option<Bundle>, meta: &mut Option<BundleProtoMeta>) -> i32 {
        let age = ((unix_timestamp() - meta.clone().unwrap().queued) / 30) as i32;

        // TODO: maybe also sort by number of new nodes this packet could reach

        age
    }
}


fn handle_new_bundle(history: &mut HashMap<String, SaWBundleData>, bundle: Bundle, bundle_id: String, max_copies: usize) {
    // Don't re-create if we already have the bundle
    if history.get(bundle_id.as_str()).is_some() {
        return;
    }

    if bundle_id.starts_with(get_node_id().as_str()) {
        // If it's our local bundle we give it max copies
        let meta = SaWBundleData {
            remaining_copies: max_copies,
            bundle,
            nodes: Vec::new(),
        };

        debug!("Adding new bundle {} from this host", &bundle_id);
        history.insert(bundle_id, meta);
    } else {
        // If it's a foreign bundle we give it 1 copy
        let meta = SaWBundleData {
            remaining_copies: 1,
            bundle,
            nodes: Vec::new(),
        };

        debug!("Adding new bundle {}", &bundle_id);
        history.insert(bundle_id, meta);
    }
}

// If we receive a bundle we already have we treat it like a acknowledgement and reduce the remaining copies.
fn handle_received(history: &mut HashMap<String, SaWBundleData>, bundle_id: String, node_id: String) {
    if let Some(meta) = history.get_mut(bundle_id.as_str()) {
        if meta.nodes.contains(&node_id) {
            return
        }

        meta.nodes.push(node_id);
        meta.remaining_copies -= 1;

        debug!("Bundle received {} reducing from {} to {}", &bundle_id, meta.remaining_copies + 1, meta.remaining_copies)
    }
}

pub async fn spray_and_wait_routing(config: StrategyConfig) {
    info!("spray and wait based routing started");

    // Settings
    let send_interval = parse_duration(config.get("SEND_INTERVAL").unwrap_or(&"10s".to_string())).expect("SEND_INTERVAL is in wrong format");
    let send_packets = i32::from_str(config.get("SEND_PACKETS").unwrap_or(&"2".to_string())).expect("SEND_PACKETS wasn't a parseable number");
    let max_copies = usize::from_str(config.get("MAX_COPIES").unwrap_or(&"7".to_string())).expect("MAX_COPIES wasn't a parseable number");
    let max_peer_age = i32::from_str(config.get("MAX_PEER_AGE").unwrap_or(&"30".to_string())).expect("MAX_PEER_AGE wasn't a parseable number");
    let random_delay_max = u32::from_str(config.get("RANDOM_DELAY_MAX").unwrap_or(&"0".to_string())).expect("RANDOM_DELAY wasn't a parseable number");
    let advertise_priority = i32::from_str(config.get("ADVERTISE_PRIORITY").unwrap_or(&"0".to_string())).expect("SELF_ADVERTISE_PRIORITY wasn't a parseable number");

    // State
    let mut send_queue: BundleQueue<BundleProtoMeta> = BundleQueue::new();
    let history: Arc<Mutex<HashMap<String, SaWBundleData>>> = Arc::new(Mutex::new(HashMap::new()));
    let local_peer_handle = local_peer_refresh();

    // If a random delay is specified wait
    if random_delay_max > 0 {
        let wait = (1 + (rand::random::<u32>() % random_delay_max)) as u64;
        debug!("Waiting for: {}", wait);
        sleep(Duration::from_secs(wait)).await;
        debug!("Wait time over");
    }

    let (broadcast_sender, mut broadcast_receive) = get_channels();

    // Add scorer and allower
    send_queue.add_allower(Box::new(SaWAllowScore::new(max_peer_age, history.clone())));
    send_queue.add_scorer(Box::new(SaWAllowScore::new(max_peer_age, history.clone())));

    // Ticker
    let mut send_ticker = interval(send_interval);

    loop {
        let node_id = get_node_id();

        select! {
                    // Sending ticker
                    //
                    // This will execute the sends each send_interval.
                    _ = send_ticker.tick() => {
                        debug!("send interval triggered!");

                        // Collect all sendable packets
                        let mut packets : Vec<(Bundle, BundleProtoMeta)> = vec!();
                        {
                            for kv in history.lock().iter() {
                                if kv.1.remaining_copies > 0 {
                                    // Create packet
                                    let proto_pack = protocol::Packet {
                                        r#type: protocol::PacketType::TypeBundleForward.into(),
                                        content: Some(Content::BundleForward(protocol::BundleForward {
                                            destination: kv.1.bundle.primary.destination.node_id().unwrap(),
                                            sender: node_id.clone(),
                                            bundle_id: kv.0.clone(),
                                            bundle_data: kv.1.bundle.clone().to_cbor(),
                                            data: std::collections::HashMap::new(),
                                        })),
                                    };

                                    debug!("Found potential bundle to send {}", kv.1.bundle.id().as_str());

                                    packets.push((kv.1.bundle.clone(), BundleProtoMeta::new(proto_pack)))
                                }
                            }
                        }

                        // Push them to the queue
                        packets.into_iter().for_each(|p| {
                            send_queue.push(0, Some(p.0), Some(p.1));
                        });

                        debug!("executing re-score! before_len={}", send_queue.len());

                        // Run re-score
                        send_queue.rescore();

                        // Print current queue state
                        debug!("queue state:\n{}", send_queue.print_state());

                        let mut events: Vec<Event> = vec![];
                        for _ in 0..send_packets {
                            let packet = send_queue.pop();
                            if let Some(packet) = packet {
                                debug!(
                                        "creating LoRaPacketTx event of len={} ({} left)",
                                        packet.1.clone().unwrap().proto_packet.len(),
                                        send_queue.len()
                                    );

                                if packet.0.is_none() {
                                    events.push(Event::LoRaPacketTx(packet.1.unwrap().proto_packet));
                                    continue;
                                }

                                // Check if history exists
                                let bndl_id = packet.0.clone().unwrap().id();
                                let bndl_destination = packet.0.clone().unwrap().primary.destination.node_id().unwrap();
                                let mut history = history.lock();
                                if let Some(his) = history.get_mut(bndl_id.as_str()) {
                                    if his.remaining_copies < 2 {
                                        // we are done with this bundle, only direct delivery remains
                                        if has_seen_peer(bndl_destination.as_str(), max_peer_age as u64) {
                                             debug!(
                                                "Attempting direct delivery of bundle {} to {} (remain={})", bndl_id.as_str(), bndl_destination.as_str(), his.remaining_copies - 1
                                             );

                                            his.remaining_copies -= 1;
                                            peer_names(max_peer_age as u64).iter().for_each(|name| {
                                                if !his.nodes.contains(name) {
                                                    his.nodes.push(name.to_string());
                                                }
                                            });
                                            events.push(Event::LoRaPacketTx(packet.1.unwrap().proto_packet));
                                        }
                                    } else if his.remaining_copies >= 2 {
                                        debug!(
                                            "Attempting sending of bundle {} (remain={})", bndl_id.as_str(), his.remaining_copies - 1
                                        );

                                        his.remaining_copies -= 1;
                                        peer_names(max_peer_age as u64).iter().for_each(|name| {
                                            if !his.nodes.contains(name) {
                                                his.nodes.push(name.to_string());
                                            }
                                        });
                                        events.push(Event::LoRaPacketTx(packet.1.unwrap().proto_packet));
                                    } else {
                                        debug!(
                                            "No copies left for {} {:#?}", bndl_id.as_str(), &his
                                        );
                                    }
                                }
                            }
                        }

                        events.into_iter().for_each(|event: Event| {
                            if broadcast_sender.send(event).is_err() {
                                error!("error while sending packet to lora_tx channel");
                            }
                        });
                    }
                    // Event Handling
                    //
                    // Here we handle the ECLA and LoRa events.
                    res = broadcast_receive.recv() => {
                         if let Ok(msg) = res {
                            match msg {
                                Event::ECLAPacketRx(Packet::ForwardData(packet)) => {
                                    info!("got ForwardDataPacket from ECLA: {} ", &packet.bundle_id);
                                    let mut history = history.lock();
                                    handle_new_bundle(&mut history, bp7::bundle::Bundle::try_from(packet.data).unwrap(), packet.bundle_id.clone(), max_copies);
                                },
                                Event::StrategyRecommendAdvertise(packet) => {
                                    send_queue.push(advertise_priority, None, Some(BundleProtoMeta::new(packet)));
                                }
                                Event::LoRaPacketParsedRx(packet) => {
                                    let packet = packet.packet;

                                    if let Some(packet) = packet.content {
                                        if let Content::BundleForward(packet) = packet {
                                            pass_to_dtnd(packet.clone())
                                        }
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
