mod client;
mod host;
mod server;
mod video_stream;

use clap::{Parser, Subcommand};
use client::ClientArgs;
use server::ServerArgs;
use video_stream::VideoStreamer;

mod c_ar {
    tonic::include_proto!("c_ar");
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
    Client(ClientArgs),
    Server(ServerArgs),
}

#[derive(Subcommand, Debug)]
enum VideoInput {
    Video(VideoStreamer),
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

    match args.endpoint {
        EndpointType::Client(client) => {
            client.run().await?;
        },
        EndpointType::Server(server) => {
            server.run().await?;
        }
    };

    Ok(())
}
