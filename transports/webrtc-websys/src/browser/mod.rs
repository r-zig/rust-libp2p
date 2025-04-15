mod signaling;
mod stream;
mod transport;

pub(crate) mod pb {
    include!(concat!(env!("OUT_DIR"), "/signaling.rs"));
}

pub use self::{
    signaling::{Signaling, SignalingProtocol, SIGNALING_PROTOCOL_ID},
    stream::ProtobufStream,
    transport::{BrowserTransport, Config},
};