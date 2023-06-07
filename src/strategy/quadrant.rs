use crate::global::{get_channels, get_position, get_sender, CloseType, Event, get_node_id, pass_to_dtnd};
use crate::bundlequeue::allower::{AllowBundleSize, AllowMetaCheck};
use crate::bundlequeue::scorer::{ScoreMetaCheck};
use crate::bundlequeue::BundleQueue;
use crate::protocol;
use crate::protocol::advertise::Position;
use crate::protocol::advertise::Position::NoPos;
use crate::protocol::packet::Content;
use crate::protocol::{BundleForward, LatLngPos};
use crate::strategy::proto::{BundleProtoMeta, generate_sends};
use crate::strategy::{local_peer_refresh, StrategyConfig};
use bp7::helpers::unix_timestamp;
use dtn7::cla::ecla::Packet;
use dtn7::client::ecla::{ForwardData};
use humantime::parse_duration;
use log::{debug, error, info};
use rand::prelude::StdRng;
use rand::{Rng, SeedableRng};
use std::any::Any;
use std::borrow::Borrow;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration};
use tokio::time::{interval, sleep};
use tokio::{select, task};
use tokio_util::sync::CancellationToken;

/// The key under which the bundle hash will be stored in the optional data inside the protobuf package.
const DATA_BUNDLE_HASH_KEY: &str = "BH";

/// The key under which the position will be stored in the optional data inside the protobuf package.
const DATA_POS_KEY: &str = "P";

/// Converts a position to a string representation.
fn pos_to_string(pos: Position) -> String {
    match pos {
        Position::LatLng(pos) => {
            format!("LL,{:.4},{:.4}", pos.lat, pos.lng)
        }
        Position::Xy(pos) => {
            format!("XY,{:.4},{:.4}", pos.x, pos.y)
        }
        Position::Generic(pos) => {
            format!("G,{}", pos.generic)
        }
        Position::NoPos(_) => "NP".to_string(),
    }
}

/// Converts a string encoded position back.
fn string_to_pos(pos: String) -> Position {
    let split = pos.split(",").collect::<Vec<&str>>();
    if split.is_empty() {
        return Position::NoPos(protocol::NoPos {});
    }

    match split[0] {
        "LL" => Position::LatLng(protocol::LatLngPos {
            lat: f32::from_str(split[1]).unwrap_or(0.0),
            lng: f32::from_str(split[2]).unwrap_or(0.0),
        }),
        "XY" => Position::Xy(protocol::XyPos {
            x: f32::from_str(split[1]).unwrap_or(0.0),
            y: f32::from_str(split[2]).unwrap_or(0.0),
        }),
        "G" => Position::Generic(protocol::GenericPos { generic: split[1].to_string() }),
        _ => Position::NoPos(protocol::NoPos {}),
    }
}

/// Calculates the quadrant relative to another position.
fn get_quadrant(self_pos: Position, other: Position) -> u32 {
    if self_pos.type_id() != other.type_id() {
        return 0;
    }

    match self_pos {
        // Calculate angle between the 2 positions and split it into 4 quadrants, each representing a 90° area.
        Position::LatLng(self_lat_lng) => {
            if let Position::LatLng(other_lat_lng) = other {
                let d_lng = other_lat_lng.lng - self_lat_lng.lng;

                let y = d_lng.sin() * other_lat_lng.lat.cos();
                let x = self_lat_lng.lat.cos() * other_lat_lng.lat.sin() - self_lat_lng.lat.sin() * other_lat_lng.lat.cos() * d_lng.cos();

                let angle = (y.atan2(x).to_degrees() + 360.0) % 360.0;

                return (angle / 90.0).ceil() as u32;
            }
        }
        Position::Xy(_) => {}
        Position::Generic(_) => {}
        Position::NoPos(_) => {}
    }

    0
}

/// Calculates the distance value between 2 quadrant values. It's either 1 or 2.
fn quad_distance(a: i32, b: i32) -> i32 {
    if a == 0 || b == 0 {
        return 1;
    }
    a.abs_diff(b) as i32 % 2
}

