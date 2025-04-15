use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

use futures::{AsyncReadExt, AsyncWriteExt};
use libp2p_core::{multiaddr::Protocol, Multiaddr};
use libp2p_identity::PeerId;
use prost::Message;

use crate::{
    pb::{hop_message, stop_message, HopMessage, Limit, Peer, Status, StopMessage}, traits::{CircuitRelay, CircuitRelayProtocol}, 
    ConnectionInterface, Error, StreamInterface, RELAY_V2_HOP_PROTOCOL_ID, RELAY_V2_STOP_PROTOCOL_ID
};

/// A cryptographic certificate signed by the relay that testifies that it is
/// willing to provide service to the reserving peer.
///
/// Note: Vouchers are only advisory as they are not
/// currently enforced.
/// 
/// https://github.com/libp2p/specs/blob/master/relay/circuit-v2.md#reservation
#[derive(Debug, Clone)]
pub struct ReservationVoucher {
    /// The relay's peer ID
    pub relay_peer_id: PeerId,
    /// The expiration time of the reservation (in UNIX seconds)
    pub expire: u64,
    /// The addresses of the relay
    pub relay_addrs: Vec<Multiaddr>,
    /// The reservation voucher
    pub voucher: Vec<u8>,
    /// Optional connection limits
    pub limit: Option<Limit>,
}

/// The circuit-relay v2 client.
#[derive(Clone, Debug)]
pub struct CircuitRelayV2Client {
    reservations: HashMap<Multiaddr, ReservationVoucher>,
    limits: HashMap<PeerId, Limit>,
}

impl CircuitRelayV2Client {
    pub fn new() -> Self {
        Self {
            reservations: HashMap::new(),
            limits: HashMap::new(),
        }
    }

    pub fn has_valid_reservation(&self, relay_addr: &Multiaddr) -> bool {
        if let Some(reservation) = self.reservations.get(relay_addr) {
            let time_now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("reservation is not expired")
                .as_secs();

            return reservation.expire > time_now;
        }

        false
    }
}

impl CircuitRelay for CircuitRelayV2Client {
    /// Connect to a peer through the relay.
    async fn connect_through_relay(
        &mut self,
        connection: &mut dyn ConnectionInterface,
        target_peer_id: PeerId,
    ) -> Result<Box<dyn StreamInterface>, Error> {
        self.hop_protocol(connection, &target_peer_id).await
    }
}

impl CircuitRelayProtocol for CircuitRelayV2Client {
    /// Implementation of the Hop Protocol. Governs interactions 
    /// between clients and the relay.
    async fn hop_protocol(
        &mut self,
        connection: &mut dyn ConnectionInterface,
        target_peer_id: &PeerId,
    ) -> Result<Box<dyn StreamInterface>, Error> {
        let rc_conn = Rc::new(RefCell::new(connection));
        let mut stream = rc_conn
            .borrow_mut()
            .new_stream(RELAY_V2_HOP_PROTOCOL_ID)
            .unwrap();

        // Send a CONNECT message to the target peer
        let connect_message = HopMessage {
            r#type: Some(hop_message::Type::Connect as i32),
            peer: Some(Peer {
                id: Some(target_peer_id.to_bytes()),
                addrs: Vec::new(),
            }),
            reservation: None,
            limit: None,
            status: None,
        };

        let mut buf: Vec<u8> = Vec::with_capacity(connect_message.encoded_len());
        connect_message
            .encode(&mut buf)
            .expect("should encode into the vec correctly");
        stream.write_all(&buf).await.unwrap();

        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let hop_message = HopMessage::decode(&response[..]).unwrap();

        let status = hop_message.status.unwrap_or(Status::Unused as i32);
        if status != hop_message::Type::Status as i32 {
            Err(Error::Connection("".to_string())).unwrap()
        }

        if status != Status::Ok as i32 {
            Err(Error::Connection("".to_string())).unwrap()
        }

        let reservation = hop_message.reservation.unwrap();
        let limit = hop_message.limit.unwrap_or_default();

        Ok(stream)
    }

