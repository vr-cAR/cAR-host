use std::{sync::Arc, error::Error, net::SocketAddr};

use clap::Args;
use log::warn;
use tokio::net::UdpSocket;
use webrtc::{track::track_local::{TrackLocal, track_local_static_rtp::TrackLocalStaticRTP, TrackLocalWriter}, rtp_transceiver::rtp_codec::RTCRtpCodecCapability, api::media_engine::MIME_TYPE_VP8};

use crate::host::{MediaProvider, Routine};

#[derive(Args, Debug)]
pub struct RtpMediaGenerator {
    rtp_addr: SocketAddr
}

impl MediaProvider for RtpMediaGenerator {
    fn provide(&self) -> Vec<(Arc<dyn TrackLocal + Send + Sync>, Routine)> {
        let mut tracks: Vec<(Arc<dyn TrackLocal + Send + Sync>, Routine)> = Vec::new();

        let video_track = Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_VP8.to_owned(),
                ..Default::default()
            },
            "front".to_owned(),
            "webrtc-rs".to_owned(),
        ));

        let vt = video_track.clone();
        let rtp_addr = self.rtp_addr.clone();
        let routine = Box::pin(async move {
            let video_track = vt;
            let listener = UdpSocket::bind(rtp_addr).await?;
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

        tracks.push((video_track as Arc<dyn TrackLocal + Send + Sync + 'static>, routine));

        tracks
    }
}