use std::collections::HashMap;
use std::io::Cursor;
use crate::{protocol, strategy};
use dtn7::cla::ecla;
use lazy_static::lazy_static;
use std::sync::{Arc};
use bp7::helpers::unix_timestamp;
use dtn7::client::ecla::Command;
use log::error;
use parking_lot::Mutex;
use prost::Message;
use strum_macros::{IntoStaticStr, EnumString};
use tokio::sync::broadcast;
use serde::{Deserialize, Serialize};
use crate::agents::RxPacket;
use crate::airtime::AirtimeConfig;
use crate::protocol::advertise::Position;
use crate::protocol::{BundleForward, Packet};

/// Peer represents a peer that has been seen by the node.
#[derive(Clone, Default, Debug)]
pub struct Peer {
    pub node_name: String,
    pub meta: HashMap<String, String>,
    pub last_location: Option<Position>,
    pub last_data: HashMap<String, String>,
    pub last_seen: u64
}

impl Peer {
    pub fn last_seen_secs(&self) -> u64 {
        unix_timestamp() - self.last_seen
    }
}

/// Airtime represents the time a bundle was on the air.
pub struct Airtime {
    pub time: u64,
    pub length_ms: f64,
}

/// ECLA config for its start command.
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct ECLAConfig {
    /// Address of the ECLA interface.
    pub addr: String,
    /// CLA module name to register as (e.g. LoRa)
    pub module_name: String,
}

