mod client;
mod host;
mod media;
mod msg_ext;
mod server;

use clap::{Parser, Subcommand};
use log::error;

mod c_ar {
    tonic::include_proto!("c_ar");
}

pub mod c_ar_controls {
    include!(concat!(env!("OUT_DIR"), "/c_ar_controls.rs"));
}

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct HostArgs {
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    endpoint: EndpointType,
}

#[derive(Subcommand, Debug)]
enum EndpointType {
    Client(client::ClientArgs),
    Server(server::ServerArgs),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = HostArgs::parse();
    match args.verbose {
        0 => simple_logger::init_with_level(log::Level::Warn)?,
        1 => simple_logger::init_with_level(log::Level::Info)?,
        2 => simple_logger::init_with_level(log::Level::Debug)?,
        _ => simple_logger::init_with_level(log::Level::Trace)?,
    };

    if let Err(err) = match args.endpoint {
        EndpointType::Client(client) => client.run().await,
        EndpointType::Server(server) => server.run().await,
    } {
        error!("Error: {}", err);
        return Err(err);
    }

    Ok(())
}
