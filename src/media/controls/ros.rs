use log::{trace, warn};
use rosrust::Publisher;
use rosrust_msg::std_msgs::Float32MultiArray;
use std::{error::Error, future::Future, pin::Pin};

use super::ControlsReceiverFactory;
use crate::{c_ar_controls::ThumbstickDirection, media::controls::ControlsReceiver};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RosControlsReceiverConfig {
    ros_node: String,
    ros_topic: String,
    queue_size: usize,
}

impl ControlsReceiverFactory for RosControlsReceiverConfig {
    type Receiver = RosControlsReceiver;

    fn create_receiver(&self) -> Self::Receiver {
        RosControlsReceiver::new(self)
    }
}

pub struct RosControlsReceiver {
    publisher: Option<Publisher<Float32MultiArray>>,
    acc: i64,
}

impl RosControlsReceiver {
    pub fn new(config: &RosControlsReceiverConfig) -> Self {
        rosrust::init(&config.ros_node);
        let publisher = match rosrust::publish(&config.ros_topic, config.queue_size) {
            Ok(publisher) => Some(publisher),
            Err(err) => {
                warn!("Failed to create ROS publisher. Error: {}", err);
                None
            }
        };
        Self {
            publisher,
            acc: i64::MIN,
        }
    }
}

impl ControlsReceiver for RosControlsReceiver {
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
        let dir = nalgebra::SVector::from([controls.dx, controls.dy]);
        if let Some(publisher) = self.publisher.as_ref() {
            Box::pin(async move {
                let speed = dir.norm() * if dir[1] < 0f64 { -1f64 } else { 1f64 };

                let msg = Float32MultiArray {
                    data: vec![speed as f32, -dir[0] as f32],
                    ..Default::default()
                };
                if let Err(err) = publisher.send(msg) {
                    warn!("Failed to send turn vector to ROS. Error: {}", err);
                }
                Ok(())
            })
        } else {
            Box::pin(async move { Ok(()) })
        }
    }
}
