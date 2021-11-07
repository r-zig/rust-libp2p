// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! A basic relay server version 2 implementation.
//!
//! The example below involves three nodes: (1) a relay server, (2) a listening
//! dcutr-client listening via the relay server and (3) a dialing dcutr-client
//! dialing the listening listening client via the relay server.
//!
//! 1. To start the relay server, run `cargo run --example=relay_v2 --package=libp2p-relay --secret-key-seed <seed number> --port <port number>`.
//!    The `-secret-key-seed` helps create a static peer id using the given number argument as a seed.
//!    The port specifies the port number used to listen for incoming requests with the loop back address. such as `/ip4/0.0.0.0/tcp/3333`.
//!    IPV4 Example:
//!    `cargo run --example=relay_v2 --package=libp2p-relay --secret-key-seed 1 --port 3333`
//!
//! 2. To start the dcutr-client run `cargo run --example=client --package=libp2p-dcutr --
//! --mode listen --secret-key-seed <see number>
//! --port <port number>
//! --address <addr-relay-server>/p2p/<peer-id-relay-server>/p2p-circuit`
//! -x <external ip address> in a second terminal where:
//!
//!   - `<addr-relay-server>` is replaced by one of the listening addresses of the relay server.
//!   - `<peer-id-relay-server>` is replaced by the peer id of the relay server.
//!
//!    IPV4 local network example:
//!    `cargo run --example=client --package=libp2p-dcutr --
//! --mode listen
//! --secret-key-seed 2
//! --port 4444
//! --address /ip4/127.0.0.1/tcp/3333/p2p/12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Rg3sBYbU1Nft9HyQ6X/p2p-circuit
//! -x 111.212.22.134`
//!
//! 3. To start the dialing dcutr-client run `cargo run --example=client --package=libp2p-dcutr --
//! --mode dial --secret-key-seed <see number>
//! --port <port number>
//! --address <addr-relay-server>/p2p/<peer-id-relay-server>/p2p-circuit`
//! -x <external ip address> in a second terminal where:` in
//! a third terminal where:
//!
//!   - `<addr-relay-server>` is replaced by one of the listening addresses of the relay server.
//!   - `<peer-id-relay-server>` is replaced by the peer id of the relay server.
//!   - `<peer-id-listening-relay-client>` is replaced by the peer id of the listening dcutr client.
//!    IPV4 local network example:
//!    `cargo run --example=client --package=libp2p-dcutr -- --mode dial --secret-key-seed 3 --port 5555 --address /ip4/127.0.0.1/tcp/3333/p2p/12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Rg3sBYbU1Nft9HyQ6X/p2p-circuit/p2p/12D3KooWH3uVF6wv47WnArKHk5p6cvgCJEb74UTmxztmQDc298L3 -x 111.212.22.134`
//!
//! In the third terminal you will see the dialing dcutr client to receive pings
//! from the listening dcutr client listener
//!
use futures::executor::block_on;
use futures::stream::StreamExt;
use libp2p::identify::{Identify, IdentifyConfig, IdentifyEvent};
use libp2p::multiaddr::Protocol;
use libp2p::ping::{Ping, PingConfig, PingEvent};
use libp2p::relay::v2::relay::{self, Relay};
use libp2p::swarm::{Swarm, SwarmEvent};
use libp2p::tcp::TcpConfig;
use libp2p::Transport;
use libp2p::{identity, NetworkBehaviour, PeerId};
use libp2p::{noise, Multiaddr};
use std::error::Error;
use std::net::{Ipv4Addr, Ipv6Addr};
use structopt::StructOpt;

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let opt = Opt::from_args();
    println!("opt: {:?}", opt);

    // Create a static known PeerId based on given secret
    let local_key: identity::Keypair = generate_ed25519(opt.secret_key_seed);
    let local_peer_id = PeerId::from(local_key.public());
    println!("Local peer id: {:?}", local_peer_id);

    let tcp_transport = TcpConfig::new();

    let noise_keys = noise::Keypair::<noise::X25519Spec>::new()
        .into_authentic(&local_key)
        .expect("Signing libp2p-noise static DH keypair failed.");

    let transport = tcp_transport
        .upgrade()
        .authenticate(noise::NoiseConfig::xx(noise_keys).into_authenticated())
        .multiplex(libp2p_yamux::YamuxConfig::default())
        .boxed();

    let behaviour = Behaviour {
        relay: Relay::new(local_peer_id, Default::default()),
        ping: Ping::new(PingConfig::new()),
        identify: Identify::new(IdentifyConfig::new(
            "/TODO/0.0.1".to_string(),
            local_key.public(),
        )),
    };

    let mut swarm = Swarm::new(transport, behaviour, local_peer_id);

    // Listen on all interfaces
    let listen_addr = Multiaddr::empty()
        .with(match opt.use_ipv6 {
            Some(true) => Protocol::from(Ipv6Addr::UNSPECIFIED),
            _ => Protocol::from(Ipv4Addr::UNSPECIFIED),
        })
        .with(Protocol::Tcp(opt.port));
    swarm.listen_on(listen_addr)?;

    block_on(async {
        loop {
            match swarm.next().await.expect("Infinite Stream.") {
                SwarmEvent::Behaviour(Event::Relay(event)) => {
                    println!("{:?}", event)
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    println!("Listening on {:?}", address);
                }
                _ => {}
            }
        }
    })
}

#[derive(NetworkBehaviour)]
#[behaviour(out_event = "Event", event_process = false)]
struct Behaviour {
    relay: Relay,
    ping: Ping,
    identify: Identify,
}

#[derive(Debug)]
enum Event {
    Ping(PingEvent),
    Identify(IdentifyEvent),
    Relay(relay::Event),
}

impl From<PingEvent> for Event {
    fn from(e: PingEvent) -> Self {
        Event::Ping(e)
    }
}

impl From<IdentifyEvent> for Event {
    fn from(e: IdentifyEvent) -> Self {
        Event::Identify(e)
    }
}

impl From<relay::Event> for Event {
    fn from(e: relay::Event) -> Self {
        Event::Relay(e)
    }
}

fn generate_ed25519(secret_key_seed: u8) -> identity::Keypair {
    let mut bytes = [0u8; 32];
    bytes[0] = secret_key_seed;

    let secret_key = identity::ed25519::SecretKey::from_bytes(&mut bytes)
        .expect("this returns `Err` only if the length is wrong; the length is correct; qed");
    identity::Keypair::Ed25519(secret_key.into())
}

#[derive(Debug, StructOpt)]
#[structopt(name = "libp2p relay")]
struct Opt {
    /// Determine if the relay listen on ipv6 or ipv4 loopback address. the default is ipv4
    #[structopt(long)]
    use_ipv6: Option<bool>,

    /// Fixed value to generate deterministic peer id
    #[structopt(long)]
    secret_key_seed: u8,

    /// The port used to listen on all interfaces
    #[structopt(long)]
    port: u16,
}
