use std::{net::SocketAddr, error::Error};

use clap::Args;
use tonic::transport::Server;
use webrtc::ice_transport::ice_server::RTCIceServer;

use crate::{VideoInput, host::{StreamGenerator, Host}, c_ar::control_server::ControlServer};

#[derive(Args, Debug)]
pub struct ServerArgs {
    addr: SocketAddr,
    #[arg(short, long)]
    ice: Option<String>,
    #[command(subcommand)]
    input: VideoInput,
}

impl ServerArgs {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        match self.input {
            VideoInput::Video(streamer) => start_service(self.addr, self.ice, streamer).await?,
        }
        Ok(())
    }
}

async fn start_service<S: StreamGenerator + Send + Sync + 'static>(
    addr: SocketAddr,
    ice: Option<String>,
    gen: S,
) -> Result<(), Box<dyn std::error::Error>>
where
    S::Stream: Send + Sync + 'static,
{
    let mut host = Host::new(gen)?.add_ice_server(RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_owned()],
        ..Default::default()
    });
    if let Some(ice) = ice {
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