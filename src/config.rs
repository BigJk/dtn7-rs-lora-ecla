use serde::{Deserialize, Serialize};
use crate::airtime::AirtimeConfig;
use crate::global::{DTNDConfig, ECLAConfig, LoRaConfig, StrategyConfig};

#[derive(Default, Clone, Serialize, Deserialize, Debug)]
pub struct Config {
    pub lora: LoRaConfig,
    pub ecla: ECLAConfig,
    pub dtnd: DTNDConfig,
    pub dtnd_executable: String,
    pub strategy: StrategyConfig,
    pub airtime: AirtimeConfig,
    pub web_port: u32,
}