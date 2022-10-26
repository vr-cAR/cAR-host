use std::{
    error::Error, fmt::Debug, future::Future, io::Read, pin::Pin, sync::Arc, time::Duration,
};

use log::{debug, error, info, trace, warn};
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    Mutex,
};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tonic::{Request, Response, Status, Streaming};
use webrtc::{
    api::{
        interceptor_registry,
        media_engine::{self, MediaEngine},
        APIBuilder,
    },
    ice_transport::{
        ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
        ice_connection_state::RTCIceConnectionState,
        ice_server::RTCIceServer,
    },
    interceptor::registry::Registry,
    media::{io::ivf_reader::IVFReader, Sample},
    peer_connection::{
        configuration::RTCConfiguration,
        offer_answer_options::RTCOfferOptions,
        peer_connection_state::RTCPeerConnectionState,
        sdp::{sdp_type::RTCSdpType, session_description::RTCSessionDescription},
        RTCPeerConnection, policy::ice_transport_policy::RTCIceTransportPolicy,
    },
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{track_local_static_sample::TrackLocalStaticSample, TrackLocal},
};

use crate::c_ar::{
    control_server::Control, handshake_message::Msg, HandshakeMessage, NotifyDescription,
    NotifyIce, SdpType,
};

pub trait StreamGenerator {
    type Stream: Read;
    type Err: Error;
    fn new_stream(&self) -> Result<Self::Stream, Self::Err>;
}

pub struct Host<SG>
where
    SG: StreamGenerator,
{
    api: webrtc::api::API,
    ivf_generator: Arc<SG>,
    config: RTCConfiguration,
}

impl<SG> Debug for Host<SG>
where
    SG: StreamGenerator + Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("ivf_generator", &self.ivf_generator)
            .finish_non_exhaustive()
    }
}

impl<SG> Host<SG>
where
    SG: StreamGenerator,
{
    pub fn new(gen: SG) -> Result<Self, Box<dyn Error>> {
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs()?;

        let registry = interceptor_registry::register_default_interceptors(
            Registry::new(),
            &mut media_engine,
        )?;

        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();

        let config = RTCConfiguration {
            ice_transport_policy: RTCIceTransportPolicy::All,
            ..Default::default()
        };
        Ok(Self {
            config,
            ivf_generator: Arc::new(gen),
            api,
        })
    }

    pub fn add_ice_server(mut self, server: RTCIceServer) -> Self {
        self.config.ice_servers.push(server);
        self
    }
}

