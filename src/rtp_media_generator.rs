use std::{error::Error, net::SocketAddr, pin::Pin, sync::Arc};

use clap::Args;
use log::{info, warn, trace};
use prost::Message;
use rosrust_msg::std_msgs::Float32MultiArray;
use std::future::Future;
use tokio::{net::UdpSocket, sync::mpsc};
use webrtc::{
    api::media_engine::MIME_TYPE_H264,
    data_channel::{
        data_channel_init::RTCDataChannelInit, OnCloseHdlrFn, OnMessageHdlrFn, OnOpenHdlrFn,
        RTCDataChannel,
    },
    error::OnErrorHdlrFn,
    peer_connection::RTCPeerConnection,
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{track_local_static_rtp::TrackLocalStaticRTP, TrackLocalWriter},
};

use crate::{
    c_ar_controls::ThumbstickDirection,
    host::{MediaProvider, MediaType, RecvChannelParams, Routine},
};

#[derive(Args, Debug)]
pub struct RtpMediaGenerator {
    rtp_addr: SocketAddr,
    ros_node: String,
    ros_topic: String,
    #[clap(short, long, default_value_t=5)]
    queue_size: usize,
}

async fn spawn_rtp_server(
    id: &str,
    conn: &RTCPeerConnection,
    addr: SocketAddr,
) -> Result<Routine, String> {
    let video_track = Arc::new(TrackLocalStaticRTP::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            ..Default::default()
        },
        id.to_owned(),
        "webrtc-rs".to_owned(),
    ));

    let rtp_sender = conn
        .add_track(video_track.clone())
        .await
        .map_err(|_err| "Failed to add tracks")?;

    // Read incoming RTCP packets
    // Before these packets are returned they are processed by interceptors. For things
    // like NACK this needs to be called.
    tokio::spawn(async move {
        let mut rtcp_buf = vec![0u8; 1500];
        while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
    });

    let routine = Box::pin(async move {
        let listener = UdpSocket::bind(addr).await?;
        let mut inbound_rtp_packet = vec![0u8; 1600]; // UDP MTU
        while let Ok((n, _)) = listener.recv_from(&mut inbound_rtp_packet).await {
            if let Err(err) = video_track.write(&inbound_rtp_packet[..n]).await {
                warn!("Failed to write rtp packet. Error: {}", err);
                if webrtc::Error::ErrClosedPipe == err {
                    return Ok(());
                } else {
                    Err(err)?;
                }
            }
        }
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    });
    Ok(routine)
}

async fn configure_controls_channel(
    label: &str,
    ros_topic: String,
    ros_queue_sz: usize,
    conn: &RTCPeerConnection,
) -> Result<Arc<RTCDataChannel>, String> {
    let l = label.to_owned();
    let on_open: Option<OnOpenHdlrFn> = Some(Box::new(move || {
        let label = l;
        Box::pin(async move {
            info!("Data channel {} opened", label);
        })
    }));
    let l = label.to_owned();
    let on_error: Option<OnErrorHdlrFn> = Some(Box::new(move |err| {
        let label = l.clone();
        Box::pin(async move {
            warn!("Data channel {} has error. Error: {}", label, err);
        })
    }));
    let l = label.to_owned();
    let on_close: Option<OnCloseHdlrFn> = Some(Box::new(move || {
        let label = l.clone();
        Box::pin(async move {
            info!("Data channel {} closed", label);
        })
    }));

    let (controls_tx, mut controls_rx) = mpsc::channel(5);
    let on_msg: Option<OnMessageHdlrFn> = Some(Box::new(move |msg| {
        let Ok(msg) = ThumbstickDirection::decode_length_delimited(msg.data) else {
            warn!("Failed to parse message from controls channel");
            return Box::pin(async {});
        };

        let tx = controls_tx.clone();
        Box::pin(async move {
            if let Err(err) = tx.send(msg).await {
                warn!("Could not send controller update. Error: {}", err);
            }
        })
    }));

    tokio::spawn(async move {
        let publisher = match rosrust::publish(&ros_topic, ros_queue_sz) {
            Ok(publisher) => Some(publisher),
            Err(err) => {
                warn!("Failed to create ROS publisher. Error: {}", err);
                None
            } 
        };
        let mut acc = i64::MIN;
        while let Some(msg) = controls_rx.recv().await {
            if msg.seq_num <= acc {
                continue;
            }

            acc = msg.seq_num;
            trace!("Thumbstick Position: dx={}, dy={}", msg.dx, msg.dy);
            let dir = nalgebra::SVector::from([msg.dx, msg.dy]);
            if let Some(publisher) = publisher.as_ref() {
                let speed = dir.norm() * if dir[1] < 0f64 { -1f64 } else { 1f64 } ;

                let msg = Float32MultiArray {
                    data: vec![speed as f32, -dir[0] as f32],
                    ..Default::default()
                };
                if let Err(err) = publisher.send(msg) {
                    warn!("Failed to send turn vector to ROS. Error: {}", err);
                }
            }
        }
    });

    let params = RecvChannelParams {
        on_open,
        on_close,
        on_msg,
        on_error,
    };

    let chn = conn
        .create_data_channel(
            label,
            Some(RTCDataChannelInit {
                ordered: Some(false),
                max_retransmits: Some(0),
                ..Default::default()
            }),
        )
        .await
        .map_err(|_err| "Failed to create data channel")?;
    params.configure_channel(chn.as_ref()).await;

    Ok(chn)
}

impl MediaProvider for RtpMediaGenerator {
    fn init(&mut self) {
        rosrust::init(&self.ros_node);
    }

    fn provide<R>(
        &self,
        conn: Arc<R>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MediaType>, String>> + Send>>
    where
        R: AsRef<RTCPeerConnection> + Send + Sync + 'static,
    {
        let rtp_addr = self.rtp_addr;
        let ros_topic = self.ros_topic.clone();
        let queue_size = self.queue_size;
        Box::pin(async move {
            let mut media: Vec<MediaType> = Vec::new();

            info!("Adding front camera feed");
            media.push(MediaType::Routine(
                spawn_rtp_server("front", conn.as_ref().as_ref(), rtp_addr).await?,
            ));
            info!("Adding controls channel");
            media.push(MediaType::Channel(
                configure_controls_channel("controls", ros_topic, queue_size, conn.as_ref().as_ref()).await?,
            ));

            Ok(media)
        })
    }
}
