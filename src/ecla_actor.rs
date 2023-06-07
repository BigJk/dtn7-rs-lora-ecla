use crate::global::{get_channels, CloseType, Event, publish_node_id, ECLAConfig};
use dtn7::client::ecla;
use dtn7::client::ecla::Command;
use futures::pin_mut;
use lazy_static::lazy_static;
use log::{error, info};
use std::sync::{Arc};
use parking_lot::Mutex;
use std::time::Duration;
use dtn7::cla::ecla::Packet::Registered;
use tokio::sync::mpsc;
use tokio::time::sleep;

lazy_static! {
    static ref ECLA_RUNNING: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
}

async fn start_ecla_client(addr: String) {
    // Wait for former instance to stop
    loop {
        let mut running = ECLA_RUNNING.lock();
        if !*running {
            *running = true;
            break;
        }

        sleep(Duration::from_millis(100)).await;
    }

    // Fetch broadcast channels
    let (broadcast_send, mut broadcast_receive) = get_channels();

    // Create channels for ecla packets
    let (ecla_tx, mut ecla_rx) = mpsc::channel::<ecla::Packet>(100);

    // Creating the client task
    tokio::spawn(async move {
        let mut c = ecla::ws_client::new("LoRa", addr.as_str(), "", ecla_tx, true).expect("couldn't create ecla client");

        // Get the command channel of the client
        let cmd_chan = c.command_channel();

        // Handle channels
        let read = tokio::spawn(async move {
            loop {
                tokio::select! {
                    res = broadcast_receive.recv() => {
                        if let Ok(msg) = res {
                            match msg {
                                Event::ECLAPacketTx(cmd) => {
                                    if let Err(err) = cmd_chan.send(cmd).await {
                                        error!("couldn't pass packet to client command channel: {}", err);
                                    }
                                }
                                Event::ECLAStart(_) => {
                                    if let Err(err) = cmd_chan.send(Command::Close).await {
                                        error!("couldn't pass packet to client command channel: {}", err);
                                    }

                                    return;
                                }
                                Event::Close(target) => {
                                    match target {
                                        CloseType::All | CloseType::ECLA | CloseType::DTND => {
                                            if let Err(err) = cmd_chan.send(Command::Close).await {
                                                error!("couldn't pass packet to client command channel: {}", err);
                                            }

                                            return;
                                        }
                                        _ => {}
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    res = ecla_rx.recv() => {
                        if let Some(msg) = res {
                            let _ = broadcast_send.send(Event::ECLAPacketRx(msg));
                        }
                    }
                }
            }
        });

        info!("ECLA started");

        let connecting = c.serve();
        pin_mut!(connecting);

        let _ = connecting.await;
        let _ = read.await;

        // We are not connected to the ECLA anymore so clear the global node id.
        publish_node_id("".to_string());

        *ECLA_RUNNING.lock() = false;

        info!("ECLA shutdown finished");
    });
}

pub async fn start_ecla_actor() {
    let (_, mut receive) = get_channels();
    let mut latest_config: ECLAConfig = ECLAConfig::default();

    info!("ECLA Actor started");

    loop {
        if let Ok(msg) = receive.recv().await {
            match msg {
                Event::ECLAStart(start) => {
                    info!("received ECLA start request. Starting...");

                    latest_config = start.clone();
                    tokio::spawn(start_ecla_client(start.addr.clone()));
                }
                Event::ECLAFetchConfig(resp) => {
                    if let Some(resp) = resp.lock().take() {
                        let _ = resp.send(latest_config.clone());
                    }
                }
                // On ECLA Registered pass the node id to the global watch so all strategies and other places can
                // use it without having to get the registered event.
                Event::ECLAPacketRx(Registered(packet)) => {
                    info!("Registered to ECLA with node_id: {}", packet.nodeid.clone());
                    publish_node_id(packet.nodeid);
                },
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
