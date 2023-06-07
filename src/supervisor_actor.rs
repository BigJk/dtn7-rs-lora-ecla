use log::info;
use crate::global::{CloseType, Event, get_channels};
use crate::config::Config;

pub async fn start_supervisor_actor(config: Config) {
    let (broadcast_sender, mut broadcast_receiver) = get_channels();

    info!("supervisor actor started");

    // Send requests to start lora & dtnd
    let _ = broadcast_sender.send(Event::LoRaStart(config.lora.clone(), config.airtime.clone()));
    let _ = broadcast_sender.send(Event::DTNDStart(config.dtnd.clone()));

    loop {
        if let Ok(msg) = broadcast_receiver.recv().await {
            match msg {
                // DTNDStarted
                //
                // When dtnd is started we can start the ECLA and Strategy.
                Event::DTNDStarted() => {
                    let _ = broadcast_sender.send(Event::ECLAStart(config.ecla.clone()));
                    let _ = broadcast_sender.send(Event::StrategyStart(config.strategy.clone()));
                }
                Event::Close(CloseType::All) => {
                    return;
                }
                _ => {}
            }
        }
    }
}