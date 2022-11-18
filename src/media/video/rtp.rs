use std::{error::Error, future::Future, net::SocketAddr, pin::Pin, sync::Arc};

use log::{info, warn};
use tokio::net::UdpSocket;
use webrtc::{
    api::media_engine::MIME_TYPE_H264,
    peer_connection::RTCPeerConnection,
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{track_local_static_rtp::TrackLocalStaticRTP, TrackLocalWriter},
};

use crate::media::{Routine, MediaProvider, MediaType};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct RtpMediaProvider {
    rtp_addr: SocketAddr,
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

impl MediaProvider for RtpMediaProvider {
    fn provide(
        &self,
        conn: Arc<dyn AsRef<RTCPeerConnection> + Send + Sync>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MediaType>, String>> + Send>> {
        let rtp_addr = self.rtp_addr;
        Box::pin(async move {
            let mut media: Vec<MediaType> = Vec::new();
            info!("Adding front camera feed");
            media.push(MediaType::Routine(
                spawn_rtp_server("front", (*conn).as_ref(), rtp_addr).await?,
            ));

            Ok(media)
        })
    }
}