/// Contains information about a encountered neighbouring node.
struct RecentNeighbor {
    last_seen: u64,
    bundle_hash: String,
    pos: Position,
    bundles: BTreeMap<String, u64>,
}

impl RecentNeighbor {
    fn set_bundle_seen(&mut self, id: String, time: u64) {
        self.bundles.insert(id, time);
    }

    fn was_bundle_seen(&mut self, id: String, seconds: u64) -> bool {
        match self.bundles.get(id.as_str()) {
            None => false,
            Some(last_seen) => (unix_timestamp() - last_seen) < seconds,
        }
    }

    fn has_bundle(&mut self, id: String) -> bool {
        match self.bundles.get(id.as_str()) {
            None => false,
            Some(_) => true,
        }
    }
}

/// Contains the data of a bp7 bundle.
#[derive(Clone)]
struct BundleRecord {
    destination: String,
    bundle_id: String,
    bundle_data: Vec<u8>,
}

impl From<BundleForward> for BundleRecord {
    fn from(pack: BundleForward) -> Self {
        BundleRecord {
            bundle_data: pack.bundle_data,
            bundle_id: pack.bundle_id,
            destination: pack.destination,
        }
    }
}

impl From<ForwardData> for BundleRecord {
    fn from(pack: ForwardData) -> Self {
        BundleRecord {
            bundle_data: pack.data,
            bundle_id: pack.bundle_id,
            destination: pack.dst,
        }
    }
}

/// Contains the interval and priority settings for this strategy.
#[derive(Default, Debug, Clone)]
struct QuadrantSettings {
    /// The interval in which the strategy tries to send.
    send_interval: Duration,

    /// The interval in which the strategy checks for packets that should be spread again.
    spread_interval: Duration,

    /// The interval in which the strategy tries to advertise the node.
    advertise_interval: Duration,

    /// The amount of packets that should be sent if the send_interval is hit.
    send_packets: i32,

    /// The priority value that gets added for the spread.
    spread_priority: i32,

    /// The base priority of a packet that is received from the local DTND instance.
    self_forward_priority: i32,

    /// The base priority of the advertise packet.
    advertise_priority: i32,

    /// Maximum random delay to introduce.
    random_delay_max: u32,
}

struct QuadrantCore {
    cancel: CancellationToken,
    settings: QuadrantSettings,
    bundles: BTreeMap<String, (BundleRecord, u64)>,
    neighbors: BTreeMap<String, RecentNeighbor>,
    send_queue: BundleQueue<BundleProtoMeta>,
}

impl QuadrantCore {
    fn new(settings: QuadrantSettings) -> Self {
        let mut q = QuadrantCore {
            cancel: CancellationToken::new(),
            bundles: BTreeMap::new(),
            neighbors: BTreeMap::new(),
            send_queue: BundleQueue::new(),
            settings,
        };

        // add allower
        q.send_queue.add_allower(Box::new(AllowBundleSize::new(220)));
        q.send_queue
            .add_allower(Box::new(AllowMetaCheck::new(|meta: &mut BundleProtoMeta| -> bool {
                if meta.proto_packet.len() > 250 {
                    debug!("Size exceeded! len={}", meta.proto_packet.len())
                }

                meta.proto_packet.len() < 250
            })));

        q.send_queue.add_scorer(Box::new(ScoreMetaCheck::new(|meta: &mut BundleProtoMeta| -> i32 {
            ((unix_timestamp() - meta.queued) / 10) as i32
        })));

        // add scorer
        /*q.send_queue
            .add_scorer(Box::new(ScoreBundleSize::new(100, 0, -1)));
        q.send_queue.add_scorer(Box::new(ScoreEndpointService::new(
            [("emergency".to_string(), 100)].iter().cloned().collect(),
        )));*/

        q
    }

    /// Calculates the hash of the local bundle ids.
    fn bundles_hash(&mut self) -> String {
        let mut bundle_ids = self.bundles.clone().into_keys().collect::<Vec<String>>();
        bundle_ids.sort();

        let mut hasher = DefaultHasher::new();
        bundle_ids.join("").hash(&mut hasher);

        return format!("{:X}", hasher.finish());
    }

