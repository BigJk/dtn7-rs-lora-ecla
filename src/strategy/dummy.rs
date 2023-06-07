use dtn7::client::ecla::Packet;
use crate::global::{get_channels, CloseType, Event};
use crate::strategy::StrategyConfig;

pub async fn dummy_routing(config: StrategyConfig) {
    let (bus_sender, mut bus_receive) = get_channels();

    // Use config values from config to configurate this strategy

    loop {
        if let Ok(msg) = bus_receive.recv().await {
            match msg {
                Event::StrategyRecommendAdvertise(packet) => {
                    // Send or queue a advertisement packet
                },
                Event::ECLAPacketRx(Packet::ForwardData(packet)) => {
                    // ECLA wants us to forward a bundle
                },
                Event::LoRaPacketParsedRx(packet) => {
                    // We received a parseable lora packet
                },
                Event::Close(target) => match target {
                    CloseType::All | CloseType::Strategy => {
                        // Stop this strategy
                        break;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}