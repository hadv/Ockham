use crate::types::{Block, Transaction, Vote};
use futures::StreamExt;
use libp2p::{
    Multiaddr, gossipsub, mdns, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux,
};
use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::time::Duration;
use tokio::sync::mpsc;

/// Network Behaviour combining Gossipsub (for consensus messages) and mDNS (for local discovery).
#[derive(NetworkBehaviour)]
pub struct SimplexBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub mdns: mdns::tokio::Behaviour,
}

/// Events emitted by the Network module to the application.
#[derive(Debug)]
pub enum NetworkEvent {
    VoteReceived(Vote),
    BlockReceived(Block),
    TransactionReceived(Transaction),
    SyncMessageReceived(crate::types::SyncMessage, String), // Message + PeerId
    PeerConnected(String),
}

/// Commands sent from the application to the Network module.
#[derive(Debug)]
enum NetworkCommand {
    Broadcastblock(Block),
    BroadcastVote(Vote),
    BroadcastTransaction(Transaction),
    BroadcastSync(crate::types::SyncMessage),
    Dial(Multiaddr),
}

/// The Network Interface.
/// Manages the `Swarm` in a background task and communicates via channels.
pub struct Network {
    command_sender: mpsc::Sender<NetworkCommand>,
    event_receiver: mpsc::Receiver<NetworkEvent>,
}

impl Network {
    pub async fn new(port: u16) -> Result<Self, Box<dyn Error>> {
        let (command_sender, mut command_receiver) = mpsc::channel(100);
        let (event_sender, event_receiver) = mpsc::channel(100);

        // 1. Setup Swarm
        let mut swarm = libp2p::SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| {
                // Gossipsub configuration
                let message_id_fn = |message: &gossipsub::Message| {
                    let mut s = DefaultHasher::new();
                    message.data.hash(&mut s);
                    gossipsub::MessageId::from(s.finish().to_string())
                };
                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_millis(200)) // Very fast heartbeat for low latency start
                    .history_length(10) // Keep message history longer to relay to late joiners
                    .history_gossip(10) // Advertise history to more peers
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .message_id_fn(message_id_fn)
                    .build()
                    .map_err(std::io::Error::other)?;

                let gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossipsub_config,
                )?;

                // mDNS configuration
                let mdns = mdns::tokio::Behaviour::new(
                    mdns::Config::default(),
                    key.public().to_peer_id(),
                )?;

