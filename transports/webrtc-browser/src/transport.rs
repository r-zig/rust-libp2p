use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use futures::{channel::mpsc::channel, future::FutureExt, StreamExt};
use libp2p_circuit_relay_v2::{
    CircuitRelay, CircuitRelayProtocol, CircuitRelayV2Client,
};
use libp2p_core::{
    multiaddr::{Multiaddr, Protocol},
    transport::{ListenerId, Transport as _, TransportError, TransportEvent},
};
use libp2p_identity::{Keypair, PeerId};
use libp2p_webrtc_utils::Fingerprint;
use wasm_bindgen::{prelude::*, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::{RtcConfiguration, RtcDataChannelInit, WebSocket};

use super::{Signaling, SIGNALING_PROTOCOL_ID};
use libp2p_webrtc_websys::{Connection, Error, upgrade};
use crate::signaling::SignalingProtocol;

/// Configuration for WebRTC browser transport
#[derive(Debug, Clone)]
pub struct Config {
    pub keypair: Keypair,
    pub stun_servers: Vec<String>,
}

/// Config for the [`Transport`].
impl Config {
    pub fn new(keypair: &Keypair) -> Self {
        Self {
            keypair: keypair.clone(),
            stun_servers: vec![],
        }
    }

    pub fn with_stun_server(mut self, server: impl Into<String>) -> Self {
        self.stun_servers.push(server.into());
        self
    }
}

/// A WebRTC [`Transport`] for browser-to-browser connections.
pub struct Transport {
    config: Config,
    pending_events: VecDeque<TransportEvent<<Self as libp2p_core::Transport>::ListenerUpgrade, Error>>,
    #[cfg(feature = "relayv2")]
    circuit_relay_cient: CircuitRelayV2Client,
    active_relay_connections: HashMap<Multiaddr, Rc<RefCell<Connection>>>,
}

impl Transport {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            pending_events: VecDeque::new(),
            #[cfg(feature = "relayv2")]
            circuit_relay_cient: CircuitRelayV2Client::new(),
            active_relay_connections: HashMap::new(),
        }
    }

    /// Reserve a spot on the circuit relay.
    #[cfg(feature = "relayv2")]
    pub async fn reserve_relay(&mut self, relay_addr: &Multiaddr) {
        let mut relay_connection = dial_relay(relay_addr.clone(), &self.config).await.unwrap();
        self.circuit_relay_cient
            .reservation(&mut relay_connection, relay_addr)
            .await
            .unwrap();
        self.active_relay_connections
            .insert(relay_addr.clone(), Rc::new(RefCell::new(relay_connection)));
    }
}

/// Dial and establish a connection with the circuit relay.
async fn dial_relay(relay_addr: Multiaddr, config: &Config) -> Result<Connection, Error> {
    // Extract the socket address from the relay multi address and establish
    // a websocket connection
    let socket_addr = extract_socket_addr(&relay_addr)?;
    let relay_fingerprint = extract_fingerprint(&relay_addr)?;

    let ws_url = format!("wss://{}:{}", socket_addr.ip(), socket_addr.port());
    let ws = WebSocket::new(&ws_url).unwrap();

    // Setup a channel to send and receive messages over the websocket.
    let (mut ws_tx, mut ws_rx) = channel(1024);

    let onmessage_callback = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
        if let Ok(data) = event.data().dyn_into::<js_sys::JsString>() {
            let message_bytes = data.as_string().unwrap().into_bytes();
            // Forward the messages from the callback to the receiver
            let _ = ws_tx.try_send(message_bytes);
        }
    }) as Box<dyn FnMut(web_sys::MessageEvent)>);
    ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
    onmessage_callback.forget();

    spawn_local(async move {
        while let Some(message) = ws_rx.next().await {
            let message_str = String::from_utf8(message).unwrap();
            ws.send_with_str(&message_str).unwrap();
        }
    });

    let (_, relay_connection) =
        upgrade::outbound(socket_addr, relay_fingerprint, config.keypair.clone())
            .await
            .unwrap();

    Ok(relay_connection)
}

impl libp2p_core::Transport for Transport {
    type Output = (PeerId, Connection);
    type Error = Error;
    type ListenerUpgrade =
        futures::future::LocalBoxFuture<'static, Result<Self::Output, Self::Error>>;
    type Dial = futures::future::LocalBoxFuture<'static, Result<Self::Output, Self::Error>>;

    fn listen_on(
        &mut self,
        id: ListenerId,
        addr: Multiaddr,
    ) -> std::result::Result<(), TransportError<Self::Error>> {
        if addr.iter().any(|p| p == Protocol::WebRTC) {
            self.pending_events.push_back(TransportEvent::NewAddress {
                listener_id: id,
                listen_addr: addr,
            });
            Ok(())
        } else {
            Err(TransportError::MultiaddrNotSupported(addr))
        }
    }

    fn remove_listener(&mut self, _id: ListenerId) -> bool {
        false
    }

