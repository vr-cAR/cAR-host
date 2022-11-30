use log::trace;
use std::{error::Error, future::Future, pin::Pin, fmt::{Display, self}};

use crate::{c_ar_controls::ThumbstickDirection, media::controls::ControlsReceiver};

use super::ControlsReceiverFactory;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TxtControlsReceiverConfig;

impl ControlsReceiverFactory for TxtControlsReceiverConfig {
    type Receiver = TxtControlsReceiver;

    fn create_receiver(&self) -> Self::Receiver {
        TxtControlsReceiver::new()
    }
}

pub struct TxtControlsReceiver {
    acc: i64,
}

impl TxtControlsReceiver {
    pub fn new() -> Self {
        Self { acc: i64::MIN }
    }
}

#[derive(Debug)]
pub enum ControlsReceiverError{}

impl Display for ControlsReceiverError{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Controls receiver error")
    }
}

impl Error for ControlsReceiverError{}

impl ControlsReceiver for TxtControlsReceiver {
    type Err = ControlsReceiverError;
    type RecvFuture<'a> = Pin<Box<dyn Future<Output = Result<(), Self::Err>> + 'a>>;
    fn recv(
        &mut self,
        controls: ThumbstickDirection,
    ) -> Pin<Box<dyn Future<Output = Result<(), ControlsReceiverError>> + '_>> {
        if controls.seq_num <= self.acc {
            return Box::pin(async move { Ok(()) });
        }
        self.acc = controls.seq_num;
        trace!(
            "Thumbstick Position: dx={}, dy={}",
            controls.dx,
            controls.dy
        );
        Box::pin(async move { Ok(()) })
    }
}
