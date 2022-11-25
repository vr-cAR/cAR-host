use log::trace;
use std::{error::Error, future::Future, pin::Pin};

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

impl ControlsReceiver for TxtControlsReceiver {
    fn recv(
        &mut self,
        controls: ThumbstickDirection,
    ) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn Error>>> + '_>> {
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
