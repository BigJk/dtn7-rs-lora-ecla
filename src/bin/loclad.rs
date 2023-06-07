use lora_ecla::global;
use lora_ecla::config;
use lora_ecla::dtnd_actor;
use lora_ecla::ecla_actor;
use lora_ecla::httpd;
use lora_ecla::lora_actor;
use lora_ecla::strategy;
use lora_ecla::strategy_actor;
use lora_ecla::supervisor_actor;
use lora_ecla::emu_actor;

use crate::global::{get_sender, CloseType, Event};
use crate::config::Config;
use crate::dtnd_actor::start_dtnd_actor;
use crate::ecla_actor::start_ecla_actor;
use crate::httpd::start_httpd;
use crate::lora_actor::start_lora_actor;
use crate::strategy::StrategyConfig;
use crate::strategy_actor::start_strategy_actor;
use crate::supervisor_actor::start_supervisor_actor;
use crate::emu_actor::start_emu_actor;

use anyhow::Result;
use clap::{crate_authors, crate_version, Arg, ArgMatches, Command};
use log::{debug, error, info};
use std::fmt::Display;
use std::fs::File;
use std::io::BufReader;
use std::str::FromStr;
use std::string::ToString;
use std::time::Duration;
use tokio::signal;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use lora_ecla::global::airtime_last_secs;

/// Overwrites a value if a match is present
fn overwrite_arg<T>(matches: &ArgMatches, key: &str, res: &mut T)
where
    T: FromStr,
    <T as FromStr>::Err: Display,
{
    if !matches.is_present(key) {
        return;
    }

    *res = matches.value_of_t(key).unwrap();
}

