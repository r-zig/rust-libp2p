// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! A basic relay server and relay client implementation.
//!
//! The example below involves three nodes: (1) a relay server, (2) a listening
//! relay client listening via the relay server and (3) a dialing relay client
//! dialing the listening relay client via the relay server.
//!
//! 1. To start the relay server, run `cargo run --example=relay --package=libp2p-relay -- -d 1 --mode relay --address /ip4/<ip address>/tcp/<port>`.
//!    The -d help create static peer id using the given number argument
//!    The mode specify that the program run as a relay server
//!    The address specify a static address usually it will be some loop back address such as /ip4/0.0.0.0/tcp/4444
//!    Example:
//!    `cargo run --example=relay --package=libp2p-relay -- -d 1 --mode relay --address /ip4/0.0.0.0/tcp/4444`
//!
//! 2. To start the listening relay client run `cargo run --example=relay --package=libp2p-relay -- -d 2 --mode client-listen --address
//! <addr-relay-server>/p2p/<peer-id-relay-server>/p2p-circuit` in a second terminal where:
//!
//!   - `<addr-relay-server>` is replaced by one of the listening addresses of the relay server.
//!   - `<peer-id-relay-server>` is replaced by the peer id of the relay server.
//!
//!    Example:
//!    `cargo run --example=relay --package=libp2p-relay -- -d 2 --mode client-listen --address /ip4/127.0.0.1/tcp/4444/p2p/12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Rg3sBYbU1Nft9HyQ6X/p2p-circuit`
//!
//! 3. To start the dialing relay client run `cargo run --example=relay --package=libp2p-relay -- -d 3 --mode client-dial --address
//! <addr-relay-server>/p2p/<peer-id-relay-server>/p2p-circuit/p2p/<peer-id-listening-relay-client>` in
//! a third terminal where:
//!
//!   - `<addr-relay-server>` is replaced by one of the listening addresses of the relay server.
//!   - `<peer-id-relay-server>` is replaced by the peer id of the relay server.
//!   - `<peer-id-listening-relay-client>` is replaced by the peer id of the listening relay client.
//!    Example:
//!    `cargo run --example=relay --package=libp2p-relay -- -d 3 --mode client-dial --address /ip4/127.0.0.1/tcp/4444/p2p/12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Rg3sBYbU1Nft9HyQ6X/p2p-circuit/p2p/12D3KooWH3uVF6wv47WnArKHk5p6cvgCJEb74UTmxztmQDc298L3`
//!
//! In the third terminal you will see the dialing relay client to receive pings
//! from both the relay server AND from the listening relay client relayed via
//! the relay server.

use futures::executor::block_on;
use futures::stream::StreamExt;
use libp2p::ping::{Ping, PingConfig, PingEvent};
use libp2p::plaintext;
use libp2p::relay::{Relay, RelayConfig};
use libp2p::tcp::TcpConfig;
use libp2p::Transport;
use libp2p::{core::upgrade, identity::ed25519};
use libp2p::{identity, NetworkBehaviour, PeerId, Swarm};
use std::error::Error;
use std::task::{Context, Poll};
use std::time::Duration;
use std::{fmt, str::FromStr};
use structopt::StructOpt;

// Listen on all interfaces and whatever port the OS assigns
const DEFAULT_RELAY_ADDRESS: &str = "/ip4/0.0.0.0/tcp/0";
const RELAY_ADDRESS_KEY: &str = "RELAY_ADDRESS";
const CLIENT_LISTEN_ADDRESS_KEY: &str = "CLIENT_LISTEN_ADDRESS";

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let opt = Opt::from_args();
    println!("opt: {:?}", opt);

    // Create a static known PeerId based on given secret
    let local_key: identity::Keypair = generate_ed25519(opt.deterministic_peer_secret);
    let local_peer_id = PeerId::from(local_key.public());
    println!("Local peer id: {:?}", local_peer_id);

    let tcp_transport = TcpConfig::new();

    let relay_config = RelayConfig {
        connection_idle_timeout: Duration::from_secs(10 * 60),
        ..Default::default()
    };
    let (relay_wrapped_transport, relay_behaviour) =
        libp2p_relay::new_transport_and_behaviour(relay_config, tcp_transport);

    let behaviour = Behaviour {
        relay: relay_behaviour,
        ping: Ping::new(
            PingConfig::new()
                .with_keep_alive(true)
                .with_interval(Duration::from_secs(1)),
        ),
    };

    let plaintext = plaintext::PlainText2Config {
        local_public_key: local_key.public(),
    };

    let transport = relay_wrapped_transport
        .upgrade(upgrade::Version::V1)
        .authenticate(plaintext)
        .multiplex(libp2p_yamux::YamuxConfig::default())
        .boxed();

    let mut swarm = Swarm::new(transport, behaviour, local_peer_id);

    match opt.mode {
        Mode::Relay => {
            let address = get_relay_address(&opt);
            swarm.listen_on(address.parse()?)?;
            println!("starting listening as relay on {}", &address);
        }
        Mode::ClientListen => {
            let relay_address = get_relay_peer_address(&opt);
            swarm.listen_on(relay_address.parse()?)?;
            println!("starting client listener via relay on {}", &relay_address);
        }
        Mode::ClientDial => {
            let client_listen_address = get_client_listen_address(&opt);
            swarm.dial_addr(client_listen_address.parse()?)?;
            println!("starting as client dialer on {}", client_listen_address);
        }
    }

    let mut listening = false;
    block_on(futures::future::poll_fn(move |cx: &mut Context<'_>| {
        loop {
            match swarm.poll_next_unpin(cx) {
                Poll::Ready(Some(event)) => println!("{:?}", event),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => {
                    if !listening {
                        for addr in Swarm::listeners(&swarm) {
                            println!("Listening on {:?}", addr);
                            listening = true;
                            publish_listener_peer(addr, &opt.mode, local_peer_id);
                        }
                    }
                    break;
                }
            }
        }
        Poll::Pending
    }))
}