#[tonic::async_trait]
impl<SG> Control for Host<SG>
where
    SG: StreamGenerator + Send + Sync + 'static,
    SG::Stream: Read + Send + Sync + 'static,
{
    type SendHandshakeStream = ReceiverStream<Result<HandshakeMessage, Status>>;

    async fn send_handshake(
        &self,
        request: Request<tonic::Streaming<HandshakeMessage>>, // Accept request of type HelloRequest
    ) -> Result<Response<Self::SendHandshakeStream>, Status> {
        info!("Received handshake request");
        let input_rx = request.into_inner();

        let peer_connection = self.api.new_peer_connection(self.config.clone()).await.map_err(|err| {
            Status::internal(format!(
                "Failed to create new peer connection. Error: {}",
                err
            ))
        })?;
        let (headset_connection, output_rx) = HeadsetConnection::new(peer_connection);
        info!("Connection creation succeeded");

        // send ice servers
        headset_connection
            .clone()
            .register_on_ice_candidate_handler()
            .await;

        // add video track
        let video_track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_VP9.to_owned(),
                ..Default::default()
            },
            "front".to_owned(),
            "webrtc-rs".to_owned(),
        ));

        let rtp_sender = headset_connection
            .connection()
            .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|_err| Status::internal("Failed to create video track"))?;
        // Read incoming RTCP packets
        // Before these packets are returned they are processed by interceptors. For things
        // like NACK this needs to be called.
        tokio::spawn(async move {
            let mut rtcp_buf = vec![0u8; 1500];
            while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
        });

        let generator = self.ivf_generator.clone();
        headset_connection
            .clone()
            .add_routine(Box::new(move |_headset_connection| {
                Box::pin(async move {
                    debug!("Starting video stream");

                    // initiate stream
                    let stream = generator.new_stream().map_err(|err| {
                        Status::internal(format!("Failed to create stream. Error: {}", err))
                    })?;
                    // Open a IVF file and start reading using our IVFReader
                    let (mut ivf, header) = IVFReader::new(stream)?;

                    // It is important to use a time.Ticker instead of time.Sleep because
                    // * avoids accumulating skew, just calling time.Sleep didn't compensate for the time spent parsing the data
                    // * works around latency issues with Sleep
                    // Send our video file frame at a time. Pace our sending so we send it at the same speed it should be played back as.
                    // This isn't required since the video is timestamped, but we will such much higher loss if we send all at once.
                    let sleep_time = Duration::from_millis(
                        ((1000 * header.timebase_numerator) / header.timebase_denominator) as u64,
                    );
                    let mut ticker = tokio::time::interval(sleep_time);
                    loop {
                        let (frame, _header) = match ivf.parse_next_frame() {
                            Ok(pair) => pair,
                            Err(err) => {
                                warn!("Video stream ended. Error: {}", err);
                                break;
                            }
                        };

                        trace!("Sending next frame.");

                        video_track
                            .write_sample(&Sample {
                                data: frame.freeze(),
                                duration: Duration::from_secs(1),
                                ..Default::default()
                            })
                            .await?;

                        let _ = ticker.tick().await;
                    }
                    Ok(())
                })
            }))
            .await;

        // Set the handler for ICE connection state
        // This will notify you when the peer has connected/disconnected
        headset_connection
            .clone()
            .register_on_ice_connection_state_change_handler()
            .await;

        // Set the handler for Peer connection state
        // This will notify you when the peer has connected/disconnected
        headset_connection
            .clone()
            .register_on_peer_connection_state_change_handler()
            .await;

        // spawn stream listener
        headset_connection
            .clone()
            .spawn_on_input_msg_handler(input_rx)
            .await;

        // create offer
        headset_connection.send_offer().await.map_err(|err| {
            Status::internal(format!("Could not send offer to peer. Error: {}", err))
        })?;

        Ok(Response::new(ReceiverStream::new(output_rx)))
    }
}

type Routine<T> = Box<
    dyn FnOnce(T) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn Error + Send + Sync>>> + Send>>
        + Send
        + Sync,
>;

struct HeadsetConnection {
    connection: Option<RTCPeerConnection>,
    output_tx: Sender<Result<HandshakeMessage, Status>>,
    on_start_routines: Mutex<Option<Vec<Routine<Arc<Self>>>>>,
    cached_ice_candidates: Mutex<Option<Vec<RTCIceCandidateInit>>>,
}

impl HeadsetConnection {
    const QUEUED_MSGS: usize = 8;

    pub fn new(
        connection: RTCPeerConnection,
    ) -> (Arc<Self>, Receiver<Result<HandshakeMessage, Status>>) {
        let (tx, rx) = mpsc::channel(Self::QUEUED_MSGS);

        (
            Arc::new(Self {
                connection: Some(connection),
                output_tx: tx,
                on_start_routines: Mutex::new(Some(Vec::new())),
                cached_ice_candidates: Mutex::new(Some(Vec::new())),
            }),
            rx,
        )
    }

    pub fn connection(&self) -> &RTCPeerConnection {
        self.connection.as_ref().unwrap()
    }

