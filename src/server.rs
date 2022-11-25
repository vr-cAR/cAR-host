use std::{error::Error, fs::File, net::SocketAddr, path::PathBuf};

use clap::Args;
use tonic::transport::Server;
use webrtc::ice_transport::ice_server::RTCIceServer;

use crate::{
    c_ar::control_server::ControlServer,
    host::Host,
    media::{
        controls::{txt::TxtControlsReceiverConfig, ControlsMediaProvider},
        video::rtp::RtpMediaProvider,
        MediaProvider,
    },
};

#[derive(Args, Debug)]
pub struct ServerArgs {
    #[clap(short, long, default_value = "10.0.0.1:1234")]
    addr: SocketAddr,
    config: PathBuf,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ServerConfig {
    #[serde(default)]
    addr: Option<SocketAddr>,
    #[serde(default)]
    ice_servers: Vec<String>,
    media: Vec<MediaInput>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
enum MediaInput {
    #[cfg(feature = "ros")]
    RosControls(crate::media::controls::ros::RosControlsReceiverConfig),
    TxtControls(TxtControlsReceiverConfig),
    Rtp(RtpMediaProvider),
    #[cfg(feature = "gstreamer")]
    Gst(crate::media::video::gst::GstMediaProvider),
}

impl From<MediaInput> for Box<dyn MediaProvider + Send + Sync> {
    fn from(val: MediaInput) -> Self {
        match val {
            #[cfg(feature = "ros")]
            MediaInput::RosControls(provider) => Box::new(ControlsMediaProvider::new(provider)),
            MediaInput::TxtControls(provider) => Box::new(ControlsMediaProvider::new(provider)),
            MediaInput::Rtp(provider) => Box::new(provider),
            #[cfg(feature = "gstreamer")]
            MediaInput::Gst(provider) => Box::new(provider),
        }
    }
}

impl ServerArgs {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        let config: ServerConfig = serde_yaml::from_reader(File::open(self.config)?)?;
        let mut host = Host::new(config.media.into_iter().map(Into::into).collect())?;

        for ice in config.ice_servers {
            host = host.add_ice_server(RTCIceServer {
                urls: vec![ice],
                ..Default::default()
            });
        }

        let addr = match config.addr {
            Some(addr) => addr,
            None => self.addr,
        };

        Server::builder()
            .add_service(ControlServer::new(host))
            .serve(addr)
            .await?;
        Ok(())
    }
}
