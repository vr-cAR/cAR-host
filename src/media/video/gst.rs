use std::{error::Error, future::Future, pin::Pin, sync::Arc};

use gstreamer::{element_error, prelude::*, Caps, MessageView};
use gstreamer_app::AppSinkCallbacks;
use log::{error, info, trace, warn};
use tokio::sync::mpsc;
use webrtc::{
    api::media_engine,
    peer_connection::RTCPeerConnection,
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{track_local_static_rtp::TrackLocalStaticRTP, TrackLocalWriter},
};

use crate::media::{MediaProvider, MediaType, Routine};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct GstMediaProvider {}

async fn spawn_rtp_server(id: &str, conn: &RTCPeerConnection) -> Result<Routine, String> {
    let video_track = Arc::new(TrackLocalStaticRTP::new(
        RTCRtpCodecCapability {
            mime_type: media_engine::MIME_TYPE_H264.to_owned(),
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

    let (buffer_tx, mut buffer_rx) = mpsc::channel(1);

    // build gstreamer pipeline
    let pipeline = gstreamer::Pipeline::default();
    let src = gstreamer::ElementFactory::make("autovideosrc")
        .property("sync", false)
        .build()
        .unwrap();
    let crop = gstreamer::ElementFactory::make("aspectratiocrop")
        .property("aspect-ratio", gstreamer::Fraction::new(1, 1))
        .build()
        .unwrap();
    let convert = gstreamer::ElementFactory::make("videoconvert")
        .build()
        .unwrap();
    let encode = gstreamer::ElementFactory::make("x264enc")
        .property_from_str("tune", "zerolatency")
        .property_from_str("speed-preset", "ultrafast")
        .build()
        .unwrap();
    let rtp = gstreamer::ElementFactory::make("rtph264pay")
        .build()
        .unwrap();
    let appsink = gstreamer_app::AppSink::builder().async_(true).build();
    appsink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                let buffer = sample.buffer_owned().ok_or_else(|| {
                    element_error!(
                        sink,
                        gstreamer::ResourceError::Failed,
                        ("Failed to get buffer from appsink")
                    );

                    gstreamer::FlowError::Error
                })?;
                let map = buffer.into_mapped_buffer_readable().map_err(|_| {
                    element_error!(
                        sink,
                        gstreamer::ResourceError::Failed,
                        ("Failed to map buffer readable")
                    );

                    gstreamer::FlowError::Error
                })?;
                buffer_tx.blocking_send(map).map_err(|_| {
                    element_error!(sink, gstreamer::ResourceError::Close, ("Endpoint closed"));
                    gstreamer::FlowError::Error
                })?;

                Ok(gstreamer::FlowSuccess::Ok)
            })
            .build(),
    );

    if let Err(err) = (|| {
        pipeline.add_many(&[&src, &crop, &convert, &encode, &rtp, appsink.upcast_ref()])?;
        src.link(&crop)?;
        crop.link(&convert)?;
        convert.link_filtered(&encode, &Caps::builder("video/x-raw").build())?;
        encode.link(&rtp)?;
        rtp.link(&appsink)?;
        Result::<_, Box<dyn Error>>::Ok(())
    })() {
        warn!("Could not construct pipeline. Error: {}", err);
        Err("Could not construct pipeline.")?;
    }

    let routine = Box::pin(async move {
        let bus = pipeline.bus().unwrap();
        bus.add_watch_local(move |_, msg| {
            match msg.view() {
                MessageView::Eos(..) => {
                    warn!("Stream ended");
                }
                MessageView::Error(err) => {
                    warn!(
                        "Stream threw error. Error from {:?}: {} ({:?})",
                        err.src().map(|s| s.path_string()),
                        err.error(),
                        err.debug()
                    );
                }
                _ => (),
            };

            glib::Continue(true)
        })?;
        pipeline.set_state(gstreamer::State::Playing)?;

        info!("Sending RTP packets");
        while let Some(buffer) = buffer_rx.recv().await {
            if let Err(err) = video_track.write(buffer.as_slice()).await {
                warn!("Failed to write rtp packet. Error: {}", err);
                if webrtc::Error::ErrClosedPipe == err {
                    return Ok(());
                } else {
                    Err(err)?;
                }
                unreachable!();
            }
            trace!("Sent RTP packet of size: {}", buffer.as_slice().len());
        }

        pipeline.set_state(gstreamer::State::Null)?;
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    });
    Ok(routine)
}

impl MediaProvider for GstMediaProvider {
    fn init(&mut self) {
        if let Err(err) = gstreamer::init() {
            error!("Could not initialize gstreamer. Error: {}", err);
            panic!()
        }
    }

    fn provide(
        &self,
        conn: Arc<dyn AsRef<RTCPeerConnection> + Send + Sync>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MediaType>, String>> + Send>> {
        Box::pin(async move {
            let mut media: Vec<MediaType> = Vec::new();
            info!("Adding front camera feed");
            media.push(MediaType::Routine(
                spawn_rtp_server("front", (*conn).as_ref()).await?,
            ));
            Ok(media)
        })
    }
}
