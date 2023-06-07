use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};
use bp7::helpers::unix_timestamp;
use log::debug;
use prost::Message;
use crate::global::Event;
use crate::bundlequeue::BundleQueue;
use crate::protocol;

#[derive(Clone, Eq)]
pub struct BundleProtoMeta {
    pub proto_type: protocol::PacketType,
    pub proto_packet: Vec<u8>,
    pub queued: u64,
}

impl BundleProtoMeta {
    pub fn new(packet: protocol::Packet) -> BundleProtoMeta {
        let mut buf = Vec::new();
        buf.reserve(packet.encoded_len());
        packet.encode(&mut buf).unwrap();

        BundleProtoMeta {
            proto_type: packet.r#type(),
            proto_packet: buf,
            queued: unix_timestamp()
        }
    }
}

impl PartialEq for BundleProtoMeta {
    fn eq(&self, other: &Self) -> bool {
        let mut hash_self = DefaultHasher::new();
        let mut hash_other = DefaultHasher::new();

        self.hash(&mut hash_self);
        other.hash(&mut hash_other);

        hash_self.finish() == hash_other.finish()
    }
}

impl Hash for BundleProtoMeta {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.proto_packet.hash(state);
    }
}

impl fmt::Display for BundleProtoMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, len={}, age={})", self.proto_type.as_str_name(), self.proto_packet.len(), unix_timestamp() - self.queued)
    }
}

/// Creates X number of send events from a bundle queue and optionally triggers a rescore before.
pub fn generate_sends(send_queue: &mut BundleQueue<BundleProtoMeta>, number: i32, rescore: bool) -> Vec<Event> {
    if rescore {
        send_queue.rescore();
    }

    // print current queue state
    debug!("queue state:\n{}", send_queue.print_state());

    let mut events: Vec<Event> = vec![];
    for _ in 0..number {
        let packet = send_queue.pop();
        if let Some(packet) = packet {
            debug!(
                    "creating LoRaPacketTx event of len={} ({} left)",
                    packet.1.clone().unwrap().proto_packet.len(),
                    send_queue.len()
                );

            events.push(Event::LoRaPacketTx(packet.1.unwrap().proto_packet))
        }
    }

    events
}