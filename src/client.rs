use std::{error::Error, sync::Arc, time::Duration, net::SocketAddr};

use clap::Args;
use log::{warn, debug, info};
use tokio::{sync::mpsc, net::UdpSocket, signal};
use tonic::transport::Endpoint;
use webrtc::{peer_connection::{configuration::RTCConfiguration, offer_answer_options::RTCAnswerOptions, sdp::session_description::RTCSessionDescription}, api::{APIBuilder, media_engine::{MediaEngine, MIME_TYPE_H264}, interceptor_registry::register_default_interceptors}, rtp_transceiver::{rtp_codec::{RTCRtpCodecParameters, RTCRtpCodecCapability, RTPCodecType}, rtp_receiver::RTCRtpReceiver}, interceptor::registry::Registry, track::track_remote::TrackRemote, rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication, util::Marshal};

use crate::c_ar::{
    control_client::ControlClient, handshake_message::Msg, HandshakeMessage, NotifyIce,
};

#[derive(Args, Debug)]
pub struct ClientArgs {
    server: Endpoint,
    rtp: SocketAddr,
}

impl ClientArgs {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        info!("Starting client");
        let mut m = MediaEngine::default();

        // Setup the codecs you want to use.
        // We'll use a H264 and Opus but you can also define your own
        m.register_codec(
            RTCRtpCodecParameters {
                capability: RTCRtpCodecCapability {
                    mime_type: MIME_TYPE_H264.to_owned(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: "".to_owned(),
                    rtcp_feedback: vec![],
                },
                payload_type: 102,
                ..Default::default()
            },
            RTPCodecType::Video,
        )?;

        // Prepare the configuration
        let config = RTCConfiguration {
            ice_servers: vec![],
            ..Default::default()
        };
    
        // Create a InterceptorRegistry. This is the user configurable RTP/RTCP Pipeline.
        // This provides NACKs, RTCP Reports and other features. If you use `webrtc.NewPeerConnection`
        // this is enabled by default. If you are manually managing You MUST create a InterceptorRegistry
        // for each PeerConnection.
        let mut registry = Registry::new();
    
        // Use the default set of Interceptors
        registry = register_default_interceptors(registry, &mut m)?;

        let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();

        let peer_connection = Arc::new(api.new_peer_connection(config).await?);

        let mut client = ControlClient::connect(self.server).await?;
        let (server_tx, mut server_rx) = mpsc::unbounded_channel();
        
        info!("Sending handshake with server");
        let mut output_rx = client
            .send_handshake(async_stream::stream! {
                while let Some(msg) = server_rx.recv().await {
                    yield msg;
                }
            })
            .await?
            .into_inner();
        
        // spawn response reader
        let pc = peer_connection.clone();
        
        let stx = server_tx.clone();
        tokio::spawn(async move {
            let server_tx = stx;
            let peer_connection = pc;
            while let Some(msg) = output_rx.message().await? {
                if let Some(msg) = msg.msg {
                    match msg {
                        Msg::Description(description) => {    
                            let remote_description: RTCSessionDescription = description.into();
                            debug!(
                                "Got description from peer. Description: {}",
                                serde_json::to_string(&remote_description)?
                            );                

                            peer_connection.set_remote_description(remote_description).await?;
                            info!("Remote description successfully set. Creating answer");

                            let answer_options = RTCAnswerOptions {
                                ..Default::default()
                            };
                            let answer = peer_connection.create_answer(Some(answer_options)).await?;
                            let msg = HandshakeMessage {
                                msg: Some(Msg::Description(answer.clone().into()))
                            };
                            peer_connection.set_local_description(answer).await?;
                            server_tx.send(msg)?;
                        }
                        Msg::Ice(ice) => {
                            debug!("Got ice candidate from peer.");
                            peer_connection.add_ice_candidate(ice.try_into()?).await?;
                        }
                    }
                }
            }
            Ok::<(), Box<dyn Error + Send + Sync>>(())
        });

        let stx = server_tx;
        peer_connection.on_ice_candidate(Box::new(move |candidate| {
            let server_tx = stx.clone();
            if let Some(ice) = candidate {
                Box::pin(async move {
                    if let Err(err) = async move {
                        server_tx.send(HandshakeMessage {
                            msg: Some(Msg::Ice(NotifyIce::from(ice).await?))
                        })?;
                        Ok::<(), Box<dyn Error>>(())
                    }.await {
                        warn!("Could not send new ice candidate to remote. Error: {}", err);
                    }
                })
            } else {
                Box::pin(async move {})
            }
        })).await;

        let pc = Arc::downgrade(&peer_connection);
        let rtp_addr = self.rtp;
        peer_connection.on_track(Box::new(
            move |track: Option<Arc<TrackRemote>>, _receiver: Option<Arc<RTCRtpReceiver>>| {
                if let Some(track) = track {
                    // Send a PLI on an interval so that the publisher is pushing a keyframe every rtcpPLIInterval
                    let media_ssrc = track.ssrc();
                    let pc2 = pc.clone();
                    tokio::spawn(async move {
                        let mut result = Result::<_, webrtc::Error>::Ok(0);
                        while result.is_ok() {
                            let timeout = tokio::time::sleep(Duration::from_secs(3));
                            tokio::pin!(timeout);
    
                            tokio::select! {
                                _ = timeout.as_mut() =>{
                                    if let Some(pc) = pc2.upgrade(){
                                        result = pc.write_rtcp(&[Box::new(PictureLossIndication{
                                            sender_ssrc: 0,
                                            media_ssrc,
                                        })]).await.map_err(Into::into);
                                    }else {
                                        break;
                                    }
                                }
                            };
                        }
                    });
    
                    Box::pin(async move {
                        if let Err(err) = async move {
                            let codec = track.codec().await;
                            let mime_type = codec.capability.mime_type.to_lowercase();
                            if mime_type == MIME_TYPE_H264.to_lowercase() {
                                println!("Got h264 track");
                                let socket = UdpSocket::bind("0.0.0.0:0").await?; // let os alloc port
                                socket.connect(rtp_addr).await?;
                                tokio::spawn(async move {
                                    if let Result::<(), Box<dyn Error>>::Err(err) = async move {
                                        loop {
                                            let (rtp_packet, _attr) = track.read_rtp().await?;
                                            let packet = rtp_packet.marshal()?;
                                            socket.send(&packet).await?;
                                        }
                                    }.await {
                                        warn!("Closing stream after read_rtp error. Error: {}", err);
                                    }
                                });
                            }
                            Result::<(), Box<dyn Error>>::Ok(())
                        }.await {
                            warn!("Could not initiate reading stream. Error: {}", err);
                        }
                    })
                } else {
                    Box::pin(async {})
                }
            },
        )).await;

        signal::ctrl_c().await?; // wait for user termination
        Ok(())
    }
}
