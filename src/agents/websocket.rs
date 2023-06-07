use crate::agents::{Command, LoRaAgent, RxPacket};
use anyhow::{bail, Result};
use async_trait::async_trait;
use futures_util::{pin_mut, SinkExt, StreamExt};
use log::error;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tungstenite::Message;

pub struct Websocket {
    node_id: String,
    ip: String,
    port: u32,
    packet_in: Option<mpsc::Sender<Command>>,
    packet_out: mpsc::Sender<RxPacket>,
}

impl Websocket {
    pub fn new(node_id : String, ip: String, port: u32, packet_out: mpsc::Sender<RxPacket>) -> Result<Websocket> {
        Ok(Websocket {
            node_id,
            ip,
            port,
            packet_in: None,
            packet_out,
        })
    }
}

#[async_trait]
impl LoRaAgent for Websocket {
    async fn open(&mut self) -> Result<()> {
        let url = url::Url::parse(format!("ws://{}:{}/api/emu/{}", self.ip, self.port, self.node_id).as_str()).unwrap();

        let (tx_in, mut tx_out) = mpsc::channel(100);
        self.packet_in = Some(tx_in);

        let (ws_stream, _) = connect_async(url).await?;
        let (mut write, read) = ws_stream.split();

        let packet_out = std::mem::replace(&mut self.packet_out, mpsc::channel(1).0);
        tokio::spawn(async move {
            let ws_to_rx = {
                read.for_each(|message| async {
                    match message {
                        Ok(msg) => {
                            let data = msg.into_data();
                            let packet : serde_json::Result<RxPacket> = serde_json::from_slice(data.as_slice());

                            match packet {
                                Ok(packet) => {
                                    if packet_out.send(packet).await.is_err() {
                                        error!("error while sending to websocket rx channel");
                                    }
                                }
                                Err(err) => {
                                    error!("error while decoding received packet: err={}", err);
                                }
                            }
                        }
                        Err(_) => {
                            error!("error while reading message from websocket");
                        }
                    }
                })
            };

            pin_mut!(ws_to_rx);
            ws_to_rx.await;
        });

        tokio::spawn(async move {
            while let Some(res) = tx_out.recv().await {
                match res {
                    Command::Send(msg) => {
                        if write.send(Message::Binary(msg)).await.is_err() {
                            error!("error while writing to websocket");
                        }
                    }
                    Command::Close() => {
                        tx_out.close();
                        return;
                    }
                }
            }
        });

        Ok(())
    }

    async fn lat_lng(&mut self) -> Result<(f64, f64)> {
        let res = reqwest::get(format!("http://{}:{}/api/node/{}/latlng", self.ip, self.port, self.node_id).as_str()).await?;
        let bytes = res.bytes().await?;

        let latlng : Vec<f64> = serde_json::from_slice(bytes.as_ref())?;

        if latlng.len() == 2 {
            return Ok((latlng[0], latlng[1]));
        }

        bail!("couldn't get lat long");
    }

    fn command_channel(&self) -> Option<mpsc::Sender<Command>> {
        self.packet_in.clone()
    }
}
