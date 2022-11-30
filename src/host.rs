use std::{collections::HashMap, error::Error, fmt::Debug, pin::Pin, sync::Arc};

use log::{debug, error, info, warn};
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    Mutex,
};
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status, Streaming};
use webrtc::{
    api::{interceptor_registry, media_engine::MediaEngine, APIBuilder},
    data_channel::RTCDataChannel,
    ice_transport::{
        ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
        ice_connection_state::RTCIceConnectionState,
        ice_server::RTCIceServer,
    },
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration, offer_answer_options::RTCOfferOptions,
        peer_connection_state::RTCPeerConnectionState,
        policy::ice_transport_policy::RTCIceTransportPolicy,
        sdp::session_description::RTCSessionDescription, RTCPeerConnection,
    },
};

use crate::{
    c_ar::{
        control_server::Control, handshake_message::Msg, HandshakeMessage, HealthCheckReply,
        HealthCheckRequest, NotifyIce,
    },
    media::{MediaProvider, MediaType, RecvChannelParams, Routine},
};

pub struct Host {
    api: webrtc::api::API,
    config: RTCConfiguration,
    providers: Vec<Box<dyn MediaProvider + Send + Sync + 'static>>,
}

impl Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host").finish_non_exhaustive()
    }
}

impl Host {
    pub fn new(
        mut providers: Vec<Box<dyn MediaProvider + Send + Sync + 'static>>,
    ) -> Result<Self, Box<dyn Error>> {
        providers.iter_mut().for_each(|provider| provider.init());

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
            api,
            providers,
        })
    }

    pub fn add_ice_server(mut self, server: RTCIceServer) -> Self {
        self.config.ice_servers.push(server);
        self
    }
}

#[tonic::async_trait]
impl Control for Host {
    type SendHandshakeStream = HeadsetStream;

    async fn health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckReply>, Status> {
        Ok(Response::new(HealthCheckReply {}))
    }

    async fn send_handshake(
        &self,
        request: Request<tonic::Streaming<HandshakeMessage>>, // Accept request of type HelloRequest
    ) -> Result<Response<Self::SendHandshakeStream>, Status> {
        info!("Received handshake request");
        let input_rx = request.into_inner();

        let peer_connection = self
            .api
            .new_peer_connection(self.config.clone())
            .await
            .map_err(|err| {
                error!("Failed to create new peer connection. Error: {}", err);
                Status::internal("Failed to create new peer connection")
            })?;
        let (headset_connection, output_rx) = HeadsetConnection::new(peer_connection);
        info!("Connection creation succeeded");

        // Handle Ice Candidates
        headset_connection.register_on_ice_candidate_handler();

        // Handle Data Channels
        headset_connection.register_on_data_channel_handler();

        // Register tracks
        for provider in &self.providers {
            for media in provider
                .provide(&*headset_connection)
                .await
                .map_err(Status::internal)?
            {
                match media {
                    MediaType::Routine(routine) => headset_connection.add_routine(routine).await,
                    MediaType::Channel(chn) => headset_connection.add_data_channel(chn).await,
                    MediaType::RecvChannel { label, params } => {
                        headset_connection
                            .add_recv_data_channel(label, params)
                            .await
                    }
                }
            }
        }

        // Set the handler for ICE connection state
        // This will notify you when the peer has connected/disconnected
        headset_connection.register_on_ice_connection_state_change_handler();

        // Set the handler for Peer connection state
        // This will notify you when the peer has connected/disconnected
        headset_connection.register_on_peer_connection_state_change_handler();

        // Spawn stream listener
        headset_connection.spawn_on_input_msg_handler(input_rx);

        // create offer
        headset_connection.send_offer().await.map_err(|err| {
            error!("Failed to send offer. Error: {}", err);
            Status::internal(format!("Could not send offer to peer. Error: {}", err))
        })?;
        Ok(Response::new(HeadsetStream(headset_connection, output_rx)))
    }
}

