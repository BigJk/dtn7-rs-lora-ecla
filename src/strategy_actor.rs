use std::time::Duration;
use humantime::parse_duration;
use log::{error, info};
use tokio::time::interval;
use crate::global::{CloseType, Event, get_channels, get_node_id, get_position, get_sender, StrategyConfig};
use crate::protocol;
use crate::protocol::packet::Content;
use crate::strategy::start_strategy;

async fn handle_advertise_recommendation(advertise_interval: Duration) {
    info!("Advertise recommender started");

    let mut ticker = interval(advertise_interval);
    let sender = get_sender();
    loop {
        ticker.tick().await;

        let pos = get_position();
        let data = std::collections::HashMap::new();

        let own_node_name = get_node_id();
        if own_node_name.is_empty() {
            error!("own node name is empty!");
            continue;
        }

        if sender.send(Event::StrategyRecommendAdvertise(protocol::Packet {
            r#type: protocol::PacketType::TypeAdvertise.into(),
            content: Some(Content::Advertise(protocol::Advertise {
                node_name: own_node_name,
                position: Some(pos),
                data,
            })),
        })).is_err() {
            error!("error while sending advertise recommendation")
        }
    }
}

pub async fn start_strategy_actor() {
    let (_, mut receive) = get_channels();
    let mut latest_config: StrategyConfig = StrategyConfig::default();
    let mut advertise_task = tokio::spawn(handle_advertise_recommendation(Duration::from_secs(60)));

    info!("Strategy Actor started");

    loop {
        if let Ok(msg) = receive.recv().await {
            match msg {
                Event::StrategyStart(start) => {
                    info!("received Strategy start request. Starting...");

                    latest_config = start.clone();

                    // Restart advertise task
                    advertise_task.abort();
                    advertise_task = tokio::spawn(handle_advertise_recommendation( parse_duration(latest_config.config.get("ADVERTISE_INTERVAL").unwrap_or(&"60s".to_string())).expect("ADVERTISE_INTERVAL is in wrong format")));

                    match start_strategy(start.name.as_str(), start.config) {
                        Ok(_) => {}
                        Err(err) => {
                            error!("error while starting strategy: {}", err);
                        }
                    }
                }
                Event::StrategyFetchConfig(resp) => {
                    if let Some(resp) = resp.lock().take() {
                        let _ = resp.send(latest_config.clone());
                    }
                }
                Event::Close(CloseType::All) => {
                    return;
                },
                _ => {}
            }
        }
    }
}