    /// Implementation of the Stop Protocol. Governs connection termination between the relay
    /// and the target peer.
    async fn stop_protocol(
        &self,
        connection: &mut dyn ConnectionInterface,
    ) -> Result<PeerId, Error> {
        // To terminate a relayed connection we must use an existing connection to open a stream to
        // the target peer
        let mut stop_protocol_stream = connection.new_stream(RELAY_V2_STOP_PROTOCOL_ID).unwrap();

        // Receive the initial message
        let mut msg_bytes = Vec::new();
        stop_protocol_stream.read_to_end(&mut msg_bytes).await?;
        let stop_msg = StopMessage::decode(&msg_bytes[..]).unwrap();

        // Make sure it's a CONNECT message and notify the user with an Error
        // if not
        if stop_msg.r#type != Some(stop_message::Type::Connect as i32) {
            
        }

        let peer_info = stop_msg
            .peer
            .ok_or_else(|| Error::Connection("No peer info in CONNECT message".into()))?;

        let peer_id = PeerId::from_bytes(&peer_info.id.unwrap())
            .map_err(|e| Error::Connection(e.to_string()))?;

        let response = StopMessage {
            r#type: Some(stop_message::Type::Status as i32),
            peer: None,
            limit: None,
            status: Some(Status::Ok as i32),
        };

        let mut buf = Vec::with_capacity(response.encoded_len());
        response.encode(&mut buf).expect("should incode into the vec correctly");
        stop_protocol_stream.write_all(&buf).await?;

        Ok(peer_id)
    }

    /// Make a reservation with the relay client. Temporarily allocates space for another
    /// connection.
    async fn reservation(
        &mut self,
        connection: &mut dyn ConnectionInterface,
        relay_addr: &Multiaddr,
    ) -> Result<ReservationVoucher, Error> {
        let mut reservation_stream = connection.new_stream(RELAY_V2_HOP_PROTOCOL_ID).unwrap();

        let reserve_message = HopMessage {
            r#type: Some(hop_message::Type::Reserve as i32),
            peer: None,
            reservation: None,
            limit: None,
            status: None,
        };

        let mut buf = Vec::with_capacity(reserve_message.encoded_len());
        reserve_message.encode(&mut buf).unwrap();
        reservation_stream.write_all(&buf).await?;

        let mut response = Vec::new();
        reservation_stream.read_to_end(&mut response).await?;
        let hop_message = HopMessage::decode(&response[..]).unwrap();

        let Some(hop_message_type) = hop_message.r#type else {
            return Err(Error::Connection("".to_string()));
        };

        if hop_message_type != hop_message::Type::Status as i32 {
            return Err(Error::Connection("".to_string()));
        }

        let status = hop_message.status.unwrap_or(Status::Unused as i32);
        if status != Status::Ok as i32 {}

        let Some(hop_reservation) = hop_message.reservation else {
            return Err(Error::Connection("".to_string()));
        };

        let relay_peer_id = relay_addr
            .iter()
            .find_map(|proto| {
                if let Protocol::P2p(peer_id) = proto {
                    Some(peer_id)
                } else {
                    None
                }
            })
            .ok_or_else(|| Error::InvalidMultiaddr("Missing relay peer ID".to_string()))?;

        // Create and store the relay reservation voucher
        let relay_reservation = ReservationVoucher {
            relay_peer_id,
            expire: hop_reservation.expire.unwrap(),
            relay_addrs: hop_reservation
                .addrs
                .into_iter()
                .map(|addr| Multiaddr::try_from(addr).expect("Valid multiaddr"))
                .collect(),
            voucher: hop_reservation.voucher.expect(""),
            limit: hop_message.limit,
        };

        self.reservations
            .insert(relay_addr.clone(), relay_reservation.clone());

        Ok(relay_reservation)
    }
}
