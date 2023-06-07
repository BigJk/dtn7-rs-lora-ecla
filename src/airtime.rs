use serde::{Deserialize, Serialize};

/// AirtimeConfig is a struct that contains all the parameters needed to calculate the airtime of a LoRa packet.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AirtimeConfig {
    pub preamble_len: f64,
    pub spreading_factor: f64,
    pub band_width: f64,
    pub coding_rate: f64,
    pub crc: bool,
    pub explicit_header: bool,
    pub low_data_rate_optimization: bool,
}

impl Default for AirtimeConfig {
    fn default() -> Self {
        AirtimeConfig {
            preamble_len: 6.0,
            spreading_factor: 7.0,
            band_width: 125.0,
            coding_rate: 5.0,
            crc: false,
            explicit_header: false,
            low_data_rate_optimization: false,
        }
    }
}

impl AirtimeConfig {
    /// new creates a new AirtimeConfig struct.
    pub fn new(preamble_len: f64, spreading_factor: f64, band_width: f64, coding_rate: f64, crc: bool, explicit_header: bool, low_data_rate_optimization: bool) -> AirtimeConfig {
        AirtimeConfig {
            preamble_len,
            spreading_factor,
            band_width,
            coding_rate,
            crc,
            explicit_header,
            low_data_rate_optimization,
        }
    }

    /// payload_valid checks if the payload length is valid.
    pub fn payload_valid(&self, payload_len: i32) -> bool {
        payload_len >= 1 && payload_len < 255
    }

    /// preamble_valid checks if the preamble length is valid.
    pub fn preamble_valid(&self) -> bool {
        self.preamble_len >= 6.0 && self.preamble_len <= 655365.0
    }

    /// symbol_time calculates the symbol time.
    pub fn symbol_time(&self) -> f64 {
        2.0f64.powf(self.spreading_factor) / self.band_width
    }

    /// symbol_rate calculates the symbol rate.
    pub fn symbol_rate(&self) -> f64 {
        1000.0 / self.symbol_time()
    }

    /// throughput calculates the throughput.
    pub fn throughput(&self, payload_len: i32) -> f64 {
        ((8.0 * (payload_len as f64)) / self.time_total(payload_len)) * 1000.0
    }

    /// n_payload calculates the number of payload symbols.
    pub fn n_payload(&self, payload_len: i32) -> f64 {
        let mut payload_bit = 8.0 * (payload_len as f64);
        payload_bit -= 4.0 * self.spreading_factor;
        payload_bit += 28.0;

        if self.crc {
            payload_bit += 16.0;
        }

        if self.explicit_header {
            payload_bit += 20.0;
        }

        payload_bit = payload_bit.max(0.0);

        let mut bits_per_symbol = self.spreading_factor;
        if self.low_data_rate_optimization {
            bits_per_symbol = self.spreading_factor - 2.0;
        }

        let mut payload_symbol = (payload_bit / 4.0 / bits_per_symbol) * self.coding_rate;
        payload_symbol += 8.0;

        payload_symbol
    }

    /// n_preamble calculates the number of preamble symbols.
    pub fn n_preamble(&self) -> f64 {
        self.preamble_len + 4.25
    }

    /// time_payload calculates the time the payload alone will be on air.
    pub fn time_payload(&self, payload_len: i32) -> f64 {
        self.n_payload(payload_len) * self.symbol_time()
    }

    /// time_preamble calculates the time the preamble alone will be on air.
    pub fn time_preamble(&self) -> f64 {
        self.n_preamble() * self.symbol_time()
    }

    /// time_total calculates the total time the packet will be on air.
    pub fn time_total(&self, payload_len: i32) -> f64 {
        self.time_preamble() + self.time_payload(payload_len)
    }
}