fn publish_listener_peer(addr: &libp2p::Multiaddr, mode: &Mode, local_peer_id: PeerId) -> () {
    match mode {
        Mode::Relay => {
            let address = format!("{}/p2p/{}/p2p-circuit", addr, local_peer_id);
            set_shared_data(RELAY_ADDRESS_KEY, &address);
        }
        Mode::ClientListen => {
            let address = format!("/p2p/{}/{}", addr, local_peer_id);
            set_shared_data(CLIENT_LISTEN_ADDRESS_KEY, &address);
        }
        Mode::ClientDial => {}
    }
}

fn generate_ed25519(deterministic_peer_secret: u8) -> identity::Keypair {
    let mut bytes = [0u8; 32];
    bytes[0] = deterministic_peer_secret;

    let secret_key = ed25519::SecretKey::from_bytes(&mut bytes)
        .expect("this returns `Err` only if the length is wrong; the length is correct; qed");
    identity::Keypair::Ed25519(secret_key.into())
}

/// Get the address for relay mode
fn get_relay_address(opt: &Opt) -> String {
    match &opt.address {
        Some(address) => address.clone(),
        None => {
            println!("--address argument was not provided, will use the default listening relay address: {}",DEFAULT_RELAY_ADDRESS);
            DEFAULT_RELAY_ADDRESS.to_string()
        }
    }
}

/// Get the address for client_listen mode
fn get_relay_peer_address(opt: &Opt) -> String {
    match &opt.address {
        Some(address) => address.clone(),
        None => {
            match get_shared_data(RELAY_ADDRESS_KEY) {
                Some(address) => address,
                None => panic!("Please provide relayed listen address such as: <addr-relay-server>/p2p/<peer-id-relay-server>/p2p-circuit"),
            }
        }
    }
}

/// Get the address for client-dial mode
fn get_client_listen_address(opt: &Opt) -> String {
    match &opt.address {
        Some(address) => address.clone(),
        None => {
            match get_shared_data(CLIENT_LISTEN_ADDRESS_KEY) {
                Some(address) => address.to_string(),
                None => panic!("Please provide client listen address such as: <addr-relay-server>/p2p/<peer-id-relay-server>/p2p-circuit/p2p/<peer-id-listening-relay-client>"),
            }
        }
    }
}

/// Placeholder to publish the peer address based on given key
fn set_shared_data(key: &str, value: &str) -> () {
    println!(
        "TODO publish to KAD or discovery service ... Set the key: {} data to:{}",
        key, value
    );
}

/// Placeholder to get the peer information based on given key
fn get_shared_data(key: &str) -> Option<String> {
    println!(
        "TODO get the data related to key: {} from KAD or discovery service.",
        key
    );
    None
}

#[derive(NetworkBehaviour)]
#[behaviour(out_event = "Event", event_process = false)]
struct Behaviour {
    relay: Relay,
    ping: Ping,
}

#[derive(Debug)]
enum Event {
    Relay(()),
    Ping(PingEvent),
}

impl From<PingEvent> for Event {
    fn from(e: PingEvent) -> Self {
        Event::Ping(e)
    }
}

impl From<()> for Event {
    fn from(_: ()) -> Self {
        Event::Relay(())
    }
}

#[derive(Debug, StructOpt)]
enum Mode {
    Relay,
    ClientListen,
    ClientDial,
}

impl FromStr for Mode {
    type Err = ModeError;
    fn from_str(mode: &str) -> Result<Self, Self::Err> {
        match mode {
            "relay" => Ok(Mode::Relay),
            "client-listen" => Ok(Mode::ClientListen),
            "client-dial" => Ok(Mode::ClientDial),
            _ => Err(ModeError {}),
        }
    }
}

#[derive(Debug)]
struct ModeError {}
impl Error for ModeError {}
impl fmt::Display for ModeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Could not parse a mode")
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = "libp2p relay")]
struct Opt {
    /// The mode (relay, client-listen, client-dial)
    #[structopt(long)]
    mode: Mode,

    /// Fixed value to generate deterministic peer id
    #[structopt(short = "d")]
    deterministic_peer_secret: u8,

    /// The listening address
    #[structopt(long)]
    address: Option<String>,
}