#![doc = include_str!("../README.md")]

mod browser;
mod connection;
mod error;
mod sdp;
mod stream;
mod transport;
pub mod upgrade;

pub use self::{
    browser::{
        BrowserTransport, Config as BrowserConfig, ProtobufStream, Signaling, SignalingProtocol,
        SIGNALING_PROTOCOL_ID,
    },
    connection::Connection,
    error::Error,
    stream::Stream,
    transport::{Config, Transport},
};
