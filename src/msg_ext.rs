use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

use webrtc::{
    ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
    peer_connection::sdp::{sdp_type::RTCSdpType, session_description::RTCSessionDescription},
};

use crate::c_ar::{NotifyDescription, NotifyIce, SdpType};

impl From<RTCSdpType> for SdpType {
    fn from(ty: RTCSdpType) -> Self {
        match ty {
            RTCSdpType::Unspecified => Self::Unspecified,
            RTCSdpType::Offer => Self::Offer,
            RTCSdpType::Pranswer => Self::Pranswer,
            RTCSdpType::Answer => Self::Answer,
            RTCSdpType::Rollback => Self::Rollback,
        }
    }
}

impl From<SdpType> for RTCSdpType {
    fn from(ty: SdpType) -> Self {
        match ty {
            SdpType::Offer => Self::Offer,
            SdpType::Pranswer => Self::Pranswer,
            SdpType::Answer => Self::Answer,
            SdpType::Rollback => Self::Rollback,
            SdpType::Unspecified => Self::Unspecified,
        }
    }
}

impl From<RTCSessionDescription> for NotifyDescription {
    fn from(desc: RTCSessionDescription) -> Self {
        let mut msg = Self::default();
        msg.set_sdp_type(desc.sdp_type.into());
        msg.sdp = desc.sdp;
        msg
    }
}

impl From<NotifyDescription> for RTCSessionDescription {
    fn from(msg: NotifyDescription) -> Self {
        let mut desc = RTCSessionDescription::default();
        desc.sdp_type = msg.sdp_type().into();
        desc.sdp = msg.sdp;
        desc
    }
}

#[derive(Debug)]
pub enum DeserializationError {
    Base64Decode,
    InvalidFormat,
}

impl Display for DeserializationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                DeserializationError::Base64Decode => "Base64 decode error",
                DeserializationError::InvalidFormat => "Formatted incorrectly",
            }
        )
    }
}

impl Error for DeserializationError {}

impl TryFrom<NotifyIce> for RTCIceCandidateInit {
    type Error = DeserializationError;

    fn try_from(value: NotifyIce) -> Result<Self, Self::Error> {
        serde_json::from_str(
            &String::from_utf8(
                base64::decode(value.json_base64)
                    .map_err(|_| DeserializationError::Base64Decode)?,
            )
            .map_err(|_| DeserializationError::InvalidFormat)?,
        )
        .map_err(|_| DeserializationError::InvalidFormat)
    }
}

impl NotifyIce {
    pub async fn from(ice: RTCIceCandidate) -> Result<Self, webrtc::Error> {
        let json_base64 = base64::encode(
            serde_json::to_string(&ice.to_json()?)
                .expect("serialization of ice to json should not fail"),
        );
        Ok(Self { json_base64 })
    }
}
