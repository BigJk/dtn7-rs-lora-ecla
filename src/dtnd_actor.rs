use std::process::Stdio;
use lazy_static::lazy_static;
use log::{error, info, warn};
use crate::global::{broadcast_event, CloseType, DTNDConfig, Event, get_channels};
use std::sync::{Arc};
use parking_lot::Mutex;
use std::time::Duration;
use tokio::time::sleep;

lazy_static! {
    static ref DTND_RUNNING: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
}

pub async fn start_dtnd(dtnd_location: String, config: DTNDConfig) {
    // Wait for former instance to stop
    loop {
        let mut running = DTND_RUNNING.lock();
        if !*running {
            *running = true;
            break;
        }

        sleep(Duration::from_millis(100)).await;
    }

    let dtnd_start = tokio::process::Command::new(dtnd_location)
        .args(config.args.split(' '))
        .stdout(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn();

    if dtnd_start.is_err() {
        error!("could not start dtnd: {}", dtnd_start.err().unwrap());
        return;
    }

    info!("dtnd started.");

    // Wait a second for dtnd to fully boot and then emit the DTND started event.
    tokio::spawn(async {
        sleep(Duration::from_secs(1)).await;
        broadcast_event(Event::DTNDStarted());
    });

    // TODO: check if dtnd died and re-start

    let mut dtnd = dtnd_start.unwrap();
    let (_, mut receive) = get_channels();
    loop {
        if let Ok(msg) = receive.recv().await {
            match msg {
                Event::Close(CloseType::All | CloseType::DTND) | Event::DTNDStart(_) => {
                    info!("killing dtnd process.");
                    let _ = dtnd.kill();
                    (*DTND_RUNNING.lock()) = false;
                }
                _ => {}
            }
        }
    }
}

pub async fn start_dtnd_actor(dtnd_location: String) {
    let (_, mut receive) = get_channels();
    let mut latest_config: DTNDConfig = DTNDConfig::default();

    info!("dtnd Actor started");

    loop {
        if let Ok(msg) = receive.recv().await {
            match msg {
                Event::DTNDStart(start) => {
                    info!("received dtnd start request. Starting...");

                    if dtnd_location.is_empty() {
                        warn!("can't start dtnd as no executable is supplied. Optimistically expect it to be running externally...");
                        broadcast_event(Event::DTNDStarted());
                        continue;
                    }

                    latest_config = start.clone();
                    tokio::spawn(start_dtnd(dtnd_location.clone(), start));
                }
                Event::Close(CloseType::All) => {
                    return
                },
                _ => {}
            }
        }
    }
}
