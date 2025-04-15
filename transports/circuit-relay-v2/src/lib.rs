mod client;
mod error;
mod protocol_id;
mod traits;

pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/circuit_relay.rs"));
}

pub use self::{
    client::CircuitRelayV2Client,
    error::Error,
    protocol_id::{RELAY_V2_HOP_PROTOCOL_ID, RELAY_V2_STOP_PROTOCOL_ID},
    traits::{CircuitRelay, CircuitRelayProtocol, ConnectionInterface, StreamInterface},
};
