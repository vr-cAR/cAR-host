pub mod controls;
pub mod video;

use std::{error::Error, future::Future, pin::Pin, sync::Arc};

use webrtc::{
    data_channel::{OnCloseHdlrFn, OnMessageHdlrFn, OnOpenHdlrFn, RTCDataChannel},
    error::OnErrorHdlrFn,
    peer_connection::RTCPeerConnection,
};

pub trait MediaProvider {
    fn init(&mut self) {}

    fn provide<'a>(
        &'a self,
        _conn: &'a (dyn AsRef<RTCPeerConnection> + Send + Sync),
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MediaType>, String>> + Send + 'a>> {
        Box::pin(async { Ok(vec![]) })
    }
}

pub enum MediaType {
    Routine(Routine),
    Channel(Arc<RTCDataChannel>),
    RecvChannel {
        label: String,
        params: RecvChannelParams,
    },
}

pub type Routine = Pin<Box<dyn Future<Output = Result<(), Box<dyn Error + Send + Sync>>> + Send>>;

pub struct RecvChannelParams {
    pub on_msg: Option<OnMessageHdlrFn>,
    pub on_open: Option<OnOpenHdlrFn>,
    pub on_close: Option<OnCloseHdlrFn>,
    pub on_error: Option<OnErrorHdlrFn>,
}

impl RecvChannelParams {
    pub async fn configure_channel(mut self, chn: &RTCDataChannel) {
        if let Some(on_open) = self.on_open.take() {
            chn.on_open(on_open);
        }

        if let Some(on_msg) = self.on_msg.take() {
            chn.on_message(on_msg);
        }

        if let Some(on_close) = self.on_close.take() {
            chn.on_close(on_close);
        }

        if let Some(on_error) = self.on_error.take() {
            chn.on_error(on_error);
        }
    }
}