/// Builds the config from file with command line argument overwrites
fn build_config(matches: ArgMatches) -> Result<Config> {
    let mut config = Config::default();

    // If config is specified read if from here
    if matches.is_present("config") {
        let file = File::open(matches.value_of("config").unwrap())?;
        let reader = BufReader::new(file);
        config = serde_json::from_reader(reader)?;
    }

    // Overwrite config by arguments
    overwrite_arg(&matches, "lora_agent", &mut config.lora.agent_type);
    overwrite_arg(&matches, "lora_arg", &mut config.lora.agent_arg);
    overwrite_arg(&matches, "ecla_addr", &mut config.ecla.addr);
    overwrite_arg(&matches, "ecla_module", &mut config.ecla.module_name);
    overwrite_arg(&matches, "strategy_name", &mut config.strategy.name);
    overwrite_arg(&matches, "dtnd", &mut config.dtnd_executable);
    overwrite_arg(&matches, "dtnd_args", &mut config.dtnd.args);
    overwrite_arg(&matches, "airtime_preamble_len", &mut config.airtime.preamble_len);
    overwrite_arg(&matches, "airtime_spreading_factor", &mut config.airtime.spreading_factor);
    overwrite_arg(&matches, "airtime_band_width", &mut config.airtime.band_width);
    overwrite_arg(&matches, "airtime_coding_rate", &mut config.airtime.coding_rate);
    overwrite_arg(&matches, "airtime_crc", &mut config.airtime.crc);
    overwrite_arg(&matches, "airtime_low_data_rate_optimization", &mut config.airtime.low_data_rate_optimization);
    overwrite_arg(&matches, "airtime_explicit_header", &mut config.airtime.explicit_header);
    overwrite_arg(&matches, "webport", &mut config.web_port);

    if matches.is_present("strategy_config") && !matches.value_of("strategy_config").unwrap().is_empty() {
        let mut strategy_config: StrategyConfig = StrategyConfig::new();
        let raw_strategy_args = matches.value_of("strategy_config").unwrap();
        if raw_strategy_args.len() > 0 {
            raw_strategy_args.split(',').into_iter().for_each(|res| {
                let split: Vec<&str> = res.split('=').collect();
                if split.len() != 2 {
                    panic!("a strategy argument was not in the format KEY=VAL")
                }

                strategy_config.insert(split[0].to_string(), split[1].to_string());
            });

            config.strategy.config = strategy_config;
        }
    }

    config.dtnd_executable = config.dtnd_executable.trim_matches('\'').trim_matches('"').to_string();
    config.dtnd.args = config.dtnd.args.trim_matches('\'').trim_matches('"').to_string();

    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Command::new("LoRa ECLA")
        .version(crate_version!())
        .author(crate_authors!())
        .about("Configurable LoRa ECLA for dtn7-rs using Protocol Buffer overlay network.")

        // Config file
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("config")
                .help("Specify config file")
                .multiple_values(false),
        )

        // LoRa overwrites
        .arg(Arg::new("lora_agent").long("lora_agent").value_name("lora_agent").multiple_values(false))
        .arg(Arg::new("lora_arg").long("lora_arg").value_name("lora_arg").multiple_values(false))

        // ECLA overwrites
        .arg(Arg::new("ecla_addr").long("ecla_addr").value_name("ecla_addr").multiple_values(false))
        .arg(Arg::new("ecla_module").long("ecla_module").value_name("ecla_module").multiple_values(false))

        // Strategy overwrites
        .arg(Arg::new("strategy_name").long("strategy_name").value_name("strategy_name").multiple_values(false))
        .arg(Arg::new("strategy_config").long("strategy_config").value_name("strategy_config").multiple_values(false))

        // DTND Overwrites
        .arg(Arg::new("dtnd").long("dtnd").value_name("dtnd").multiple_values(false))
        .arg(Arg::new("dtnd_args").long("dtnd_args").value_name("dtnd_args").multiple_values(false))

        // Airtime Overwrites
        .arg(Arg::new("airtime_preamble_len").long("airtime_preamble_len").value_name("airtime_preamble_len").multiple_values(false))
        .arg(Arg::new("airtime_spreading_factor").long("airtime_spreading_factor").value_name("airtime_spreading_factor").multiple_values(false))
        .arg(Arg::new("airtime_band_width").long("airtime_band_width").value_name("airtime_band_width").multiple_values(false))
        .arg(Arg::new("airtime_coding_rate").long("airtime_coding_rate").value_name("airtime_scoding_rate").multiple_values(false))
        .arg(Arg::new("airtime_crc").long("airtime_crc").value_name("airtime_crc").multiple_values(false))
        .arg(Arg::new("airtime_explicit_header").long("airtime_explicit_header").value_name("airtime_explicit_header").multiple_values(false))
        .arg(Arg::new("airtime_low_data").long("airtime_low_data").value_name("airtime_explicit_header").multiple_values(false))

        // Other
        .arg(Arg::new("webport").long("webport").value_name("webport").multiple_values(false))

        // Flags
        .arg(
            Arg::new("create_config")
                .long("create_config")
                .help("Writes default config to --config path")
                .takes_value(false),
        )
        .arg(Arg::new("debug").short('d').long("debug").help("Set log level to debug").takes_value(false))
        .get_matches();

    if matches.is_present("create_config") {
        let default_config = Config {
            lora: Default::default(),
            ecla: Default::default(),
            dtnd: Default::default(),
            airtime: Default::default(),
            dtnd_executable: "dtnd".to_string(),
            strategy: Default::default(),
            web_port: 0,
        };

        let file = File::create(matches.value_of("config").unwrap())?;
        serde_json::to_writer_pretty(file, &default_config)?;

        info!("default config written.");

        return Ok(());
    }

    if matches.is_present("debug") {
        std::env::set_var("RUST_LOG", "debug,hyper=info,reqwest=info");
        pretty_env_logger::init_timed();
    }

    let config = build_config(matches)?;

    debug!("Config: {:#?}", config.clone());

    // Start actors
    let emu_handle = tokio::spawn(start_emu_actor());
    let dtnd_handle = tokio::spawn(start_dtnd_actor(config.dtnd_executable.clone()));
    let strategy_handle = tokio::spawn(start_strategy_actor());
    let ecla_handle = tokio::spawn(start_ecla_actor());
    let lora_handle = tokio::spawn(start_lora_actor());

    // Wait a moment for all the actor subscribers to be available
    sleep(Duration::from_millis(250)).await;

    // Start REST API
    let mut httpd_handle: Option<JoinHandle<()>> = None;
    if config.web_port > 0 && config.web_port < 65535 {
        httpd_handle.replace(tokio::spawn(start_httpd(config.web_port)));
    }

    // Start supervisor actor
    let supervisor_handle = tokio::spawn(start_supervisor_actor(config.clone()));

    // Handle CTRL-C interrupt
    tokio::spawn(async {
        match signal::ctrl_c().await {
            Ok(()) => {
                info!("Interrupt received. Shutting down...");
                let _ = get_sender().send(Event::Close(CloseType::All));
            }
            Err(err) => {
                error!("Unable to listen for shutdown signal: {}", err);
            }
        }
    });

    tokio::spawn(async {
        loop {
            sleep(Duration::from_secs(60 * 5)).await;

            let airtime_5min = airtime_last_secs(60 * 5);
            let airtime_60min = airtime_last_secs(60 * 60);
            let airtime_24h = airtime_last_secs(60 * 60 * 24);

            info!("Airtime 5min: {:.2}ms, 60min: {:.2}ms, 24h: {:.2}ms", airtime_5min, airtime_60min, airtime_24h);
        }
    });

    // Wait for actor finish
    let _ = ecla_handle.await;
    let _ = lora_handle.await;
    let _ = strategy_handle.await;
    let _ = dtnd_handle.await;
    let _ = emu_handle.await;
    let _ = supervisor_handle.await;

    // Close REST API
    if let Some(httpd_handle) = httpd_handle {
        httpd_handle.abort();
    }

    Ok(())
}
