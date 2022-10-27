use std::{net::SocketAddr, error::Error};

use clap::{Args, Subcommand};
use tonic::transport::Server;
use webrtc::ice_transport::ice_server::RTCIceServer;

use crate::{host::{Host, MediaProvider}, c_ar::control_server::ControlServer, rtp_media_generator::RtpMediaGenerator};

#[derive(Args, Debug)]
pub struct ServerArgs {
    addr: SocketAddr,
    #[arg(short, long)]
    ice: Vec<String>,
    #[command(subcommand)]
    input: VideoInput,
}

#[derive(Subcommand, Debug)]
enum VideoInput {
    RTP(RtpMediaGenerator)
}

impl ServerArgs {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        match self.input {
            VideoInput::RTP(rtp) => start_service(self.addr, self.ice, rtp).await,
        }
    }
}

async fn start_service<P>(
    addr: SocketAddr,
    ices: Vec<String>,
    provider: P,
) -> Result<(), Box<dyn std::error::Error>>
where
    P: MediaProvider + Send + Sync + 'static
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