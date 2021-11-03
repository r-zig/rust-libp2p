// Copyright 2021 Protocol Labs.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! [`NetworkBehaviour`] to act as a circuit relay v2 **relay**.

mod handler;
pub mod rate_limiter;

use crate::v2::message_proto;
use libp2p_core::connection::{ConnectedPoint, ConnectionId};
use libp2p_core::multiaddr::Protocol;
use libp2p_core::{Multiaddr, PeerId};
use libp2p_swarm::{NetworkBehaviour, NetworkBehaviourAction, NotifyHandler, PollParameters};
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroU32;
use std::ops::Add;
use std::task::{Context, Poll};
use std::time::Duration;
use wasm_timer::Instant;

/// Configuration for the [`Relay`] [`NetworkBehaviour`].
///
/// # Panics
///
/// [`Config::max_circuit_duration`] may not exceed [`u32::MAX`].
pub struct Config {
    pub max_reservations: usize,
    pub reservation_duration: Duration,
    // TODO: Mutable state in a config. Good idea?
    pub reservation_rate_limiters: Vec<Box<dyn rate_limiter::RateLimiter>>,

    pub max_circuits: usize,
    pub max_circuit_duration: Duration,
    pub max_circuit_bytes: u64,
    // TODO: Mutable state in a config. Good idea?
    pub circuit_src_rate_limiters: Vec<Box<dyn rate_limiter::RateLimiter>>,
}

impl Default for Config {
    fn default() -> Self {
        let reservation_rate_limiters = vec![
            // For each peer ID one reservation every 2 minutes with up to 30 reservations per hour.
            rate_limiter::new_per_peer(rate_limiter::GenericRateLimiterConfig {
                limit: NonZeroU32::new(30).expect("30 > 0"),
                interval: Duration::from_secs(60 * 2),
            }),
            // For each IP address one reservation every minute with up to 60 reservations per hour.
            rate_limiter::new_per_ip(rate_limiter::GenericRateLimiterConfig {
                limit: NonZeroU32::new(60).expect("60 > 0"),
                interval: Duration::from_secs(60),
            }),
        ];

        let circuit_src_rate_limiters = vec![
            // For each source peer ID one circuit every 2 minute with up to 30 circuits per hour.
            rate_limiter::new_per_peer(rate_limiter::GenericRateLimiterConfig {
                limit: NonZeroU32::new(30).expect("30 > 0"),
                interval: Duration::from_secs(60 * 2),
            }),
            // For each source IP address one circuit every minute with up to 60 circuits per hour.
            rate_limiter::new_per_ip(rate_limiter::GenericRateLimiterConfig {
                limit: NonZeroU32::new(60).expect("60 > 0"),
                interval: Duration::from_secs(60),
            }),
        ];

        Config {
            max_reservations: 128,
            reservation_duration: Duration::from_secs(60 * 60),
            reservation_rate_limiters,

            max_circuits: 16,
            max_circuit_duration: Duration::from_secs(2 * 60),
            max_circuit_bytes: 1 << 17, // 128 kibibyte
            circuit_src_rate_limiters,
        }
    }
}

/// The events produced by the [`Relay`] behaviour.
#[derive(Debug)]
pub enum Event {
    /// An inbound reservation request has been accepted.
    ReservationReqAccepted {
        src_peer_id: PeerId,
        /// Indicates whether the request replaces an existing reservation.
        renewed: bool,
    },
    /// Accepting an inbound reservation request failed.
    ReservationReqAcceptFailed {
        src_peer_id: PeerId,
        error: std::io::Error,
    },
    /// An inbound reservation request has been denied.
    ReservationReqDenied { src_peer_id: PeerId },
    /// Denying an inbound reservation request has failed.
    ReservationReqDenyFailed {
        src_peer_id: PeerId,
        error: std::io::Error,
    },
    /// An inbound reservation has timed out.
    ReservationTimedOut { src_peer_id: PeerId },
    /// An inbound circuit request has been denied.
    CircuitReqDenied {
        src_peer_id: PeerId,
        dst_peer_id: PeerId,
    },
    /// Denying an inbound circuit request failed.
    CircuitReqDenyFailed {
        src_peer_id: PeerId,
        dst_peer_id: PeerId,
        error: std::io::Error,
    },
    /// An inbound circuit request has been accepted.
    CircuitReqAccepted {
        src_peer_id: PeerId,
        dst_peer_id: PeerId,
    },
    /// Accepting an inbound circuit request failed.
    CircuitReqAcceptFailed {
        src_peer_id: PeerId,
        dst_peer_id: PeerId,
        error: std::io::Error,
    },
    /// An inbound circuit has closed.
    CircuitClosed {
        src_peer_id: PeerId,
        dst_peer_id: PeerId,
        error: Option<std::io::Error>,
    },
}