    fn dial(
        &mut self,
        addr: Multiaddr,
        dial_opts: libp2p_core::transport::DialOpts,
    ) -> std::result::Result<Self::Dial, TransportError<Self::Error>> {
        // Check if the browser WebRTC addr is valid
        if !libp2p_webrtc_utils::is_valid_browser_webrtc_addr(&addr) {
            return Err(TransportError::MultiaddrNotSupported(addr));
        }

        if dial_opts.role.is_listener() {
            return Err(TransportError::MultiaddrNotSupported(addr));
        }

        let (relay_addr, target_peer) = extract_relay_and_target(&addr)
            .ok_or_else(|| TransportError::MultiaddrNotSupported(addr.clone()))?;

        let config = self.config.clone();
        let mut relay_client = self.circuit_relay_cient.clone();
        let active_relay_conns = self.active_relay_connections.clone();

        let addr = addr.clone();

        let socket_addr = extract_socket_addr(&relay_addr).unwrap();
        let remote_fingerprint = extract_fingerprint(&addr).unwrap();

        // Prepare the RtcConfiguration
        let rtc_config = RtcConfiguration::new();

        if !&config.stun_servers.is_empty() {
            let ice_servers = js_sys::Array::new();

            for server in &config.stun_servers {
                let ice_server = js_sys::Object::new();
                js_sys::Reflect::set(
                    &ice_server,
                    &JsValue::from_str("urls"),
                    &JsValue::from_str(&server),
                )
                .map_err(|err| TransportError::Other(err))
                .unwrap();
                ice_servers.push(&ice_server);
            }

            rtc_config.set_ice_servers(&ice_servers);
        }

        Ok(async move {
            // Setup a relay connection and establish a new stream for WebRTC signaling
            let relay_connection = {
                let existing_conn = active_relay_conns
                    .iter()
                    .find(
                        |(addr, _rc_conns): &(&Multiaddr, &Rc<RefCell<Connection>>)| {
                            *addr == &relay_addr
                        },
                    )
                    .map(|(_, conn)| conn.clone());

                match existing_conn {
                    Some(conn) => {
                        if relay_client.has_valid_reservation(&relay_addr) {
                            conn
                        } else {
                            let mut conn = dial_relay(relay_addr.clone(), &config).await.unwrap();

                            relay_client
                                .reservation(&mut conn, &relay_addr)
                                .await
                                .unwrap();

                            Rc::new(RefCell::new(conn))
                        }
                    }
                    None => {
                        let mut conn = dial_relay(relay_addr.clone(), &config).await.unwrap();
                        relay_client
                            .reservation(&mut conn, &relay_addr)
                            .await
                            .unwrap();

                        Rc::new(RefCell::new(conn))
                    }
                }
            };

            let mut connection = relay_connection.borrow_mut();
            let relayed_stream = relay_client
                .connect_through_relay(&mut *connection, target_peer.clone())
                .await
                .unwrap();

            let signaling_data_channel = RtcDataChannelInit::new();
            let rtc_data_channel = relay_connection
                .borrow_mut()
                .rtc_connection()
                .create_data_channel_with_data_channel_dict(
                    SIGNALING_PROTOCOL_ID,
                    &signaling_data_channel,
                );

            // Perform signaling over the WebSocket relay connection
            let signaling_protocol = SignalingProtocol::new();
            signaling_protocol
                .perform_signaling(
                    &relay_connection.borrow().rtc_connection(),
                    relayed_stream,
                    true,
                )
                .await
                .unwrap();

            let (peer_id, connection) =
                upgrade::outbound(socket_addr, remote_fingerprint, config.keypair.clone()).await?;

            Ok((peer_id, connection))
        }
        .boxed_local())
    }

    fn poll(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        if let Some(event) = self.pending_events.pop_front() {
            return Poll::Ready(event);
        }

        Poll::Pending
    }
}

/// Extracts the relay address and target peer ID from a [`Multiaddr`].
fn extract_relay_and_target(addr: &Multiaddr) -> Option<(Multiaddr, PeerId)> {
    let components: Vec<_> = addr.iter().collect();

    for i in 0..components.len().saturating_sub(1) {
        if components[i] == Protocol::WebRTC && matches!(components[i + 1], Protocol::P2p(_)) {
            // Everything before /webrtc is the relayed multiaddr
            let relay_addr = components[..i]
                .iter()
                .fold(Multiaddr::empty(), |addr, proto| addr.with(proto.clone()));

            if let Protocol::P2p(peer_id) = &components[i + 1] {
                return Some((relay_addr, peer_id.clone()));
            }
        }
    }

    None
}

/// Extracts fingerprint from a [`Multiaddr`].
fn extract_fingerprint(addr: &Multiaddr) -> Result<Fingerprint, Error> {
    for proto in addr.iter() {
        if let Protocol::Certhash(hash) = proto {
            let digest_bytes = hash.digest();
            if digest_bytes.len() != 32 {
                return Err(Error::InvalidMultiaddr(format!(
                    "Invalid fingerprint length: {}",
                    digest_bytes.len()
                )));
            }
            let mut array = [0u8; 32];
            array.copy_from_slice(&digest_bytes);
            return Ok(Fingerprint::raw(array));
        }
    }
    Err(Error::InvalidMultiaddr(
        "No certificate fingerprint found in multiaddr".into(),
    ))
}

/// Extracts the socket address from a [`Multiaddr`].
fn extract_socket_addr(addr: &Multiaddr) -> Result<SocketAddr, Error> {
    let mut ip = None;
    let mut port = None;

    for proto in addr.iter() {
        match proto {
            Protocol::Ip4(ip_addr) => ip = Some(std::net::IpAddr::V4(ip_addr)),
            Protocol::Ip6(ip_addr) => ip = Some(std::net::IpAddr::V6(ip_addr)),
            Protocol::Tcp(p) | Protocol::Udp(p) => port = Some(p),
            _ => {}
        }
    }

    if let (Some(ip_addr), Some(port_num)) = (ip, port) {
        Ok(SocketAddr::new(ip_addr, port_num))
    } else {
        Err(Error::InvalidMultiaddr("Missing IP address or port".into()))
    }
}