    fn set_neighbor(&mut self, name: String, data: RecentNeighbor) {
        if let Some(neighbor) = self.neighbors.get_mut(name.as_str()) {
            neighbor.bundle_hash = data.bundle_hash;
            neighbor.pos = data.pos;
            neighbor.last_seen = data.last_seen;
        } else {
            self.neighbors.insert(name, data);
        }
    }

    fn set_bundle_seen(&mut self, pack: BundleRecord, node_name: String, time: u64) {
        self.bundles.insert(pack.bundle_id.clone(), (pack.clone(), time));

        // Also record it to the per-node bundle list
        if node_name.is_empty() {
            return;
        }
        if let Some(neighbor) = self.neighbors.get_mut(node_name.as_str()) {
            neighbor.set_bundle_seen(pack.bundle_id.clone(), time);
        }
    }

    fn was_bundle_seen(&mut self, id: String, seconds: u64) -> bool {
        match self.bundles.get(id.as_str()) {
            None => false,
            Some(last_seen) => (unix_timestamp() - last_seen.1) < seconds,
        }
    }

    fn has_bundle(&mut self, id: String) -> bool {
        match self.bundles.get(id.as_str()) {
            None => false,
            Some(_) => true,
        }
    }

    /// Calculates the retransmission priority based on own position, sender position, bundle id and sender name.
    fn retransmission_priority(&mut self, self_pos: Position, sender_pos: Position, bndl_id: String, sender: String) -> i32 {
        let bundle_hash = self.bundles_hash();
        let mut prio: i32 = 0;
        let quad = get_quadrant(self_pos.clone(), sender_pos);

        for kv in self.neighbors.iter_mut() {
            // don't check self
            if *kv.0 == sender {
                continue;
            }

            // if bundle hash of neighbor is equal or we saw that bundle from this neighbor already we skip it
            if kv.1.bundle_hash == bundle_hash || kv.1.has_bundle(bndl_id.clone()) {
                continue;
            }

            // if sender quadrant != neighbor quadrant we add prio
            let nb_quad = get_quadrant(self_pos.clone(), kv.1.pos.clone());
            let dist = quad_distance(quad as i32, nb_quad as i32);
            if quad != nb_quad {
                prio += dist
            }
        }

        prio * 10
    }

    /// Returns a list of spreadable BundleRecord's together with the priority.
    fn spreadable_bundles(&mut self) -> Vec<(BundleRecord, i32)> {
        let bundle_hash = self.bundles_hash();
        let mut bundles_to_spread: HashMap<String, i32> = HashMap::new();

        for neighbor in self.neighbors.iter_mut() {
            // skip if neighbor already has the same bundles
            if neighbor.1.bundle_hash == bundle_hash {
                continue;
            }

            for bundle in self.bundles.iter() {
                // skip if we already saw that bundle from this neighbor
                if neighbor.1.has_bundle((*bundle.0).clone()) {
                    continue;
                }

                // increase spread
                bundles_to_spread.insert((*bundle.0).clone(), bundles_to_spread.get(bundle.0.as_str()).unwrap_or(&0) + self.settings.spread_priority);
            }
        }

        bundles_to_spread.iter().map(|kv| (self.bundles.get(kv.0.as_str()).unwrap().0.clone(), (*kv.1))).collect()
    }
}

/// waits for node_id to become populated
async fn wait_for_node_name(core: Arc<Mutex<QuadrantCore>>) {
    let mut node_id_ticker = interval(Duration::from_millis(500));
    loop {
        node_id_ticker.tick().await;

        if !get_node_id().is_empty() {
            break;
        }
    }
}

