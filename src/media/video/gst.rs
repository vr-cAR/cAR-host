use std::{collections::HashMap, error::Error, future::Future, pin::Pin, sync::Arc};

use gstreamer::{element_error, prelude::*, MessageView};
use gstreamer_app::AppSinkCallbacks;
use log::{error, info, trace, warn};
use tokio::sync::mpsc;
use webrtc::{
    peer_connection::RTCPeerConnection,
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{track_local_static_rtp::TrackLocalStaticRTP, TrackLocalWriter},
};

use crate::media::{MediaProvider, MediaType, Routine};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GstMediaProvider {
    mime_type: String,
    elements: Vec<Element>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Element {
    name: String,
    #[serde(default)]
    properties: HashMap<String, Value>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Value {
    String(String),
    Int(u64),
    Float(f64),
    Bool(bool),
}

impl ToString for Value {
    fn to_string(&self) -> String {
        match self {
            Value::String(str) => str.clone(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bool(b) => b.to_string(),
        }
    }
}

impl GstMediaProvider {
    async fn spawn_rtp_server(self, id: &str, conn: &RTCPeerConnection) -> Result<Routine, String> {
        let video_track = Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: self.mime_type,
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
        let mut elems = Vec::new();
        if let Err(err) = (|| {
            // add elements to chain
            for Element { name, properties } in self.elements {
                let properties: HashMap<_, _> = properties
                    .into_iter()
                    .map(|(key, val)| (key, val.to_string()))
                    .collect();
                let mut builder = gstreamer::ElementFactory::make(&name);
                for (name, value) in &properties {
                    builder = builder.property_from_str(name, value);
                }
                elems.push(builder.build()?);
            }

            // create rtp packet sink
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
                            element_error!(
                                sink,
                                gstreamer::ResourceError::Close,
                                ("Endpoint closed")
                            );
                            gstreamer::FlowError::Error
                        })?;

                        Ok(gstreamer::FlowSuccess::Ok)
                    })
                    .build(),
            );

            // add elements to pipeline
            let mut elems_ref: Vec<_> = elems.iter().collect();
            elems_ref.push(appsink.upcast_ref());
            pipeline.add_many(&elems_ref)?;
            for elem_idx in 1..elems_ref.len() {
                elems_ref[elem_idx - 1].link(elems_ref[elem_idx])?;
            }
            Result::<_, Box<dyn Error>>::Ok(())
        })() {
            warn!("Failed to construct pipeline. Error: {}.", err);
            return Err("Failed to construct pipeline".to_owned());
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

            info!("Sending RTP packets");
            pipeline.set_state(gstreamer::State::Playing)?;
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
        let self_cloned = self.clone();
        Box::pin(async move {
            let mut media: Vec<MediaType> = Vec::new();
            info!("Adding front camera feed");
            media.push(MediaType::Routine(
                self_cloned
                    .spawn_rtp_server("front", (*conn).as_ref())
                    .await?,
            ));
            Ok(media)
        })
    }
}