/// [`Relay`] is a [`NetworkBehaviour`] that implements the relay server
/// functionality of the circuit relay v2 protocol.
pub struct Relay {
    config: Config,

    local_peer_id: PeerId,

    reservations: HashMap<PeerId, HashSet<ConnectionId>>,
    circuits: CircuitsTracker,

    /// Queue of actions to return when polled.
    queued_actions: VecDeque<NetworkBehaviourAction<Event, handler::Prototype>>,
}

impl Relay {
    pub fn new(local_peer_id: PeerId, config: Config) -> Self {
        Self {
            config,
            local_peer_id,
            reservations: Default::default(),
            circuits: Default::default(),
            queued_actions: Default::default(),
        }
    }
}

impl NetworkBehaviour for Relay {
    type ProtocolsHandler = handler::Prototype;
    type OutEvent = Event;

    fn new_handler(&mut self) -> Self::ProtocolsHandler {
        handler::Prototype {
            config: handler::Config {
                reservation_duration: self.config.reservation_duration,
                max_circuit_duration: self.config.max_circuit_duration,
                max_circuit_bytes: self.config.max_circuit_bytes,
            },
        }
    }

    fn addresses_of_peer(&mut self, _remote_peer_id: &PeerId) -> Vec<Multiaddr> {
        vec![]
    }

    fn inject_connected(&mut self, _peer_id: &PeerId) {}

    fn inject_disconnected(&mut self, _peer: &PeerId) {}

    fn inject_connection_closed(
        &mut self,
        peer: &PeerId,
        connection: &ConnectionId,
        _: &ConnectedPoint,
        _handler: handler::Handler,
    ) {
        self.reservations
            .get_mut(peer)
            .map(|cs| cs.remove(&connection))
            .unwrap_or(false);

        for circuit in self
            .circuits
            .remove_by_connection(*peer, *connection)
            .iter()
            // Only emit [`CircuitClosed`] for accepted requests.
            .filter(|c| matches!(c.status, CircuitStatus::Accepted))
        {
            self.queued_actions
                .push_back(NetworkBehaviourAction::GenerateEvent(
                    Event::CircuitClosed {
                        src_peer_id: circuit.src_peer_id,
                        dst_peer_id: circuit.dst_peer_id,
                        error: Some(std::io::ErrorKind::ConnectionAborted.into()),
                    },
                ));
        }
    }