/// handles the sending from the packet priority queue to LoRa.
async fn handle_sending(core: Arc<Mutex<QuadrantCore>>) {
    let cancel = core.lock().unwrap().cancel.clone();
    let broadcast_sender = get_sender();

    let send_interval = core.lock().unwrap().settings.send_interval;
    let send_packets = core.lock().unwrap().settings.send_packets;
    let random_delay_max = core.lock().unwrap().settings.random_delay_max;

    wait_for_node_name(core.clone()).await;

    // use node id as seed
    let mut hasher = DefaultHasher::new();
    get_node_id().hash(&mut hasher);
    let hash = hasher.finish();

    // create a random start offset
    if random_delay_max > 0 {
        let mut rnd = StdRng::seed_from_u64(hash);
        let offset = 1 + rnd.gen_range(0..random_delay_max);

        debug!("sending timeframe offset is {} with hash {}", offset, hash);

        // wait the offset
        sleep(Duration::from_secs(offset as u64)).await;
    }

    let mut ticker = interval(send_interval);
    loop {
        select! {
            _ = ticker.tick() => {}
            _ = cancel.cancelled() => {
                return;
            }
        }

        /*

        // Random sleep to drift the sending timeframes further

        let extra_sleep = Duration::from_secs(rnd.gen_range(0..3));
        if !extra_sleep.is_zero() {
            debug!("adding additional timeframe offset {}", extra_sleep.as_secs());

            sleep(extra_sleep).await;
        }

        */

        info!("sending timeframe started! trying to send {} packets", send_packets);
        debug!("our bundle hash: {}", core.lock().unwrap().bundles_hash());

        generate_sends(&mut core.lock().unwrap().send_queue, send_packets, true).into_iter().for_each(|event: Event| {
            if broadcast_sender.send(event).is_err() {
                error!("error while sending packet to lora_tx channel");
            }
        });
    }
}

/// handles the spreading.
async fn handle_spreading(core: Arc<Mutex<QuadrantCore>>) {
    let cancel = core.lock().unwrap().cancel.clone();

    let spread_interval = core.lock().unwrap().settings.spread_interval;

    let mut ticker = interval(spread_interval);
    loop {
        select! {
            _ = ticker.tick() => {}
            _ = cancel.cancelled() => {
                return;
            }
        }

        {
            let mut core = core.lock().unwrap();

            // get spreadable bundles
            let spreadable = core.spreadable_bundles();

            info!("found {} packets to spread", spreadable.len());

            for bundle in spreadable {
                let proto_pack = protocol::Packet {
                    r#type: protocol::PacketType::TypeBundleForward.into(),
                    content: Some(Content::BundleForward(protocol::BundleForward {
                        destination: bundle.0.destination.clone(),
                        sender: get_node_id(),
                        bundle_id: bundle.0.bundle_id.clone(),
                        bundle_data: bundle.0.bundle_data.clone(),
                        data: HashMap::new(),
                    })),
                };

                // send bundles to send queue with given priority
                info!("trying to queue bndl={}", bundle.0.bundle_id);

                let queued = core.send_queue.push_buf(bundle.1, bundle.0.bundle_data, Some(BundleProtoMeta::new(proto_pack)));

                info!("spread queue result: queued={} prio={} queue_len={}", queued.0, queued.1, core.send_queue.len());
            }
        }
    }
}

/// handles the advertise.
async fn handle_advertise(core: Arc<Mutex<QuadrantCore>>) {
    let cancel = core.lock().unwrap().cancel.clone();

    let advertise_interval = core.lock().unwrap().settings.advertise_interval;
    let advertise_priority = core.lock().unwrap().settings.advertise_priority;

    wait_for_node_name(core.clone()).await;

    let mut ticker = interval(advertise_interval);
    loop {
        select! {
            _ = ticker.tick() => {}
            _ = cancel.cancelled() => {
                return;
            }
        }

        let pos = get_position();
        let hash = core.lock().unwrap().bundles_hash();

        // Add bundle hash to additional data
        let mut data = std::collections::HashMap::new();
        data.insert(DATA_BUNDLE_HASH_KEY.to_string(), hash.clone());
        data.insert(DATA_POS_KEY.to_string(), pos_to_string((pos.borrow()).clone()));

        let own_node_name = get_node_id();
        if own_node_name.is_empty() {
            error!("own node name is empty!");
            continue;
        }

        let pos = (*pos.borrow()).clone();
        debug!("advertise pos: {}", pos_to_string(pos.clone()));

        // Create packet
        let proto_pack = protocol::Packet {
            r#type: protocol::PacketType::TypeAdvertise.into(),
            content: Some(Content::Advertise(protocol::Advertise {
                node_name: own_node_name,
                position: Some(pos),
                data,
            })),
        };

        // Send packet to send queue
        info!("trying to queue advertise with hash={}", hash);

        let mut core = core.lock().unwrap();
        let queued = core.send_queue.push(advertise_priority, None, Some(BundleProtoMeta::new(proto_pack)));

        info!("advertise queue result: queued={} prio={} queue_len={}", queued.0, queued.1, core.send_queue.len());
    }
}