/// LoRa agent config for its start command.
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct LoRaConfig {
    /// What type of agent to start (e.g. WebSocket, Serial, etc.)
    pub agent_type: String,
    /// Connection argument for the agent.
    pub agent_arg: String,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct StrategyConfig {
    pub name: String,
    pub config: strategy::StrategyConfig
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct DTNDConfig {
    pub args: String
}

/// A parsed protocol buffer packet from a raw LoRa packet.
#[derive(Clone, Default)]
pub struct LoRaParsedPacket {
    pub rssi: i16,
    pub snr: i16,
    pub recv_time: u64,
    pub packet: protocol::Packet
}

impl TryFrom<RxPacket> for LoRaParsedPacket {
    type Error = &'static str;

    fn try_from(raw_packet: RxPacket) -> Result<Self, Self::Error> {
        match protocol::Packet::decode(&mut Cursor::new(raw_packet.data.clone().as_slice())) {
            Ok(res) => {
                Ok(LoRaParsedPacket{
                    rssi: raw_packet.rssi,
                    snr: raw_packet.snr,
                    recv_time: raw_packet.recv_time,
                    packet: res,
                })
            }
            Err(_) => {
                Err("could not decode")
            }
        }
    }
}

/// Specifies which part of the system to shutdown.
#[derive(Clone, Serialize, Deserialize, EnumString, Debug)]
pub enum CloseType {
    #[strum(ascii_case_insensitive)]
    All,
    #[strum(ascii_case_insensitive)]
    ECLA,
    #[strum(ascii_case_insensitive)]
    LoRa,
    #[strum(ascii_case_insensitive)]
    DTND,
    #[strum(ascii_case_insensitive)]
    Strategy,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum ActorType {
    ECLA,
    LoRa,
    DTND,
    Strategy,
}

/// Represents the response channel for a request-like Event.
type ResponseChannel<T> = Arc<Mutex<Option<tokio::sync::oneshot::Sender<T>>>>;

/// Events that can be send or received over the event bus.
#[derive(Clone, IntoStaticStr)]
pub enum Event {
    // ECLA Events
    ECLAStart(ECLAConfig),
    ECLAPacketRx(ecla::Packet),
    ECLAPacketTx(ecla::ws_client::Command),
    ECLAFetchConfig(ResponseChannel<ECLAConfig>),

    // LoRa Events
    LoRaStart(LoRaConfig, AirtimeConfig),
    LoRaPacketRx(RxPacket),
    LoRaPacketParsedRx(LoRaParsedPacket),
    LoRaPacketTx(Vec<u8>),
    LoRaFetchConfig(ResponseChannel<LoRaConfig>),

    // Strategy Events
    StrategyStart(StrategyConfig),
    StrategyStarted(),
    StrategyFetchConfig(ResponseChannel<StrategyConfig>),
    StrategyRecommendAdvertise(Packet),

    // DTND Events
    DTNDStart(DTNDConfig),
    DTNDStarted(),
    DTNDFetchConfig(ResponseChannel<DTNDConfig>),

    // Other
    Close(CloseType),
    Error((ActorType, String)),
}

pub fn build_request_response<T>() -> (tokio::sync::oneshot::Receiver<T>, ResponseChannel<T>) {
    let (sender, recv) = tokio::sync::oneshot::channel::<T>();
    (recv, Arc::new(Mutex::new(Some(sender))))
}

lazy_static! {
    /// Represents the global event bus.
    pub static ref BROADCAST: Arc<Mutex<(broadcast::Sender<Event>, broadcast::Receiver<Event>)>> =
        Arc::new(Mutex::new(broadcast::channel(100)));

    /// Represents the global state of the position.
    pub static ref POSITION: Arc<
        Mutex<(
            tokio::sync::watch::Sender<protocol::advertise::Position>,
            tokio::sync::watch::Receiver<protocol::advertise::Position>
        )>,
    > = Arc::new(Mutex::new(tokio::sync::watch::channel(
        protocol::advertise::Position::NoPos(protocol::NoPos {})
    )));

    /// Represents the global state of the node id.
    pub static ref NODE_ID: Arc<
        Mutex<(
            tokio::sync::watch::Sender<String>,
            tokio::sync::watch::Receiver<String>
        )>,
    > = Arc::new(Mutex::new(tokio::sync::watch::channel("".to_string())));

    /// Represents the global peer list.
    pub static ref PEERS: Arc<Mutex<HashMap<String, Peer>>> = Arc::new(Mutex::new(HashMap::new()));

    /// Represents the global airtime list.
    pub static ref AIRTIMES: Arc<Mutex<Vec<Airtime>>> = Arc::new(Mutex::new(Vec::new()));
}

/// Gets a sender instance for the event bus.
pub fn get_sender() -> broadcast::Sender<Event> {
    let locked = BROADCAST.lock();
    locked.0.clone()
}

/// Broadcasts a single event to the event bus.
pub fn broadcast_event(event: Event) {
    let _ = get_sender().send(event);
}

/// Gets a sender and receiver channel for the event bus. It's mandatory to read from the receiver or let it
/// go out of scope as soon as possible to keep the event bus from lagging!
pub fn get_channels() -> (broadcast::Sender<Event>, broadcast::Receiver<Event>) {
    let locked = BROADCAST.lock();
    (locked.0.clone(), locked.0.subscribe())
}

/// Publishes a new position to the global watch.
pub fn publish_position(position: protocol::advertise::Position) {
    let _ = POSITION.lock().0.send(position);
}

/// Fetches the latest position from the global watch.
pub fn get_position() -> protocol::advertise::Position {
    POSITION.lock().1.borrow().clone()
}

/// Publishes a new node id to the global watch.
pub fn publish_node_id(node_id: String) {
    let _ = NODE_ID.lock().0.send(node_id);
}

/// Fetches the latest node id from the global watch.
pub fn get_node_id() -> String {
    NODE_ID.lock().1.borrow().clone()
}

/// Pass a packet to the DTND.
pub fn pass_to_dtnd(packet: BundleForward) {
    if let Err(err) = get_sender().send(Event::ECLAPacketTx(Command::SendPacket(ecla::Packet::ForwardData(ecla::ForwardData {
        src: "".to_string(),
        dst: "".to_string(),
        bundle_id: packet.bundle_id,
        data: packet.bundle_data,
    })))) {
        error!("error while sending to command channel: {}", err)
    }
}

/// Check if a peer was seen.
pub fn has_seen_peer(id: &str, max_age: u64) -> bool {
    PEERS.lock().iter().any(|kv| -> bool {
        kv.0 == id && kv.1.last_seen_secs() < max_age
    })
}

/// Returns which peer names were seen.
pub fn peer_names(max_age: u64) -> Vec<String> {
    PEERS.lock().iter().filter(|kv| -> bool {
        kv.1.last_seen_secs() < max_age
    }).map(|kv| -> String {
        kv.0.clone()
    }).collect()
}

/// Records a new airtime.
pub fn record_airtime(len_ms: f64) {
    AIRTIMES.lock().push(Airtime {
        length_ms: len_ms,
        time: unix_timestamp(),
    });
}

/// Get the total airtime in milliseconds.
pub fn airtime_total() -> f64 {
    AIRTIMES.lock().iter().fold(0.0, |acc, x| -> f64 {
        acc + x.length_ms
    })
}

/// Fetches the total airtime over the last `secs` seconds.
pub fn airtime_last_secs(secs: u64) -> f64 {
    let until = unix_timestamp() - secs;
    let mut total = 0.0;

    // We know that the list is sorted as the clock runs forward.
    // We can iterate in reverse and stop as soon as we hit a time that is too old.
    let _ = AIRTIMES.lock().iter().rev().try_for_each(|x| -> Result<(), ()> {
        // Stop as soon as we hit a time that is too old.
        if x.time < until {
            return Err(());
        }

        // Otherwise sum up the airtime.
        total += x.length_ms;
        Ok(())
    });

    total
}