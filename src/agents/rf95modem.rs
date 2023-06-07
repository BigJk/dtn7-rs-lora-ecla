use crate::agents::{Command, LoRaAgent, RxPacket};
use anyhow::Result;
use async_trait::async_trait;
use log::{debug, error};
use std::fmt::Write;
use std::time::{Duration, SystemTime};
use humantime::parse_duration;
use tokio::sync::mpsc;

pub struct Rf95modem {
    port_name: String,
    packet_in: Option<mpsc::Sender<Command>>,
    packet_out: mpsc::Sender<RxPacket>,
    watch_lng: Option<tokio::sync::watch::Receiver<f64>>,
    watch_lat: Option<tokio::sync::watch::Receiver<f64>>
}

impl Rf95modem {
    pub fn new(serial_port: String, packet_out: mpsc::Sender<RxPacket>) -> Result<Rf95modem> {
        Ok(Rf95modem {
            port_name: serial_port,
            packet_in: None,
            packet_out,
            watch_lng: None,
            watch_lat: None
        })
    }
}

#[async_trait]
impl LoRaAgent for Rf95modem {
    async fn open(&mut self) -> Result<()> {
        let mut port_read = serialport::new(self.port_name.clone(), 115200)
            .timeout(Duration::from_secs(2))
            .open()?;

        let mut port_write = port_read.try_clone()?;

        let (tx_in, mut tx_out) = mpsc::channel(100);
        self.packet_in = Some(tx_in.clone());

        let (lat_tx, lat_rx) = tokio::sync::watch::channel(0.0);
        let (lng_tx, lng_rx) = tokio::sync::watch::channel(0.0);

        self.watch_lat = Some(lat_rx);
        self.watch_lng = Some(lng_rx);

        // Read packets from serial and pass it to packet_out
        let packet_out = std::mem::replace(&mut self.packet_out, mpsc::channel(1).0);
        let serial_read = tokio::spawn(async move {
            let mut cur_packet = Vec::new();
            let mut read_buf: Vec<u8> = vec![0; 1];

            loop {
                if port_read.read(read_buf.as_mut_slice()).is_ok() {
                    if read_buf[0] as char == '\n' {
                        debug!(
                            "Packet={}",
                            cur_packet.clone().into_iter().collect::<String>()
                        );

                        let packet_str = cur_packet.clone().into_iter().collect::<String>();

                        if packet_str.starts_with("+RX ") {
                            let fields = packet_str.split(',').collect::<Vec<&str>>();

                            let data = hex::decode(fields[1]);
                            if data.is_err() {
                                error!("Hex decode error for data: {}", fields[1]);
                                cur_packet.clear();
                                continue;
                            }
                            
                            let rssi: i16 = fields[2].parse().unwrap();
                            let snr: i16 = fields[3].parse().unwrap();
                            let recv_time = SystemTime::now()
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .unwrap()
                                .as_secs();

                            let packet = RxPacket { rssi, snr, recv_time, data: data.unwrap() };
                            let res = packet_out.send(packet).await;
                            if let Err(err) = res {
                                error!("error while sending to packet out channel: {}", err);
                            }

                            debug!("RxPacket parsed: rssi={} snr={}", rssi, snr);
                        } else if packet_str.starts_with("Latitude") {
                            let fields = packet_str.split(':').collect::<Vec<&str>>();
                            if fields.len() == 2 {
                                if let Ok(lat) = fields[1].trim().parse() {
                                    _ = lat_tx.send(lat);
                                    debug!("Lat: {}", lat);
                                } else {
                                    error!("Error parsing lat: {}", fields[1].trim());
                                }
                            }
                        } else if packet_str.starts_with("Longitude") {
                            let fields = packet_str.split(':').collect::<Vec<&str>>();
                            if fields.len() == 2 {
                                if let Ok(lng) = fields[1].trim().parse() {
                                    _ = lng_tx.send(lng);
                                    debug!("Lng: {}", lng);
                                } else {
                                    error!("Error parsing lat: {}", fields[1].trim());
                                }
                            }
                        }

                        cur_packet.clear();
                    } else {
                        cur_packet.push(read_buf[0] as char);
                    }
                }
            }

            debug!("LoRa rf95modem done!");
        });

        // Write packets from packet_in to serial
        tokio::spawn(async move {
            if port_write.write(("AT+GPS=1\n").as_bytes()).is_err() {
                error!("error while writing gps enable to serial");
            }

            let mut gps_ticker = tokio::time::interval(parse_duration("5s").unwrap());

            loop {
                tokio::select! {
                    res = tx_out.recv() => {
                        if let Some(res) = res {
                            match res {
                                Command::Send(msg) => {
                                    let mut hex_data = String::new();
                                    for &byte in msg.as_slice() {
                                        write!(&mut hex_data, "{:X}", byte).expect("Unable to write");
                                    }

                                    if port_write.write(("AT+TX=".to_string() + &hex_data + "\n").as_bytes()).is_err() {
                                        error!("error while writing to serial");
                                    }
                                }
                                Command::Close() => {
                                    serial_read.abort();
                                    tx_out.close();
                                    return;
                                }
                            }
                        }
                    }
                    _ = gps_ticker.tick() => {
                        if port_write.write(("AT+GPS\n").as_bytes()).is_err() {
                            error!("error while writing gps request to serial");
                        }
                    }
                }
            }
        });

        Ok(())
    }

    async fn lat_lng(&mut self) -> Result<(f64, f64)> {
        if self.watch_lat.is_none() || self.watch_lng.is_none() {
            return Ok((0.0, 0.0))
        }
        return Ok((*self.watch_lat.as_ref().unwrap().borrow(), *self.watch_lng.as_ref().unwrap().borrow()))
    }

    fn command_channel(&self) -> Option<mpsc::Sender<Command>> {
        self.packet_in.clone()
    }
}
