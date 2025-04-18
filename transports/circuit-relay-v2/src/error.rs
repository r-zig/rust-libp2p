/// Errors that may happen on the [`Transport`](crate::Transport) or the
/// [`Connection`](crate::Connection).
#[derive(thiserror::Error, Debug)]
#[cfg(feature = "relayv2")]
pub enum Error {
    #[error("Invalid multiaddr: {0}")]
    InvalidMultiaddr(String),

    #[error("JavaScript error: {0}")]
    Js(String),

    #[error("JavaScript typecasting failed")]
    JsCastFailed,

    #[error("Unknown remote peer ID")]
    UnknownRemotePeerId,

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Serialization error: {0}")]
    ProtoSerialization(String),
}

#[cfg(feature = "relayv2")]
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Connection(error.to_string())
    }
}

#[cfg(feature = "relayv2")]
impl From<prost::DecodeError> for Error {
    fn from(error: prost::DecodeError) -> Self {
        Error::ProtoSerialization(error.to_string())
    }
}

#[cfg(feature = "relayv2")]
impl From<prost::EncodeError> for Error {
    fn from(error: prost::EncodeError) -> Self {
        Error::ProtoSerialization(error.to_string())
    }
}

// Libp2p PeerID parsing errors
#[cfg(feature = "relayv2")]
impl From<libp2p_identity::ParseError> for Error {
    fn from(error: libp2p_identity::ParseError) -> Self {
        Error::Connection(error.to_string())
    }
}

// For multiaddr parsing errors
#[cfg(feature = "relayv2")]
impl From<libp2p_core::multiaddr::Error> for Error {
    fn from(error: libp2p_core::multiaddr::Error) -> Self {
        Error::InvalidMultiaddr(error.to_string())
    }
}

// Generic string error conversion
#[cfg(feature = "relayv2")]
impl From<String> for Error {
    fn from(error: String) -> Self {
        Error::Connection(error)
    }
}
