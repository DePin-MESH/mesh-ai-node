use clap::Parser;
use futures::prelude::*;
use libp2p::{
    StreamProtocol, identify,
    multiaddr::{Multiaddr, Protocol},
    noise, ping, relay,
    request_response::{self, ProtocolSupport},
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use mesh_ai_node::{PromptRequest, PromptResponse};
use std::{error::Error, time::Duration};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "mesh-ai-node")]
struct Opt {
    /// Relay address to connect to.
    /// Example: /ip4/127.0.0.1/tcp/xxxxx/p2p/xxxxx
    #[arg(long)]
    relay_address: Option<Multiaddr>,
}

#[derive(NetworkBehaviour)]
struct MyBehaviour {
    ping: ping::Behaviour,
    request_response: request_response::cbor::Behaviour<PromptRequest, PromptResponse>,
    relay: relay::client::Behaviour,
    identify: identify::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let opt = Opt::parse();

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key, relay_behaviour| MyBehaviour {
            ping: ping::Behaviour::default(),
            request_response: request_response::cbor::Behaviour::new(
                [(StreamProtocol::new("/mesh-ai/1.0.0"), ProtocolSupport::Full)],
                request_response::Config::default(),
            ),
            relay: relay_behaviour,
            identify: identify::Behaviour::new(identify::Config::new(
                "/mesh-ai/1.0.0".to_string(),
                key.public(),
            )),
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(u64::MAX)))
        .build();

    println!("Local PeerID: {}", swarm.local_peer_id());

    let relay_addr_opt = opt.relay_address.clone();
    let mut listening_on_relay = false;

    if let Some(ref relay_addr) = relay_addr_opt {
        println!("Connecting to relay at {relay_addr}");
        swarm.dial(relay_addr.clone())?;
    } else {
        swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
    }

    println!("Node started. Waiting for connections...");

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                println!("Connection established with {peer_id}");
                // If we have a relay address and haven't started listening yet
                if let Some(ref relay_addr) = relay_addr_opt {
                    if !listening_on_relay {
                        // Extract relay peer id from the address
                        let relay_peer_id_from_addr = relay_addr.iter().find_map(|p| {
                            if let Protocol::P2p(id) = p {
                                Some(id)
                            } else {
                                None
                            }
                        });

                        if relay_peer_id_from_addr == Some(peer_id) {
                            println!("Connected to relay. Starting to listen via relay...");
                            let listen_addr = relay_addr.clone().with(Protocol::P2pCircuit);
                            if let Err(e) = swarm.listen_on(listen_addr) {
                                eprintln!("Failed to listen on relay: {e}");
                            } else {
                                listening_on_relay = true;
                            }
                        }
                    }
                }
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("Listening on {address:?}");
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::RequestResponse(
                request_response::Event::Message {
                    peer,
                    message:
                        request_response::Message::Request {
                            request, channel, ..
                        },
                    ..
                },
            )) => {
                println!("Received request from {peer:?}: {}", request.prompt);

                // Call Ollama
                let response_text = call_ollama(request.prompt).await.unwrap_or_else(|e| {
                    eprintln!("Ollama error: {e}");
                    format!("Error calling Ollama: {e}")
                });

                let _ = swarm.behaviour_mut().request_response.send_response(
                    channel,
                    PromptResponse {
                        response: response_text,
                    },
                );
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Ping(event)) => {
                println!("Ping event: {event:?}");
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Relay(event)) => {
                println!("Relay event: {event:?}");
            }
            _ => {}
        }
    }
}

async fn call_ollama(prompt: String) -> Result<String, Box<dyn Error>> {
    let client = reqwest::Client::new();
    let res = client
        .post("http://localhost:11434/api/generate")
        .json(&serde_json::json!({
            "model": "deepseek-coder:1.3b",
            "prompt": prompt,
            "stream": false
        }))
        .send()
        .await?;

    if !res.status().is_success() {
        return Err(format!("Ollama returned error: {}", res.status()).into());
    }

    let body: serde_json::Value = res.json().await?;
    Ok(body["response"]
        .as_str()
        .unwrap_or("No response")
        .to_string())
}

//Q:
//how the swarm make sures that the peers identify each other one thing is its in the same private network so i think
