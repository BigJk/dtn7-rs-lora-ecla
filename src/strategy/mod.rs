use crate::global::{get_sender, Event};
use anyhow::{anyhow};
use bp7::EndpointID;
use dtn7::client::ecla::Command::SendPacket;
use dtn7::client::ecla::{Beacon, Packet};
use dtn7::ipnd::services::ServiceBlock;
use humantime::parse_duration;
use log::error;
use std::collections::HashMap;
use tokio::task::JoinHandle;
use tokio::time::interval;

use crate::strategy::quadrant::quadrant_routing;
use crate::strategy::random::random_routing;
use crate::strategy::sprayandwait::spray_and_wait_routing;

pub mod proto;
pub mod quadrant;
pub mod random;
pub mod sprayandwait;
pub mod fistcontact;
mod dummy;

pub type StrategyConfig = HashMap<String, String>;

/// Starts a strategy by name with the given config.
pub fn start_strategy(name: &str, config: StrategyConfig) -> anyhow::Result<JoinHandle<()>> {
    match name {
        "quadrant" => {
            Ok(tokio::spawn(quadrant_routing(config)))
        }
        "random" => {
            Ok(tokio::spawn(random_routing(config)))
        }
        "spray_and_wait" => {
            Ok(tokio::spawn(spray_and_wait_routing(config)))
        }
        _ => {
            Err(anyhow!("unknown strategy"))
        }
    }
}

/// Peer Refresh
///
/// To make dtnd send this ECLA data there needs to be a peer present. We just create
/// a static LoRa peer and refresh that so that we continue to receive packets.
fn local_peer_refresh() -> JoinHandle<()> {
    tokio::spawn(async move {
        let broadcast_sender = get_sender();

        let mut service_block = ServiceBlock::new();
        service_block.add_cla("LoRa", &None);
        let service_vec = serde_cbor::to_vec(&service_block).unwrap();

        let mut ticker = interval(parse_duration("3s").unwrap());
        loop {
            ticker.tick().await;

            if broadcast_sender
                .send(Event::ECLAPacketTx(SendPacket(Packet::Beacon(Beacon {
                    eid: EndpointID::try_from("dtn://LoRa_Local/").unwrap(),
                    service_block: service_vec.clone(),
                    addr: "".to_string(),
                }))))
                .is_err()
            {
                error!("error while sending beacon to ecla");
                break;
            }
        }
    })
}
