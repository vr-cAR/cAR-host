#[cfg(feature = "ros")]
pub mod ros;
pub mod txt;

use std::{error::Error, future::Future, pin::Pin, sync::Arc};

use log::{info, warn};
use prost::Message;
use tokio::sync::mpsc;
use webrtc::{
    data_channel::{
        data_channel_init::RTCDataChannelInit, OnCloseHdlrFn, OnMessageHdlrFn, OnOpenHdlrFn,
        RTCDataChannel,
    },
    error::OnErrorHdlrFn,
    peer_connection::RTCPeerConnection,
};

use crate::c_ar_controls::ThumbstickDirection;

use super::{MediaProvider, MediaType, RecvChannelParams};

pub struct ControlsMediaProvider<F>
where
    F: ControlsReceiverFactory,
{
    factory: F,
}

pub trait ControlsReceiver {
    fn recv(
        &mut self,
        controls: ThumbstickDirection,
    ) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn Error>>> + '_>>;
}

pub trait ControlsReceiverFactory {
    type Receiver: ControlsReceiver;
    fn create_receiver(&self) -> Self::Receiver;
}

impl<F> ControlsMediaProvider<F>
where
    F: ControlsReceiverFactory,
{
    pub fn new(factory: F) -> Self {
        Self { factory }
    }
}

impl<F> ControlsMediaProvider<F>
where
    F: ControlsReceiverFactory + Send + Sync + 'static,
    F::Receiver: Send + Sync + 'static,
{
    async fn configure_controls_channel(
        &self,
        label: &str,
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

        let mut receiver = self.factory.create_receiver();
        tokio::spawn(async move {
            while let Some(msg) = controls_rx.recv().await {
                receiver.recv(msg);
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
}

impl<F> MediaProvider for ControlsMediaProvider<F>
where
    F: ControlsReceiverFactory + Send + Sync + 'static,
    F::Receiver: Send + Sync + 'static,
{
    fn init(&mut self) {}

    fn provide(
        &self,
        conn: Arc<dyn AsRef<RTCPeerConnection> + Send + Sync>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MediaType>, String>> + Send + '_>> {
        Box::pin(async move {
            let mut media: Vec<MediaType> = Vec::new();

            info!("Adding controls channel");
            media.push(MediaType::Channel(
                self.configure_controls_channel("controls", (*conn).as_ref())
                    .await?,
            ));

            Ok(media)
        })
    }
}
