use futures::{
    AsyncRead, AsyncWrite
};
use libp2p_core::{Multiaddr, PeerId};

#[cfg(feature = "relayv2")]
use crate::{client::ReservationVoucher, error::Error};

/// An abstraction over a stream to be used with the relay.
#[cfg(feature = "relayv2")]
pub trait StreamInterface: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

/// An abstraction over a Connection to be used with the relay.
#[cfg(feature = "relayv2")]
pub trait ConnectionInterface {
    fn new_stream(&mut self, protocol_id: &str) -> Result<Box<dyn StreamInterface>, Error>;
}

/// A trait representing the implementation of the p2p circuit relay protocol v2
/// based on: 
/// 
/// https://github.com/libp2p/specs/blob/master/relay/circuit-v2.md.
#[cfg(feature = "relayv2")]
pub trait CircuitRelayProtocol {
    async fn hop_protocol(
        &mut self,
        connection: &mut dyn ConnectionInterface,
        target_peer_id: &PeerId,
    ) -> Result<Box<dyn StreamInterface>, Error>;

    async fn stop_protocol(
        &self,
        connection: &mut dyn ConnectionInterface,
    ) -> Result<PeerId, Error>;

    async fn reservation(
        &mut self,
        connection: &mut dyn ConnectionInterface,
        relay_addr: &Multiaddr,
    ) -> Result<ReservationVoucher, Error>;
}

/// A trait representing the basic functionality of the circuit relay.
#[cfg(feature = "relayv2")]
pub trait CircuitRelay {
    async fn connect_through_relay(
        &mut self,
        connection: &mut dyn ConnectionInterface,
        target_peer_id: PeerId,
    ) -> Result<Box<dyn StreamInterface>, Error>;
}
