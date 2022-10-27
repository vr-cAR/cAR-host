use std::error::Error;

use clap::Args;
use tokio::{sync::mpsc, io::{self, BufReader, AsyncBufReadExt}};
use tonic::transport::Endpoint;

use crate::c_ar::{control_client::ControlClient, handshake_message::Msg, HandshakeMessage, NotifyDescription, SdpType};

#[derive(Args, Debug)]
pub struct ClientArgs {
    endpoint: Endpoint,
}

impl ClientArgs {
    pub async fn run(self) -> Result<(), Box<dyn Error>>{
        let mut client = ControlClient::connect(self.endpoint).await?;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut output_rx = client.send_handshake(async_stream::stream! {
            while let Some(msg) = rx.recv().await {
                yield msg;
            }
        }).await?.into_inner();

        // spawn response reader
        let response_reader_handle = tokio::spawn(async move {
            while let Some(msg) = output_rx.message().await? {
                if let Some(msg) = msg.msg {
                    match msg {
                        Msg::Description(description) => {
                            println!("{{
                                type: {:?}
                                sdp: {}
                            }}", description.sdp_type(), description.sdp);
                        },
                        Msg::Ice(ice) => {
                            println!("{}", String::from_utf8(base64::decode(ice.json_base64)?)?);
                        },
                    }
                }
            }
            Ok::<(), Box<dyn Error + Send + Sync>>(())
        });

        // run request reader
        let mut stdin = BufReader::new(io::stdin());
        let mut sdp = String::new();
        stdin.read_line(&mut sdp).await?;
        let mut notify_description = NotifyDescription::default();
        notify_description.set_sdp_type(SdpType::Answer);
        notify_description.sdp = sdp;
        tx.send(HandshakeMessage {
            msg: Some(Msg::Description(
                notify_description
            ))
        })?;
        response_reader_handle.await?.map_err(|err| err as Box<dyn Error>)?;
        
        Ok(())
    }
}