pub struct HeadsetConnection {
    connection: Option<RTCPeerConnection>,
    channels: Mutex<Vec<Arc<RTCDataChannel>>>,
    recv_channels: Mutex<HashMap<String, RecvChannelParams>>,
    output_tx: Sender<Result<HandshakeMessage, Status>>,
    on_start_routines: Mutex<Option<Vec<Routine>>>,
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
                channels: Mutex::new(Vec::new()),
                recv_channels: Mutex::new(HashMap::new()),
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

    pub async fn add_data_channel(&self, chn: Arc<RTCDataChannel>) {
        self.channels.lock().await.push(chn);
    }

    pub async fn add_recv_data_channel(&self, label: String, params: RecvChannelParams) {
        self.recv_channels.lock().await.insert(label, params);
    }

    pub fn register_on_data_channel_handler(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.connection
            .as_ref()
            .unwrap()
            .on_data_channel(Box::new(move |chn| {
                let Some(self_arc) = weak.upgrade() else {
                    info!("Dropping on_data_channel event as connection closed.");
                    return Box::pin(async move {});
                };
                Box::pin(async move {
                    if let Some(params) = self_arc.recv_channels.lock().await.remove(chn.label()) {
                        info!("Got data channel with label {}", chn.label());
                        params.configure_channel(chn.as_ref()).await;
                    } else {
                        warn!("Got data channel with unknown label {}", chn.label());
                    }
                })
            }));
    }

    pub fn register_on_ice_candidate_handler(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.connection
            .as_ref()
            .unwrap()
            .on_ice_candidate(Box::new(move |candidate| {
                let Some(self_arc) = weak.upgrade() else {
                    info!("Dropping on_ice_candidate event since connection closed.");
                    return Box::pin(async move {})
                };
                Box::pin(async move {
                    if let Err(err) = self_arc.on_ice_candidate(candidate).await {
                        error!("Could not notify peer of new ICE candidate. Error: {}", err);
                        self_arc
                            .output_tx
                            .send(Err(Status::from_error(err)))
                            .await
                            .ok();
                    }
                })
            }));
    }

    async fn on_ice_candidate(
        &self,
        candidate: Option<RTCIceCandidate>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(candidate) = candidate {
            debug!("ICE candidate: {}", candidate.to_string());
            let ice_msg = HandshakeMessage {
                msg: Some(Msg::Ice(NotifyIce::from(candidate).await?)),
            };
            self.output_tx.send(Ok(ice_msg)).await?;
        }
        Ok(())
    }

