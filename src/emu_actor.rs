use crate::global::{get_channels, Event, CloseType};
use anyhow::{bail, Result};
use reqwest::StatusCode;
use std::collections::HashMap;
use log::error;

async fn commit_meta(ip: &str, port: &str, node_id: &str, meta: &HashMap<String, String>) -> Result<()> {
    let res = reqwest::Client::new()
        .put(format!("http://{}:{}/api/node/{}/meta", ip, port, node_id).as_str())
        .json(meta)
        .send()
        .await?;

    if res.status() != StatusCode::OK {
        bail!("couldn't set meta");
    }

    Ok(())
}

pub async fn start_emu_actor() {
    let (_, mut receive) = get_channels();

    let mut is_emu = false;
    let mut config: Vec<String> = vec![];
    let mut meta: HashMap<String, String> = HashMap::new();

    loop {
        let mut commit = false;

        if let Ok(msg) = receive.recv().await {
            match msg {
                Event::LoRaStart(start, _) => {
                    if start.agent_type == "websocket" {
                        is_emu = true;
                        config = start.agent_arg.split(':').map(|s| s.to_owned()).collect::<Vec<String>>();
                    }
                }
                Event::StrategyStart(start) => {
                    meta.insert("strategy".to_string(), start.name);
                    for kv in start.config.iter() {
                        meta.insert(kv.0.clone(), kv.1.clone());
                    }

                    commit = true;
                }
                Event::ECLAStart(start) => {
                    meta.insert("dtnd".to_string(), "http://".to_owned() + &start.addr);

                    commit = true;
                }
                Event::Close(CloseType::All) => {
                    return
                }
                _ => {}
            }
        }

        if commit && is_emu && !config.is_empty() {
            if let Err(err) = commit_meta(config[0].as_str(), config[1].as_str(), config[2].as_str(), &meta).await {
                error!("could not commit meta info for emulator: {}", err);
            }
        }
    }
}
