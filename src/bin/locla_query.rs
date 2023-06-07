use clap::{Parser, Subcommand};
use log::error;
use lora_ecla::httpd::PingPongRequest;

/// A simple Bundle Protocol 7 Query Utility for Delay Tolerant Networking
#[derive(Parser, Debug)]
#[clap(version, author, long_about = None)]
struct Args {
    /// Local web port (default = 7262)
    #[clap(short, long, default_value_t = 7262)]
    port: u16,

    #[clap(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    LoraConfig,
    DTNDConfig,
    StrategyConfig,
    PingPong {
        #[clap(short, long)]
        target: Option<String>,
        #[clap(short, long)]
        wait_ms: u32,
    },
}

fn main() {
    let args = Args::parse();

    let url = match args.cmd {
        Commands::LoraConfig => {
            format!("http://127.0.0.1:{}/config/lora", args.port)
        }
        Commands::DTNDConfig => {
            format!("http://127.0.0.1:{}/config/dtnd", args.port)
        }
        Commands::StrategyConfig => {
            format!("http://127.0.0.1:{}/config/strategy", args.port)
        }
        Commands::PingPong { target, wait_ms } => {
            let res = attohttpc::post(format!("http://127.0.0.1:{}/ping_pong", args.port))
                .json(&PingPongRequest {
                    node_id: target.unwrap_or("".to_string()),
                    timeout_ms: wait_ms,
                })
                .expect("could not encode json body")
                .send()
                .expect("error connecting to local loclad")
                .text()
                .unwrap();

            println!("{}", res);

            "".to_string()
        }
        _ => "".to_string(),
    };

    if url.is_empty() {
        return;
    }

    let res = attohttpc::get(url).send().expect("error connecting to local loclad").text().unwrap();
    println!("{}", res);
}