    fn inject_event(
        &mut self,
        event_source: PeerId,
        connection: ConnectionId,
        event: handler::Event,
    ) {
        match event {
            handler::Event::ReservationReqReceived {
                inbound_reservation_req,
                remote_addr,
            } => {
                let now = Instant::now();

                let handler_event = if remote_addr.iter().any(|p| p == Protocol::P2pCircuit) {
                    // Deny reservation requests over relayed circuits.
                    handler::In::DenyReservationReq {
                        inbound_reservation_req,
                        status: message_proto::Status::PermissionDenied,
                    }
                } else if self
                    .reservations
                    .iter()
                    .map(|(_, cs)| cs.len())
                    .sum::<usize>()
                    >= self.config.max_reservations
                    || !self
                        .config
                        .reservation_rate_limiters
                        .iter_mut()
                        .all(|limiter| limiter.try_next(event_source, &remote_addr, now))
                {
                    // Deny reservation exceeding limits.
                    handler::In::DenyReservationReq {
                        inbound_reservation_req,
                        status: message_proto::Status::ResourceLimitExceeded,
                    }
                } else {
                    // Accept reservation.
                    self.reservations
                        .entry(event_source)
                        .or_default()
                        .insert(connection);
                    handler::In::AcceptReservationReq {
                        inbound_reservation_req,
                        addrs: vec![],
                    }
                };

                self.queued_actions
                    .push_back(NetworkBehaviourAction::NotifyHandler {
                        handler: NotifyHandler::One(connection),
                        peer_id: event_source,
                        event: handler_event,
                    });
            }
            handler::Event::ReservationReqAccepted { renewed } => {
                // Ensure local eventual consistent reservation state matches handler (source of
                // truth).
                self.reservations
                    .entry(event_source)
                    .or_default()
                    .insert(connection);

                self.queued_actions
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        Event::ReservationReqAccepted {
                            src_peer_id: event_source,
                            renewed,
                        },
                    ));
            }
            handler::Event::ReservationReqAcceptFailed { error } => {
                self.queued_actions
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        Event::ReservationReqAcceptFailed {
                            src_peer_id: event_source,
                            error,
                        },
                    ));
            }
            handler::Event::ReservationReqDenied {} => {
                self.queued_actions
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        Event::ReservationReqDenied {
                            src_peer_id: event_source,
                        },
                    ));
            }
            handler::Event::ReservationReqDenyFailed { error } => {
                self.queued_actions
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        Event::ReservationReqDenyFailed {
                            src_peer_id: event_source,
                            error,
                        },
                    ));
            }
            handler::Event::ReservationTimedOut {} => {
                self.reservations
                    .get_mut(&event_source)
                    .map(|cs| cs.remove(&connection));

                self.queued_actions
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        Event::ReservationTimedOut {
                            src_peer_id: event_source,
                        },
                    ));
            }
            handler::Event::CircuitReqReceived {
                inbound_circuit_req,
                remote_addr,
            } => {
                let now = Instant::now();

                let action = if remote_addr.iter().any(|p| p == Protocol::P2pCircuit) {
                    // Deny circuit requests over relayed circuit.
                    //
                    // An attacker could otherwise build recursive or cyclic circuits.
                    NetworkBehaviourAction::NotifyHandler {
                        handler: NotifyHandler::One(connection),
                        peer_id: event_source,
                        event: handler::In::DenyCircuitReq {
                            circuit_id: None,
                            inbound_circuit_req,
                            status: message_proto::Status::PermissionDenied,
                        },
                    }
                } else if self.circuits.len() >= self.config.max_circuits
                    || !self
                        .config
                        .circuit_src_rate_limiters
                        .iter_mut()
                        .all(|limiter| limiter.try_next(event_source, &remote_addr, now))
                {
                    // Deny circuit exceeding limits.
                    NetworkBehaviourAction::NotifyHandler {
                        handler: NotifyHandler::One(connection),
                        peer_id: event_source,
                        event: handler::In::DenyCircuitReq {
                            circuit_id: None,
                            inbound_circuit_req,
                            status: message_proto::Status::ResourceLimitExceeded,
                        },
                    }
                } else if let Some(dst_conn) = self
                    .reservations
                    .get(&inbound_circuit_req.dst())
                    .map(|cs| cs.iter().next())
                    .flatten()
                {
                    // Accept circuit request if reservation present.
                    let circuit_id = self.circuits.insert(Circuit {
                        status: CircuitStatus::Accepting,
                        src_peer_id: event_source,
                        src_connection_id: connection,
                        dst_peer_id: inbound_circuit_req.dst(),
                        dst_connection_id: *dst_conn,
                    });

                    NetworkBehaviourAction::NotifyHandler {
                        handler: NotifyHandler::One(*dst_conn),
                        peer_id: event_source,
                        event: handler::In::NegotiateOutboundConnect {
                            circuit_id,
                            inbound_circuit_req,
                            relay_peer_id: self.local_peer_id,
                            src_peer_id: event_source,
                            src_connection_id: connection,
                        },
                    }
                } else {
                    // Deny circuit request if no reservation present.
                    NetworkBehaviourAction::NotifyHandler {
                        handler: NotifyHandler::One(connection),
                        peer_id: event_source,
                        event: handler::In::DenyCircuitReq {
                            circuit_id: None,
                            inbound_circuit_req,
                            status: message_proto::Status::NoReservation,
                        },
                    }
                };
                self.queued_actions.push_back(action);
            }
            handler::Event::CircuitReqDenied {
                circuit_id,
                dst_peer_id,
            } => {
                if let Some(circuit_id) = circuit_id {
                    self.circuits.remove(circuit_id);
                }

                self.queued_actions
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        Event::CircuitReqDenied {
                            src_peer_id: event_source,
                            dst_peer_id,
                        },
                    ));
            }
            handler::Event::CircuitReqDenyFailed {
                circuit_id,
                dst_peer_id,
                error,
            } => {
                if let Some(circuit_id) = circuit_id {
                    self.circuits.remove(circuit_id);
                }

                self.queued_actions
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        Event::CircuitReqDenyFailed {
                            src_peer_id: event_source,
                            dst_peer_id,
                            error,
                        },
                    ));
            }
            handler::Event::OutboundConnectNegotiated {
                circuit_id,
                src_peer_id,
                src_connection_id,
                inbound_circuit_req,
                dst_handler_notifier,
                dst_stream,
                dst_pending_data,
            } => {
                self.queued_actions
                    .push_back(NetworkBehaviourAction::NotifyHandler {
                        handler: NotifyHandler::One(src_connection_id),
                        peer_id: src_peer_id,
                        event: handler::In::AcceptAndDriveCircuit {
                            circuit_id,
                            dst_peer_id: event_source,
                            inbound_circuit_req,
                            dst_handler_notifier,
                            dst_stream,
                            dst_pending_data,
                        },
                    });
            }
            handler::Event::OutboundConnectNegotiationFailed {
                circuit_id,
                src_peer_id,
                src_connection_id,
                inbound_circuit_req,
                status,
            } => {
                self.queued_actions
                    .push_back(NetworkBehaviourAction::NotifyHandler {
                        handler: NotifyHandler::One(src_connection_id),
                        peer_id: src_peer_id,
                        event: handler::In::DenyCircuitReq {
                            circuit_id: Some(circuit_id),
                            inbound_circuit_req,
                            status,
                        },
                    });
            }
            handler::Event::CircuitReqAccepted {
                dst_peer_id,
                circuit_id,
            } => {
                self.circuits.accepted(circuit_id);
                self.queued_actions
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        Event::CircuitReqAccepted {
                            src_peer_id: event_source,
                            dst_peer_id,
                        },
                    ));
            }
            handler::Event::CircuitReqAcceptFailed {
                dst_peer_id,
                circuit_id,
                error,
            } => {
                self.circuits.remove(circuit_id);
                self.queued_actions
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        Event::CircuitReqAcceptFailed {
                            src_peer_id: event_source,
                            dst_peer_id,
                            error,
                        },
                    ));
            }
            handler::Event::CircuitClosed {
                dst_peer_id,
                circuit_id,
                error,
            } => {
                self.circuits.remove(circuit_id);

                self.queued_actions
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        Event::CircuitClosed {
                            src_peer_id: event_source,
                            dst_peer_id,
                            error,
                        },
                    ));
            }
        }
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
        poll_parameters: &mut impl PollParameters,
    ) -> Poll<NetworkBehaviourAction<Self::OutEvent, Self::ProtocolsHandler>> {
        if let Some(mut event) = self.queued_actions.pop_front() {
            // Set external addresses in [`AcceptReservationReq`].
            if let NetworkBehaviourAction::NotifyHandler {
                event: handler::In::AcceptReservationReq { ref mut addrs, .. },
                ..
            } = &mut event
            {
                *addrs = poll_parameters
                    .external_addresses()
                    .map(|a| {
                        a.addr
                            .with(Protocol::P2p((*poll_parameters.local_peer_id()).into()))
                    })
                    .collect();
            }

            return Poll::Ready(event);
        }

        Poll::Pending
    }
}