    pub async fn register_on_ice_candidate_handler(self: Arc<Self>) {
        let cloned = self.clone();
        self.connection
            .as_ref()
            .unwrap()
            .on_ice_candidate(Box::new(move |candidate| {
                let cloned = cloned.clone();
                Box::pin(async move {
                    if let Err(err) = cloned.on_ice_candidate(candidate).await {
                        error!("Could not notify peer of new ICE candidate. Error: {}", err);
                        cloned
                            .output_tx
                            .send(Err(Status::from_error(err)))
                            .await
                            .ok();
                    }
                })
            }))
            .await;
    }

    async fn on_ice_candidate(
        &self,
        candidate: Option<RTCIceCandidate>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(candidate) = candidate {
            debug!("ICE candidate: {}", candidate.to_string());
            let json_base64 = base64::encode(serde_json::to_string(&candidate.to_json().await?)?);
            let ice_msg = HandshakeMessage {
                msg: Some(Msg::Ice(NotifyIce { json_base64 })),
            };
            self.output_tx.send(Ok(ice_msg)).await?;
        }
        Ok(())
    }

    pub async fn spawn_on_input_msg_handler(
        self: Arc<Self>,
        mut input_rx: Streaming<HandshakeMessage>,
    ) {
        tokio::spawn(async move {
            while let Some(msg) = input_rx.next().await {
                match msg {
                    Ok(msg) => {
                        if let Err(err) = self.on_input_msg(msg).await {
                            error!(
                                "Could not handle message received from peer. Error: {}",
                                err
                            );
                            self.output_tx.send(Err(Status::from_error(err))).await.ok();
                        }
                    }
                    Err(err) => {
                        warn!("Received error from peer. Error: {}", err);
                    }
                }
            }
        });
    }

    async fn on_input_msg(
        &self,
        msg: HandshakeMessage,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(msg) = msg.msg {
            match msg {
                Msg::Description(description) => {
                    let mut remote_description = RTCSessionDescription::default();
                    remote_description.sdp_type = match description.sdp_type() {
                        SdpType::Offer => RTCSdpType::Offer,
                        SdpType::Pranswer => RTCSdpType::Pranswer,
                        SdpType::Answer => RTCSdpType::Answer,
                        SdpType::Rollback => RTCSdpType::Rollback,
                        SdpType::Unspecified => RTCSdpType::Unspecified,
                    };
                    remote_description.sdp = description.sdp;
                    debug!(
                        "Got description from peer. Description: {}",
                        serde_json::to_string(&remote_description)?
                    );

                    self.connection
                        .as_ref()
                        .unwrap()
                        .set_remote_description(remote_description)
                        .await?;
                    info!("Connection established");
                }
                Msg::Ice(ice) => {
                    debug!("Got ice candidate from peer.");
                    let candidate = serde_json::from_str::<RTCIceCandidateInit>(
                        &String::from_utf8(base64::decode(ice.json_base64)?)?,
                    )?;
                    let mut guard = self.cached_ice_candidates.lock().await;
                    if let Some(candidates) = guard.as_mut() {
                        candidates.push(candidate);
                    } else {
                        std::mem::drop(guard);
                        self.connection
                            .as_ref()
                            .unwrap()
                            .add_ice_candidate(candidate)
                            .await?;
                    }
                }
            }
            Ok(())
        } else {
            warn!("Received empty message from peer.");
            Err(Box::new(Status::invalid_argument(
                "Received empty message from peer.",
            )))
        }
    }

    pub async fn register_on_ice_connection_state_change_handler(self: Arc<Self>) {
        let cloned = self.clone();
        self.connection
            .as_ref()
            .unwrap()
            .on_ice_connection_state_change(Box::new(move |candidate| {
                let cloned = cloned.clone();
                Box::pin(async move {
                    if let Err(err) = cloned
                        .clone()
                        .on_ice_connection_state_change(candidate)
                        .await
                    {
                        error!(
                            "Could not handle ICE connection state change. Error: {}",
                            err
                        );
                        cloned
                            .output_tx
                            .send(Err(Status::from_error(err)))
                            .await
                            .ok();
                    }
                })
            }))
            .await;
    }