    pub fn spawn_on_input_msg_handler(self: &Arc<Self>, mut input_rx: Streaming<HandshakeMessage>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(msg) = input_rx.next().await {
                match msg {
                    Ok(msg) => {
                        let Some(self_arc) = weak.upgrade() else {
                            info!("Dropping message from peer since connection closed.");
                            continue;
                        };
                        if let Err(err) = self_arc.on_input_msg(msg).await {
                            error!(
                                "Could not handle message received from peer. Error: {}",
                                err
                            );
                            self_arc
                                .output_tx
                                .send(Err(Status::from_error(err)))
                                .await
                                .ok();
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
                    let remote_description: RTCSessionDescription = description.into();
                    debug!(
                        "Got description from peer. Description: {}",
                        serde_json::to_string(&remote_description)?
                    );

                    self.connection
                        .as_ref()
                        .unwrap()
                        .set_remote_description(remote_description)
                        .await?;
                    info!("Remote description successfully set");

                    let mut guard = self.cached_ice_candidates.lock().await;
                    for candidate in guard.take().unwrap() {
                        debug!("Adding ice candidate {}", candidate.candidate);
                        self.connection
                            .as_ref()
                            .unwrap()
                            .add_ice_candidate(candidate)
                            .await?;
                    }
                }
                Msg::Ice(ice) => {
                    debug!("Got ice candidate from peer.");
                    let candidate = ice.try_into()?;
                    let mut guard = self.cached_ice_candidates.lock().await;
                    if let Some(candidates) = guard.as_mut() {
                        candidates.push(candidate);
                    } else {
                        std::mem::drop(guard);
                        debug!("Adding ice candidate {}", candidate.candidate);
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

    pub fn register_on_ice_connection_state_change_handler(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.connection
            .as_ref()
            .unwrap()
            .on_ice_connection_state_change(Box::new(move |candidate| {
                let Some(self_arc) = weak.upgrade() else {
                    info!("Dropping on_ice_connection_state_change event as connection closed.");
                    return Box::pin(async move {});
                };
                Box::pin(async move {
                    if let Err(err) = self_arc.on_ice_connection_state_change(candidate).await {
                        error!(
                            "Could not handle ICE connection state change. Error: {}",
                            err
                        );
                        self_arc
                            .output_tx
                            .send(Err(Status::from_error(err)))
                            .await
                            .ok();
                    }
                })
            }));
    }

    async fn on_ice_connection_state_change(
        &self,
        state: RTCIceConnectionState,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        debug!("Connection state has changed {}", state);
        if state == RTCIceConnectionState::Connected {
            // fire routines
            let mut guard = self.on_start_routines.lock().await;
            if let Some(routines) = guard.take() {
                for routine in routines {
                    spawn_routine(routine);
                }
            }

            // register cached ICE candidates
            let mut guard = self.cached_ice_candidates.lock().await;
            if let Some(candidates) = guard.take() {
                for candidate in candidates {
                    debug!("Adding ice candidate {}", candidate.candidate);
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

    pub fn register_on_peer_connection_state_change_handler(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.connection
            .as_ref()
            .unwrap()
            .on_peer_connection_state_change(Box::new(move |candidate| {
                let Some(self_arc) = weak.upgrade() else {
                    info!("Dropping on_peer_connection_state_change event as connection closed.");
                    return Box::pin(async move {});
                };
                Box::pin(async move {
                    if let Err(err) = self_arc.on_peer_connection_state_change(candidate).await {
                        error!(
                            "Could not handle peer connection state change. Error: {}",
                            err
                        );
                        self_arc
                            .output_tx
                            .send(Err(Status::from_error(err)))
                            .await
                            .ok();
                    }
                })
            }));
    }

    async fn on_peer_connection_state_change(
        &self,
        state: RTCPeerConnectionState,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        debug!("Peer connection state has changed {}", state);
        match state {
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                info!("Connection failed or closed");
            }
            _ => {}
        };
        Ok(())
    }

    pub async fn add_routine(&self, routine: Routine) {
        let mut guard = self.on_start_routines.lock().await;
        if let Some(routines) = guard.as_mut() {
            routines.push(routine);
        } else {
            std::mem::drop(guard);
            spawn_routine(routine);
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
        let msg = HandshakeMessage {
            msg: Some(Msg::Description(offer.clone().into())),
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

impl AsRef<RTCPeerConnection> for HeadsetConnection {
    fn as_ref(&self) -> &RTCPeerConnection {
        self.connection()
    }
}

impl Drop for HeadsetConnection {
    fn drop(&mut self) {
        let connection = self.connection.take().unwrap();
        tokio::spawn(async move {
            info!("Closing connection");
            if let Err(err) = connection.close().await {
                error!("Failed to close connection. Error: {}", err);
            }
        });
    }
}

pub struct HeadsetStream(
    Arc<HeadsetConnection>,
    Receiver<Result<HandshakeMessage, Status>>,
);

impl tokio_stream::Stream for HeadsetStream {
    type Item = Result<HandshakeMessage, Status>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.1.poll_recv(cx)
    }
}

fn spawn_routine(routine: Routine) {
    tokio::spawn(async move {
        if let Err(err) = routine.await {
            warn!("Routine exited with error. Error: {}", err);
        }
    });
}
