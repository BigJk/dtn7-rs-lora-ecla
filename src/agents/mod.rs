pub mod rf95modem;
pub mod websocket;

use serde::{Deserialize, Serialize};
use anyhow::Result;
use async_trait::async_trait;

mod base64 {
    use base64::{decode, encode};
    use serde::{Deserialize, Serialize};
    use serde::{Deserializer, Serializer};

    // TODO: Uses a extra allocation at the moment. Might be worth investigating a allocation-less solution in the future.

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let base64 = encode(v);
        String::serialize(&base64, s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let base64 = String::deserialize(d)?;
        decode(base64.as_bytes()).map_err(serde::de::Error::custom)
    }
}

pub enum Command {
    Send(Vec<u8>),
    Close()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RxPacket {
    pub rssi: i16,
    pub snr: i16,
    #[serde(with="base64")]
    pub data: Vec<u8>,
    pub recv_time: u64,
}

#[async_trait]
pub trait LoRaAgent {
    async fn open(&mut self) -> Result<()>;
    async fn lat_lng(&mut self) -> Result<(f64, f64)>;
    fn command_channel(&self) -> Option<tokio::sync::mpsc::Sender<Command>>;
}