    fn fire_routine(self: Arc<Self>, routine: Routine<Arc<Self>>) {
        tokio::spawn(async move {
            if let Err(err) = routine(self.clone()).await {
                error!("Routine threw error. Error: {}", err);
                self.output_tx.send(Err(Status::from_error(err))).await.ok();
            }
        });
    }

    async fn on_ice_connection_state_change(
        self: Arc<Self>,
        state: RTCIceConnectionState,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        debug!("Connection state has changed {}", state);
        if state == RTCIceConnectionState::Connected {
            // fire routines
            let mut guard = self.on_start_routines.lock().await;
            if let Some(routines) = guard.take() {
                for routine in routines {
                    self.clone().fire_routine(routine);
                }
            }

            // register cached ICE candidates
            let mut guard = self.cached_ice_candidates.lock().await;
            if let Some(candidates) = guard.take() {
                for candidate in candidates {
                    self.connection
                        .as_ref()
                        .unwrap()
                        .add_ice_candidate(candidate)
                        .await?;
                }
            }
        }

        Ok(())
    }

    pub async fn register_on_peer_connection_state_change_handler(self: Arc<Self>) {
        let cloned = self.clone();
        self.connection
            .as_ref()
            .unwrap()
            .on_peer_connection_state_change(Box::new(move |candidate| {
                let cloned = cloned.clone();
                Box::pin(async move {
                    if let Err(err) = cloned.on_peer_connection_state_change(candidate).await {
                        error!(
                            "Could not handle peer connection state change. Error: {}",
                            err
                        );
                        cloned
                            .output_tx
                            .send(Err(Status::from_error(err)))
                            .await
                            .ok();
                    }
                })
            }))
            .await;
    }

    async fn on_peer_connection_state_change(
        &self,
        state: RTCPeerConnectionState,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        debug!("Peer connection state has changed {}", state);
        if state == RTCPeerConnectionState::Disconnected {
            // TODO: do something
            warn!("Peer connection disconnected");
        }

        Ok(())
    }

    pub async fn add_routine(self: Arc<Self>, routine: Routine<Arc<Self>>) {
        let mut guard = self.on_start_routines.lock().await;
        if let Some(routines) = guard.as_mut() {
            routines.push(routine);
        } else {
            std::mem::drop(guard);
            self.fire_routine(routine);
        }
    }

    pub async fn send_offer(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let offer_options = RTCOfferOptions {
            ice_restart: false,
            ..RTCOfferOptions::default()
        };
        let offer = self
            .connection
            .as_ref()
            .unwrap()
            .create_offer(Some(offer_options))
            .await?;
        let offer_json = serde_json::to_string(&offer)?;
        debug!("Created offer: {}", offer_json);

        // create message from offer
        let sdp_type = match offer.sdp_type {
            RTCSdpType::Unspecified => SdpType::Unspecified,
            RTCSdpType::Offer => SdpType::Offer,
            RTCSdpType::Pranswer => SdpType::Pranswer,
            RTCSdpType::Answer => SdpType::Answer,
            RTCSdpType::Rollback => SdpType::Rollback,
        };
        let mut msg = NotifyDescription::default();
        msg.set_sdp_type(sdp_type);
        msg.sdp = offer.sdp.clone();

        let msg = HandshakeMessage {
            msg: Some(Msg::Description(msg)),
        };

        // set local description to offer
        self.connection
            .as_ref()
            .unwrap()
            .set_local_description(offer)
            .await?;

        self.output_tx.send(Ok(msg)).await?;
        Ok(())
    }
}

impl Drop for HeadsetConnection {
    fn drop(&mut self) {
        let connection = self.connection.take().unwrap();
        tokio::spawn(async move {
            if let Err(err) = connection.close().await {
                error!("Failed to close connection. Error: {}", err);
            }
        });
    }
}
