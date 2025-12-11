use ockham::consensus::{ConsensusAction, SimplexState};
use ockham::crypto::{PrivateKey, PublicKey};
use ockham::network::{Network, NetworkEvent};
use std::env;
use std::time::Duration;
use tokio::time;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // 1. Parse Node ID from args (0, 1, 2, 3)
    let args: Vec<String> = env::args().collect();
    let id_arg = args
        .get(1)
        .expect("Usage: cargo run -- <node_id>")
        .parse::<u64>()?;
    let my_id = PublicKey(id_arg);
    let my_key = PrivateKey(id_arg);

    log::info!("Starting Node {}", id_arg);

    // 2. Setup Committee (Fixed 4 nodes for prototype)
    let committee: Vec<PublicKey> = (0..4).map(PublicKey).collect();

    // 3. Initialize Network
    // Node 0 Listen on 9000, others random (0)
    let port = if id_arg == 0 { 9000 } else { 0 };
    let mut network = Network::new(port).await?;

    // Bootnode logic: If not node 0, dial node 0
    if id_arg != 0 {
        log::info!("Dialing bootnode...");
        network.dial("/ip4/127.0.0.1/tcp/9000").await;
    }

    // 4. Initialize Consensus State
    let mut state = SimplexState::new(my_id, my_key, committee.clone());

    // 5. Timer for Views (Simple timeout for prototype)
    let mut view_timer = time::interval(Duration::from_secs(30));

    // State for startup synchronization
    let mut connected_peers = 0;
    let mut consensus_started = false;

    // 6. Main Event Loop
    loop {
        tokio::select! {
            // A. Network Events
            Some(event) = network.next_event() => {
                let actions = match event {
                    NetworkEvent::VoteReceived(vote) => {
                        log::info!("Received Vote View {} from {:?}", vote.view, vote.author);
                        state.on_vote(vote)
                    }
                    NetworkEvent::BlockReceived(block) => {
                        log::info!("Received Block: {:?}", block);
                        state.on_proposal(block)
                    }
                    NetworkEvent::PeerConnected(pid) => {
                        log::info!("Peer Connected: {}", pid);
                        connected_peers += 1;
                        if connected_peers >= 1 && !consensus_started {
                            log::info!("Enough peers connected ({}). Starting Consensus!", connected_peers);
                            consensus_started = true;
                            // Reset timer to align with start
                            view_timer.reset();

                            // Check if WE are the leader for View 1 and propose immediately!
                             if let Ok(initial_actions) = state.try_propose() {
                                 // Process immediate proposal actions
                                 let mut queue = initial_actions;
                                 while let Some(action) = queue.pop() {
                                     match action {
                                         ConsensusAction::BroadcastVote(vote) => { network.broadcast_vote(vote).await; }
                                         ConsensusAction::BroadcastBlock(block) => {
                                             log::info!("Broadcasting Block: {:?}", block);
                                             network.broadcast_block(block).await;
                                         }
                                     }
                                 }
                             }
                        }
                        Ok(vec![])
                    }
                };

                match actions {
                    Ok(mut action_queue) => {
                        if consensus_started {
                             while let Some(action) = action_queue.pop() {
                                 match action {
                                     ConsensusAction::BroadcastVote(vote) => {
                                         log::info!("Broadcasting Vote for View {}", vote.view);
                                         network.broadcast_vote(vote.clone()).await;

                                         // Loopback: Apply own vote locally
                                         if let Ok(new_actions) = state.on_vote(vote) {
                                             action_queue.extend(new_actions);
                                         }
                                     }
                                     ConsensusAction::BroadcastBlock(block) => {
                                         log::info!("Broadcasting Block: {:?}", block);
                                         network.broadcast_block(block).await;
                                     }
                                 }
                             }
                        }
                    },
                    Err(e) => log::error!("Consensus Error: {:?}", e),
                }
            }

            // B. Timer (Timeout -> Dummy Block)
            _ = view_timer.tick() => {
                if !consensus_started {
                    continue;
                }

                // View Timeout processing
                match state.on_timeout(state.current_view) {
                     Ok(mut action_queue) => {
                         while let Some(action) = action_queue.pop() {
                             match action {
                                 ConsensusAction::BroadcastVote(vote) => {
                                     log::info!("Broadcasting Vote for View {}", vote.view);
                                     network.broadcast_vote(vote.clone()).await;
                                     if let Ok(new_actions) = state.on_vote(vote) {
                                         action_queue.extend(new_actions);
                                     }
                                 }
                                 ConsensusAction::BroadcastBlock(block) => {
                                     log::info!("Broadcasting Block: {:?}", block);
                                     network.broadcast_block(block).await;
                                 }
                             }
                         }
                     },
                     Err(e) => log::error!("Timeout Error: {:?}", e),
                }
            }
        }
    }
}