pub async fn quadrant_routing(config: StrategyConfig) {
    info!("quadrant based routing started");

    let settings = QuadrantSettings {
        send_interval: parse_duration(config.get("SEND_INTERVAL").unwrap_or(&"10s".to_string())).expect("SEND_INTERVAL is in wrong format"),
        spread_interval: parse_duration(config.get("SPREAD_INTERVAL").unwrap_or(&"35s".to_string())).expect("SPREAD_INTERVAL is in wrong format"),
        advertise_interval: parse_duration(config.get("ADVERTISE_INTERVAL").unwrap_or(&"60s".to_string())).expect("ADVERTISE_INTERVAL is in wrong format"),
        send_packets: i32::from_str(config.get("SEND_PACKETS").unwrap_or(&"2".to_string())).expect("SEND_PACKETS wasn't a parseable number"),
        spread_priority: i32::from_str(config.get("SPREAD_PRIORITY").unwrap_or(&"2".to_string())).expect("SPREAD_PRIORITY wasn't a parseable number"),
        self_forward_priority: i32::from_str(config.get("SELF_FORWARD_PRIORITY").unwrap_or(&"10".to_string())).expect("SELF_FORWARD_PRIORITY wasn't a parseable number"),
        advertise_priority: i32::from_str(config.get("ADVERTISE_PRIORITY").unwrap_or(&"0".to_string())).expect("SELF_ADVERTISE_PRIORITY wasn't a parseable number"),
         random_delay_max: u32::from_str(config.get("RANDOM_DELAY_MAX").unwrap_or(&"0".to_string())).expect("RANDOM_DELAY wasn't a parseable number")
    };

    let core: Arc<Mutex<QuadrantCore>> = Arc::new(Mutex::new(QuadrantCore::new(settings.clone())));

    debug!("quadrant args: {:#?}", core.lock().unwrap().settings);

    let (_, mut broadcast_receive) = get_channels();

    let local_peer_handle = local_peer_refresh();
    let advertise_handle = task::spawn(handle_advertise(core.clone()));
    let handle_spreading = task::spawn(handle_spreading(core.clone()));
    let sending_handle = task::spawn(handle_sending(core.clone()));

    loop {
        if let Ok(msg) = broadcast_receive.recv().await {
            match msg {
                Event::ECLAPacketRx(packet) => match packet {
                    // ForwardData
                    //
                    // This is the forward data request from the ECLA, which triggers if a new bundle should be sent.
                    // If the bundle was already passed in the last 60 seconds it will be ignored otherwise the bundle will instantly
                    // be submitted to the send queue with the SELF_FORWARD_PRIORITY priority.
                    Packet::ForwardData(packet) => {
                        info!("got ForwardDataPacket from ECLA: {} -> {}", packet.src, packet.dst);

                        // If we already have the bundle in our internal store we skip it
                        if core.lock().unwrap().has_bundle(packet.bundle_id.clone()) {
                            info!("skipping bundle from ecla: already in own store");
                            continue;
                        }

                        // Create packet
                        let proto_pack = protocol::Packet {
                            r#type: protocol::PacketType::TypeBundleForward.into(),
                            content: Some(Content::BundleForward(protocol::BundleForward {
                                destination: packet.dst.clone(),
                                sender: get_node_id(),
                                bundle_id: packet.bundle_id.clone(),
                                bundle_data: packet.data.clone(),
                                data: std::collections::HashMap::new(),
                            })),
                        };

                        // Send packet to send queue
                        let mut core = core.lock().unwrap();
                        let queued = core
                            .send_queue
                            .push_buf(settings.self_forward_priority, packet.data.clone(), Some(BundleProtoMeta::new(proto_pack)));

                        info!("self forward queue result: queued={} prio={} queue_len={}", queued.0, queued.1, core.send_queue.len());

                        // Set bundle seen and store data
                        core.set_bundle_seen(packet.clone().into(), "".to_string(), unix_timestamp());
                    }
                    _ => {}
                },
                Event::LoRaPacketParsedRx(packet) => {
                    let packet = packet.packet;
                    match packet.content {
                        // Advertise
                        //
                        // This is the protobuf advertise packet containing position and hash of bundle ids.
                        Some(Content::Advertise(packet)) => {
                            debug!("got Advertise from: {}", packet.node_name.clone());

                            match packet.position.clone().unwrap_or(Position::LatLng(LatLngPos { lat: 0.0, lng: 0.0 })) {
                                Position::LatLng(pos) => {
                                    info!("got Advertise with LatLng: {} (lat={}, lng={})", packet.node_name, pos.lat, pos.lng);
                                }
                                Position::Xy(_) => {}
                                Position::Generic(_) => {}
                                Position::NoPos(_) => {}
                            }

                            if packet.node_name.is_empty() {
                                error!("empty node name in advertise!");
                                continue;
                            }

                            let neighbor = RecentNeighbor {
                                pos: packet.position.unwrap_or(Position::NoPos(protocol::NoPos {})),
                                last_seen: unix_timestamp(),
                                bundle_hash: (*packet.data.get(DATA_BUNDLE_HASH_KEY).unwrap_or(&"".to_string())).clone(),
                                bundles: Default::default(),
                            };

                            debug!("bundle_hash {} from {}", &neighbor.bundle_hash, &packet.node_name);

                            core.lock().unwrap().set_neighbor(packet.node_name, neighbor);
                        }

                        // BundleForward
                        //
                        // This is the protobuf bundle forward packet that contains a bundle and additional information.
                        Some(Content::BundleForward(packet)) => {
                            pass_to_dtnd(packet.clone());

                            // Get sender pos or NoPos in case sender is not in recent neighbors
                            let sender_pos: Position = core
                                .lock()
                                .unwrap()
                                .neighbors
                                .get_mut(packet.sender.as_str())
                                .unwrap_or(&mut RecentNeighbor {
                                    last_seen: 0,
                                    bundle_hash: "".to_string(),
                                    pos: NoPos(protocol::NoPos {}),
                                    bundles: Default::default(),
                                })
                                .pos
                                .clone();

                            let quad = get_quadrant(get_position(), sender_pos.clone());

                            info!("got BundleForward packet from LoRa with sender={} quad={}", packet.sender.clone(), quad);

                            // Set seen
                            core.lock().unwrap().set_bundle_seen(packet.clone().into(), packet.sender.clone(), unix_timestamp());

                            let priority = core
                                .lock()
                                .unwrap()
                                .retransmission_priority(get_position(), sender_pos, packet.bundle_id.clone(), packet.sender.clone());

                            // Create packet
                            let proto_pack = protocol::Packet {
                                r#type: protocol::PacketType::TypeBundleForward.into(),
                                content: Some(Content::BundleForward(protocol::BundleForward {
                                    destination: packet.destination,
                                    sender: get_node_id(),
                                    bundle_id: packet.bundle_id.clone(),
                                    bundle_data: packet.bundle_data.clone(),
                                    data: HashMap::new(),
                                })),
                            };

                            // Sending
                            {
                                let mut core = core.lock().unwrap();

                                // Send packet to send queue
                                info!("trying to queue forward bndl={} with prio={}", packet.bundle_id, priority);

                                let queued = core.send_queue.push_buf(priority, packet.bundle_data, Some(BundleProtoMeta::new(proto_pack)));

                                info!("forward queue result: queued={} prio={} queue_len={}", queued.0, queued.1, core.send_queue.len());
                            }
                        }
                        _ => {}
                    }
                }
                Event::Close(target) => match target {
                    CloseType::All | CloseType::Strategy => {
                        core.lock().unwrap().cancel.cancel();
                        break;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    local_peer_handle.abort();

    let _ = handle_spreading.await;
    let _ = advertise_handle.await;
    let _ = sending_handle.await;

    info!("quadrant based routing shut down");
}