#[derive(Default)]
struct CircuitsTracker {
    next_id: CircuitId,
    circuits: HashMap<CircuitId, Circuit>,
}

impl CircuitsTracker {
    fn len(&self) -> usize {
        self.circuits.len()
    }

    fn insert(&mut self, circuit: Circuit) -> CircuitId {
        let id = self.next_id;
        self.next_id = self.next_id + 1;

        self.circuits.insert(id, circuit);

        id
    }

    fn accepted(&mut self, circuit_id: CircuitId) {
        if let Some(c) = self.circuits.get_mut(&circuit_id) {
            c.status = CircuitStatus::Accepted;
        };
    }

    fn remove(&mut self, circuit_id: CircuitId) -> Option<Circuit> {
        self.circuits.remove(&circuit_id)
    }

    fn remove_by_connection(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
    ) -> Vec<Circuit> {
        let mut removed = vec![];

        self.circuits.retain(|_circuit_id, circuit| {
            let is_src =
                circuit.src_peer_id == peer_id && circuit.src_connection_id == connection_id;
            let is_dst =
                circuit.dst_peer_id == peer_id && circuit.dst_connection_id == connection_id;

            if is_src || is_dst {
                removed.push(circuit.clone());
                // Remove circuit from HashMap.
                false
            } else {
                // Retain circuit in HashMap.
                true
            }
        });

        removed
    }
}

#[derive(Clone)]
struct Circuit {
    src_peer_id: PeerId,
    src_connection_id: ConnectionId,
    dst_peer_id: PeerId,
    dst_connection_id: ConnectionId,
    status: CircuitStatus,
}

#[derive(Clone)]
enum CircuitStatus {
    Accepting,
    Accepted,
}

#[derive(Default, Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct CircuitId(u64);

impl Add<u64> for CircuitId {
    type Output = CircuitId;

    fn add(self, rhs: u64) -> Self {
        CircuitId(self.0 + rhs)
    }
}
