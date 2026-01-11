use futures::prelude::*;
use libp2p::{
    Multiaddr, StreamProtocol, noise, ping,
    request_response::{self, ProtocolSupport},
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use mesh_ai_node::{PromptRequest, PromptResponse};
use std::{error::Error, time::Duration};
use tracing_subscriber::EnvFilter;

#[derive(NetworkBehaviour)]
struct MyBehaviour {
    ping: ping::Behaviour,
    request_response: request_response::cbor::Behaviour<PromptRequest, PromptResponse>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|_| MyBehaviour {
            ping: ping::Behaviour::default(),
            request_response: request_response::cbor::Behaviour::new(
                [(StreamProtocol::new("/mesh-ai/1.0.0"), ProtocolSupport::Full)],
                request_response::Config::default(),
            ),
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(u64::MAX)))
        .build();

    // Dial the peer identified by the multi-address given as the second
    // command-line argument.
    let target_addr: Multiaddr = std::env::args()
        .nth(1)
        .ok_or("Expected multiaddr as argument")?
        .parse()?;

    swarm.dial(target_addr.clone())?;
    println!("Dialed {target_addr}");

    let mut prompt_sent = false;

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                println!("Connected to {peer_id}");
                if !prompt_sent {
                    let prompt = "What is the capital of France?".to_string();
                    println!("Sending prompt: {prompt}");
                    swarm
                        .behaviour_mut()
                        .request_response
                        .send_request(&peer_id, PromptRequest { prompt });
                    prompt_sent = true;
                }
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::RequestResponse(
                request_response::Event::Message {
                    peer,
                    message: request_response::Message::Response { response, .. },
                    ..
                },
            )) => {
                println!("Received response from {peer}: {}", response.response);
                return Ok(());
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Ping(event)) => {
                println!("Ping event: {event:?}");
            }
            SwarmEvent::OutgoingConnectionError { error, .. } => {
                eprintln!("Connection error: {error}");
                return Err(error.into());
            }
            _ => {}
        }
    }
}
