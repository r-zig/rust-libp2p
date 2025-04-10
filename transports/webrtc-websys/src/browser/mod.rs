mod signaling;
mod stream;
mod transport;

pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/signaling.rs"));
}

pub use self::{
    signaling::{Signaling, SIGNALING_PROTOCOL_ID},
    stream::ProtobufStream,
    transport::BrowserTransport,
};
