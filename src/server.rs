use std::{error::Error, net::SocketAddr};

use clap::{Args, Subcommand};
use tonic::transport::Server;
use webrtc::ice_transport::ice_server::RTCIceServer;

use crate::{
    c_ar::control_server::ControlServer,
    host::{Host, MediaProvider},
    rtp_media_generator::RtpMediaGenerator,
};

#[derive(Args, Debug)]
pub struct ServerArgs {
    addr: SocketAddr,
    #[arg(short, long)]
    queue_size: usize,
    #[arg(short, long)]
    ice: Vec<String>,
    #[command(subcommand)]
    input: MediaInput,
}

#[derive(Subcommand, Debug)]
enum MediaInput {
    Rtp(RtpMediaGenerator),
}

impl ServerArgs {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        match self.input {
            MediaInput::Rtp(rtp) => start_service(self.addr, self.ice, rtp).await,
        }
    }
}

async fn start_service<P>(
    addr: SocketAddr,
    ices: Vec<String>,
    provider: P,
) -> Result<(), Box<dyn std::error::Error>>
where
    P: MediaProvider + Send + Sync + 'static,
{
    let mut host = Host::new(provider)?.add_ice_server(RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_owned()],
        ..Default::default()
    });
    for ice in ices {
        host = host.add_ice_server(RTCIceServer {
            urls: vec![ice],
            ..Default::default()
        });
    }
    Server::builder()
        .add_service(ControlServer::new(host))
        .serve(addr)
        .await?;
    Ok(())
}