                Ok(SimplexBehaviour { gossipsub, mdns })
            })?
            .build();

        // 1b. Listen on localhost with specified port
        let addr = format!("/ip4/127.0.0.1/tcp/{}", port).parse()?;
        swarm.listen_on(addr)?;

        // 2. Subscribe to topics
        let topic = gossipsub::IdentTopic::new("simplex-consensus");
        swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

        // 3. Spawn background Task
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    event = swarm.select_next_some() => match event {
                        SwarmEvent::NewListenAddr { address, .. } => {
                            println!("Swarm listening on {address:?}");
                        },
                        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                            println!("Connection established with peer: {peer_id}");
                            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                            let _ = event_sender.send(NetworkEvent::PeerConnected(peer_id.to_string())).await;
                        },
                        SwarmEvent::OutgoingConnectionError { error, .. } => {
                            println!("Outgoing connection error: {error:?}");
                        },
                        SwarmEvent::Behaviour(SimplexBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                            for (peer_id, _multiaddr) in list {
                                println!("mDNS discovered a new peer: {peer_id}");
                                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                                let _ = event_sender.send(NetworkEvent::PeerConnected(peer_id.to_string())).await;
                            }
                        },
                        SwarmEvent::Behaviour(SimplexBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                             for (peer_id, _multiaddr) in list {
                                println!("mDNS discover peer has expired: {peer_id}");
                                swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                            }
                        },
                        SwarmEvent::Behaviour(SimplexBehaviourEvent::Gossipsub(gossipsub::Event::Message { propagation_source: _peer_id, message_id: _id, message })) => {
                            // Deserialize message
                             if let Ok(block) = serde_json::from_slice::<Block>(&message.data) {
                                 let _ = event_sender.send(NetworkEvent::BlockReceived(block)).await;
                             } else if let Ok(vote) = serde_json::from_slice::<Vote>(&message.data) {
                                 let _ = event_sender.send(NetworkEvent::VoteReceived(vote)).await;
                             } else if let Ok(tx) = serde_json::from_slice::<Transaction>(&message.data) {
                                let _ = event_sender.send(NetworkEvent::TransactionReceived(tx)).await;
                             } else if let Ok(sync_msg) = serde_json::from_slice::<crate::types::SyncMessage>(&message.data) {
                                let peer_id = message.source.map(|p| p.to_string()).unwrap_or_default();
                                let _ = event_sender.send(NetworkEvent::SyncMessageReceived(sync_msg, peer_id)).await;
                             }
                        },
                        _ => {}
                    },
                    command = command_receiver.recv() => match command {
                        Some(NetworkCommand::Broadcastblock(block)) => {
                            let data = serde_json::to_vec(&block).unwrap();
                            let topic = gossipsub::IdentTopic::new("simplex-consensus");
                             if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, data) {
                                match e {
                                    gossipsub::PublishError::Duplicate => {},
                                    _ => println!("Publish error: {e:?}"),
                                }
                             }
                        },
                         Some(NetworkCommand::BroadcastVote(vote)) => {
                              let data = serde_json::to_vec(&vote).unwrap();
                              let topic = gossipsub::IdentTopic::new("simplex-consensus");
                              if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, data) {
                                 match e {
                                     gossipsub::PublishError::Duplicate => {},
                                     _ => println!("Publish error: {e:?}"),
                                 }
                              }
                         },
                          Some(NetworkCommand::BroadcastTransaction(tx)) => {
                               let data = serde_json::to_vec(&tx).unwrap();
                               let topic = gossipsub::IdentTopic::new("simplex-consensus");
                               if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, data) {
                                  match e {
                                      gossipsub::PublishError::Duplicate => {},
                                      _ => println!("Publish error: {e:?}"),
                                  }
                               }
                          },
                         Some(NetworkCommand::BroadcastSync(msg)) => {
                              let data = serde_json::to_vec(&msg).unwrap();
                              let topic = gossipsub::IdentTopic::new("simplex-consensus");
                              if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, data) {
                                 match e {
                                     gossipsub::PublishError::Duplicate => {},
                                     _ => println!("Publish error: {e:?}"),
                                 }
                              }
                         },
                        Some(NetworkCommand::Dial(addr)) => {
                             if let Err(e) = swarm.dial(addr) {
                                println!("Dial error: {e:?}");
                             }
                        },
                        None => break, // Channel closed
                    }
                }
            }
        });

        Ok(Network {
            command_sender,
            event_receiver,
        })
    }

    pub async fn dial(&self, addr: &str) {
        if let Ok(multiaddr) = addr.parse() {
            let _ = self
                .command_sender
                .send(NetworkCommand::Dial(multiaddr))
                .await;
        }
    }

    pub async fn broadcast_block(&self, block: Block) {
        let _ = self
            .command_sender
            .send(NetworkCommand::Broadcastblock(block))
            .await;
    }

    pub async fn broadcast_vote(&self, vote: Vote) {
        let _ = self
            .command_sender
            .send(NetworkCommand::BroadcastVote(vote))
            .await;
    }

    pub async fn broadcast_sync(&self, msg: crate::types::SyncMessage) {
        let _ = self
            .command_sender
            .send(NetworkCommand::BroadcastSync(msg))
            .await;
    }

    pub async fn broadcast_transaction(&self, tx: Transaction) {
        let _ = self
            .command_sender
            .send(NetworkCommand::BroadcastTransaction(tx))
            .await;
    }

    pub async fn next_event(&mut self) -> Option<NetworkEvent> {
        self.event_receiver.recv().await
    }
}
