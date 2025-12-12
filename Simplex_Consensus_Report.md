# Simplex Consensus: A Comprehensive Architectural Framework for Next-Generation Blockchain Implementation

## 1. Introduction and Theoretical Foundations

The evolution of distributed consensus protocols has followed a trajectory from the classical stability of Paxos and PBFT to the dynamic, permissionless environments of modern blockchains. In the context of building a new blockchain network from scratch, the selection of the consensus engine is the single most critical architectural decision. It dictates not only the security model but also the throughput, latency, and operational complexity of the entire system. This report provides an exhaustive technical blueprint for implementing the Simplex Consensus protocol, a novel partially synchronous consensus mechanism that offers a breakthrough in the efficiency-simplicity trade-off space.

The Simplex protocol, as detailed in recent theoretical literature, addresses a specific inefficiency in the current state-of-the-art: the tension between optimistic confirmation time and the complexity of liveness recovery. Traditional protocols like PBFT achieve low latency but rely on complex view-change subroutines that are notoriously difficult to implement correctly. Newer streamlined protocols like HotStuff or PaLa simplify the "steady state" but often introduce "hidden lock" problems or require multi-round commit rules that degrade performance under faults. Simplex offers a compelling alternative: it achieves optimal optimistic confirmation time ($3\delta$) and optimal block time ($2\delta$) while maintaining a liveness proof that is conceptually simpler than its predecessors, relying on a unique "dummy block" mechanism to handle leader failures without a dedicated view-change phase.

This document serves as a definitive implementation guide for engineering a Simplex-based blockchain. It bridges the gap between the theoretical proofs found in the Simplex paper and the practical realities of systems engineering. We will explore the construction of the network stack using libp2p, the cryptographic layer using BLS12-381 signatures for aggregation, and the consensus state machine using high-performance asynchronous runtimes. The goal is to provide a roadmap that leverages available open-source libraries to construct a system that is robust, scalable, and mathematically proven.

### 1.1 The Partially Synchronous Model and Simplex Advantages

To understand the engineering constraints, we must first ground ourselves in the network model. Simplex operates in the partially synchronous setting. This model assumes that there exists a bound $\Delta$ on message delivery delay, but this bound is unknown to the protocol, and the network may behave asynchronously (messages delayed arbitrarily) until some unknown Global Stabilization Time (GST).

The elegance of Simplex lies in how it handles the "optimistic" case versus the "pessimistic" case. Most rotating-leader protocols degrade significantly when a leader is faulty. For instance, HotStuff variants might require a new leader to wait or gather extra certificates to ensure safety. Simplex, however, treats a leader failure as a standard state transition involving a "dummy block". This dummy block is not merely a timeout artifact; it is a first-class citizen of the blockchain, cryptographically notarized just like a data block. This design choice implies that the blockchain structure itself records the "heartbeat" of the network, preserving the sequence of views even when no transactions are processed.

The table below illustrates the theoretical positioning of Simplex against other prominent protocols, highlighting why it is the superior choice for a new implementation focusing on latency and simplicity.

| Protocol | Optimistic Confirmation | Optimistic Block Time | Pessimistic Liveness (Expected) | Communication Complexity |
|:---|:---|:---|:---|:---|
| **Simplex** | $3\delta$ | $2\delta$ | $3.5\delta + 1.5\Delta$ | Data derived from comparative analysis in the Simplex research paper. |
| Arweave TX | $3\delta$ | $3\delta$ | $4\delta + 2\Delta$ | $O(n)$ |
| HotStuff (Chained) | $7\delta$ | $2\delta$ | $19.31\delta + 12.18\Delta$ | $O(n)$ |
| Jolteon | $5\delta$ | $2\delta$ | $10.87\delta + 9.5\Delta$ | $O(n)$ |
| PBFT | $3\delta$ | - | - | $O(n^2)$ |

The implication of these metrics for our implementation is profound. The $3\delta$ optimistic confirmation time means that under normal network conditions, a transaction is finalized as fast as the network allows (three network hops), independent of the worst-case timeout parameter $\Delta$. This property, known as Optimistic Responsiveness, is a mandatory requirement for a high-performance blockchain. It ensures that if we engineer our P2P layer to be fast (low $\delta$), the user experience improves linearly, regardless of how conservatively we set the safety timeout $\Delta$.

### 1.2 System Scope and Design Philosophy

Building this system "from scratch" requires a layered architecture. We are not simply writing a consensus script; we are building a distributed operating system. The architecture is guided by the Actor Model, where distinct components (Consensus, Network, Mempool, Storage) run as concurrent actors communicating via asynchronous channels. This is crucial because the Simplex protocol relies on precise timing—specifically the $3\Delta$ timer for dummy block generation. A monolithic, single-threaded design would risk blocking the timer on I/O operations, leading to inadvertent liveness failures.

The proposed stack leverages the Rust programming language ecosystem, chosen for its memory safety and the maturity of its blockchain-related crates (libraries). Specifically, we will architect the system around the **Tokio** asynchronous runtime for concurrency, **libp2p** for the networking layer, **blst** for high-performance BLS signatures, and **RocksDB** for persistent storage. This selection aligns with the user's request to leverage available libraries, providing a production-grade foundation while allowing us to implement the custom Simplex logic at the core.

## 2. The Simplex Consensus Protocol Specification

At the heart of our blockchain is the Simplex consensus engine. Unlike standard implementations of Raft or PBFT, Simplex introduces a specific "Notarize-Finalize" cadence and a "Dummy Block" recovery mechanism that necessitates a bespoke state machine design. This section translates the formal protocol description into engineering logic.

### 2.1 The Two-Phase Voting Mechanism

The defining characteristic of Simplex is its dual voting path. In many streamed protocols like Streamlet or HotStuff, voters only send one type of vote. Simplex requires two: a **Notarization Vote** and a **Finalization Vote**. This distinction is what allows the protocol to achieve consistency without complex locking rules.

The lifecycle of a view $h$ proceeds as follows:
1.  **Proposal**: The designated leader $L_h$ proposes a block $b_h$.
2.  **Notarization (Vote 1)**: Validators verify the block and multicast a vote message. If a block receives $2n/3$ votes, it becomes **Notarized**.
3.  **Finalization (Vote 2)**: Upon seeing a Notarized block for view $h$, validators immediately advance to view $h+1$. Crucially, if their local timer for view $h$ has not yet expired, they multicast a finalize message for view $h$. If a block gathers $2n/3$ finalize messages, it is **Finalized**—irreversibly committed to the ledger.

This two-step process implies that our consensus engine must handle two distinct aggregate types: the Notarization QC (which justifies the parent of the next block) and the Finalization QC (which triggers the commitment of the current block to the database). The engineering challenge here is that these events happen asynchronously. A node might receive the Notarization QC for block $h$ (allowing it to proceed to $h+1$) before it receives the Finalization QC for block $h$. Our state machine must be non-blocking with respect to finalization; it must allow the chain to grow optimistically based on Notarizations while "back-filling" the Finalization status as those certificates arrive.

### 2.2 The Dummy Block Mechanism

The most unique aspect of Simplex is the Dummy Block $\perp_h$. In traditional BFT, a timeout triggers a "View Change" message. Nodes stop processing blocks and exchange view-change messages until a "New View" certificate is formed. This is a complex, distinct protocol mode.

In Simplex, a timeout is simply a vote for a specific object: the Dummy Block.
*   **Trigger**: Upon entering view $h$, a node starts a timer $T_h$ with duration $3\Delta$.
*   **Action**: If $T_h$ expires before the node sees a valid proposal or QC, the node multicasts $\langle vote, h, \perp_h \rangle$.
*   **Aggregation**: If $2n/3$ nodes timeout, they generate a QC for the Dummy Block.
*   **Result**: This Dummy QC serves as the valid "parent" for the proposal in view $h+1$.

From an implementation perspective, this simplifies the codebase immensely. There is no "View Change" state. There is only the "Voting" state. The object being voted on effectively switches from "Proposed Block" to "Dummy Block" based on the timer. The data structure for a Block must therefore support a variant type: `Block::Standard(Data)` or `Block::Dummy`. The `Block::Dummy` variant carries no transactions but possesses a valid lineage (Parent Hash) and a valid QC, maintaining the cryptographic chain of custody.

### 2.3 The Quorum Intersection and Safety Argument

The safety of the system relies on the Quorum Intersection Lemma. The protocol guarantees that for any height $h$, it is impossible to have both a Notarized Dummy Block and a Finalized Standard Block. This is because an honest validator will only cast a finalize vote if their timer $T_h$ has not fired. Conversely, they will only cast a vote for the dummy block if their timer $T_h$ has fired.

Implementation-wise, this logic mandates strict state enforcement. The consensus module must maintain a boolean flag `has_timed_out` for the current view.
*   If `has_timed_out == true`: The node is logically barred from signing a `finalize` message for this view.
*   If `has_timed_out == false`: The node is eligible to finalize, provided it sees the QC.

This check must be atomic. We cannot allow a race condition where the timer fires during the processing of the finalize signature. In Rust/Tokio, this is managed by handling the timer interrupt and the QC-arrival event in the same `select!` loop within the actor, ensuring serialized processing of the state transition.

### 2.4 Piggybacking Optimization

The Simplex paper mentions a critical optimization: "Piggybacking" the finalize message onto the first vote of the next block. In the naive implementation, a node sends a vote for $b_h$, then later sends a finalize for $b_h$, then a vote for $b_{h+1}$. This triples the message load.

To optimize, we can modify the message schema. The Vote message for view $h+1$ can carry an optional field: `finalization_view: Option<u64>`.
*   When a node votes for $b_{h+1}$, it checks if it is eligible to finalize $b_h$.
*   If yes, it sets `finalization_view = Some(h)` within the Vote payload for $h+1$.
*   The leader of $h+1$, when aggregating votes, also aggregates these finalization distinct bits (or signatures).

However, rigorous implementation requires care. The Vote for $b_{h+1}$ and the Finalize for $b_h$ are logically distinct statements. If we piggyback, we must ensure that the cryptographic signature covers both intents. A cleaner approach for a "from scratch" build, to ensure clarity and modularity, is to keep the messages separate at the protocol logic level but bundle them at the network transport level if they are generated simultaneously. This avoids coupling the validity of the next vote with the validity of the previous finalization.

## 3. System Architecture and Concurrency

Building the Simplex node requires a robust system architecture that can handle high throughput while respecting the strict timing constraints of the consensus protocol. We will adopt a modular, event-driven architecture.

### 3.1 The Actor Model Implementation

We define the system as a collection of asynchronous actors. Using Rust's **Tokio** runtime, each actor is a task consuming messages from an `mpsc` (multi-producer, single-consumer) channel.

**Core Actors:**
1.  **Network Actor**: Interfaces with libp2p. It manages the swarm, handles peer discovery, and demultiplexes incoming streams. It routes `ConsensusMessage` packets to the Consensus Actor and `Transaction` packets to the Mempool Actor.
2.  **Consensus Actor**: The brain of the system. It holds the `SimplexState`. It is the only component allowed to sign votes. It manages the view timer.
3.  **Mempool Actor**: Manages a DAG of pending transactions. It performs pre-validation (signature checks, balance checks) to ensure the Consensus Actor only proposes valid blocks.
4.  **Storage Actor (Blockstore)**: Interfaces with RocksDB. It handles blocking I/O operations (disk writes) off the main consensus thread.
5.  **RPC/API Actor**: Serves external clients (wallets, explorers) via HTTP/JSON-RPC.

### 3.2 The Consensus Event Loop

The Consensus Actor's main loop is the most critical code path. It must utilize a `select!` macro to handle multiple asynchronous inputs with priority.

```rust
// Conceptual Rust Structure for the Consensus Loop
struct ConsensusActor {
    view: u64,
    timer: Pin<Box<Sleep>>,
    state: SimplexState,
    //... channels
}

impl ConsensusActor {
    async fn run(&mut self) {
        loop {
            tokio::select! {
                // 1. Handle incoming network messages (Proposals, Votes, QCs)
                Some(msg) = self.network_rx.recv() => {
                    self.handle_message(msg).await;
                }
                // 2. Handle Timer Expiration (Liveness Mechanism)
                _ = &mut self.timer => {
                    self.handle_timeout().await;
                }
                // 3. Handle Local Signals (e.g., Block Created by Self)
                Some(internal_event) = self.internal_rx.recv() => {
                    self.handle_internal(internal_event).await;
                }
            }
        }
    }
}
```

This structure ensures that the timer is a first-class event source. If the `handle_message` processing takes too long (e.g., verifying a massive block), it could delay the loop. Therefore, CPU-intensive tasks like signature verification should be offloaded to a **rayon** thread pool (for parallel CPU processing) using `tokio::task::spawn_blocking`, ensuring the main event loop remains responsive to timer events.

### 3.3 Bootstrapping and Synchronization

When a node comes online, it is likely behind the rest of the network. Simplex consensus cannot function if the node is in view $h=10$ while the network is at $h=1000$. We need a State Sync protocol.
*   **Mechanism**: Upon startup, the node enters `SyncMode`. It queries peers for their `HighestQC`.
*   **Verification**: If a peer provides a QC for view 1000, the node validates the QC signature (aggregate BLS). Because the QC is cryptographic proof that $2n/3$ validators were at view 1000, the node can safely "jump" to that view.
*   **Catch-up**: The node requests the chain of headers from its current tip to 1000 to verify the lineage and update the validator set (if the set changes). Only after syncing does the node switch to `ConsensusMode` and begin the Simplex voting loop.

## 4. Networking and P2P Layer

For the networking stack, we leverage **libp2p**, the de facto standard for decentralized networks. It provides modularity for transport, encryption, and peer discovery.

### 4.1 Transport and Discovery

*   **Transport**: We use TCP with Yamux for stream multiplexing and Noise for encrypted authentication. This ensures that all traffic between nodes is secure and efficient.
    *   Libraries: `libp2p-tcp`, `libp2p-noise`, `libp2p-yamux`.
*   **Discovery**: We employ the Kademlia DHT (Distributed Hash Table). Kademlia allows nodes to discover peers by traversing a logical distance metric (XOR metric).
    *   Library: `libp2p-kad`.
    *   **Bootnodes**: The genesis configuration must list a set of static "bootnodes" (IP/Port/PeerID) to seed the initial discovery process.

### 4.2 Gossipsub Configuration

Simplex relies on the efficient multicast of messages to all nodes ($O(n)$ complexity). We use **Gossipsub v1.1**, a pub/sub protocol designed for robust message propagation with spam protection.

**Topic Segmentation:**
To prevent low-value traffic from blocking critical consensus messages, we segregate traffic into distinct topics:
1.  `consensus/proposal`: High bandwidth. Carries full block proposals.
2.  `consensus/vote`: High frequency, small payload. Carries signatures.
3.  `consensus/qc`: Critical control messages. Carries aggregated certificates.

**Parameter Tuning:**
Gossipsub parameters must be tuned for the Simplex timing assumptions ($3\delta$).
*   `D` (Target Degree): 6. Each node maintains full connections to 6 peers for gossiping.
*   `D_low` (Min Degree): 4. If connections drop below this, aggressively find new peers.
*   `D_high` (Max Degree): 12. Prune connections above this to save bandwidth.
*   `heartbeat_interval`: 0.5s. This controls how often the mesh is rebalanced. It needs to be significantly lower than $\delta$ to ensure the mesh adapts to partitions quickly.

### 4.3 Validation and Spam Protection

A critical attack vector in P2P networks is DoS via invalid messages. We must implement an Application Layer Validation callback in Gossipsub.
Before propagating a message, the node must verify:
*   **Syntactic Validity**: Does the Protobuf decode correctly?
*   **View Relevance**: Is the message for view $h$ or $h+1$? Messages for $h-10$ are old and should be dropped. Messages for $h+100$ are future-spam and should be dropped.
*   **Signature**: Is the signature valid? (This is expensive, so we might cache results).
*   **Scoring**: Peers that forward invalid messages are penalized. If their score drops below a threshold, they are disconnected and blacklisted.

## 5. Cryptography and Identity

The Simplex paper assumes a "Bare PKI" and digital signatures. For a production system, we must select specific cryptographic primitives that support aggregation and high-speed verification.

### 5.1 BLS12-381 for Consensus Signatures

We select the **BLS12-381** curve for all consensus-related signatures.
*   **Why BLS?** The primary advantage is Signature Aggregation. In Simplex, a Notarization requires $2n/3$ votes. If we used ECDSA (like Bitcoin) or Ed25519 (like Solana), storing 2000 signatures in a block header would be prohibitively large (e.g., $2000 \times 64$ bytes = 128KB). With BLS, we can aggregate these 2000 signatures into a single 96-byte signature.
*   **Library**: `blst` (by Supranational). It is the fastest implementation currently available, written in assembly with Rust bindings.

**Aggregation Logic:**
*   Each vote contains a signature $\sigma_i$.
*   The leader (or aggregator) computes $\Sigma = \sum \sigma_i$ (point addition on the elliptic curve).
*   To verify, a node computes the aggregate public key $PK_{agg} = \sum pk_i$ based on the signer bitfield (a bitmap indicating which validators signed).
*   The verification equation is $e(g_1, \Sigma) == e(PK_{agg}, H(m))$.

### 5.2 Verifiable Random Functions (VRF) for Leader Election

Simplex requires a "Random Leader Election Oracle". We implement this using a VRF to ensure the leader schedule is unpredictable (preventing DDOS on future leaders) and unbiasable.
*   **Mechanism**: $(hash, proof) = VRF_{sk}(seed || view)$
*   The seed is updated every epoch using the signature of the previous epoch, creating a randomness beacon chain.
*   **Election Rule**: We map the VRF output to a validator index. $LeaderIndex = (hash \pmod{TotalStake})$ (if using weighted stake) or simply $(hash \pmod N)$.

**Library**: While BLS-based VRFs exist, the standard ECVRF (on Ed25519) is often faster and has standard implementations (e.g., `schnorrkel` in the Substrate ecosystem). However, using BLS keys for both signing and VRF simplifies key management. We can use the unique signature property of BLS as a VRF: The signature is the output, and it is deterministic. $H(signature)$ acts as the randomness.

### 5.3 Bare PKI Registration

The system needs a genesis block or a registration contract.
*   **Genesis**: Contains a static list of `(PublicKey, StakeWeight, NetworkAddress)`.
*   **Key Possession Proof**: When registering, a validator must sign a message proving ownership of the private key to prevent "rogue key" attacks where an attacker registers a key which is an aggregation of others.

## 6. Storage and State Management

The storage layer must persist the blockchain data and allow for efficient "Linearization" of the log. Simplex's structure, which includes dummy blocks and forks (before finalization), requires a tree-based storage schema.

### 6.1 Schema Design using RocksDB

We use **RocksDB**, a high-performance embedded key-value store. We define several Column Families (CF) to segregate data types:
1.  `CF_BLOCKS`: `BlockHash -> SerializedBlock`. Stores the raw data.
2.  `CF_HEADERS`: `BlockHash -> BlockHeader`. Used for traversing the chain without reading full payloads.
3.  `CF_CHILDREN`: `BlockHash -> Vec<BlockHash>`. An adjacency list allowing us to traverse the tree downwards (from parent to children). This is crucial for visualizing forks and identifying the canonical chain.
4.  `CF_QCS`: `View -> QC`. Indexes certificates by view number.
5.  `CF_STATE`: `AccountKey -> Balance`. Stores the current world state (Merkle Patricia Trie nodes).

### 6.2 The Tree vs. The Log

The consensus engine operates on a Block Tree.
*   When a proposal arrives, it is appended to the tree at the appropriate parent.
*   When a block is Notarized, it becomes a "stable" branch.
*   When a block is Finalized, the path from the root to that block becomes the Canonical Log.

**Pruning Strategy:**
Once block $b_h$ is finalized, all sibling blocks at height $h$ (and their descendants) are effectively orphaned. However, we cannot immediately delete them, as there might be a "re-org" or a long-range attack proof needed later. A practical approach is Epoch-based Pruning: keep history for $X$ epochs, then archive or delete non-canonical branches.

### 6.3 Linearization Implementation

The Simplex paper defines the output as $linearize(b_0, ..., b_h)$. In our implementation, this function acts as a filter iterator:

```rust
fn linearize(chain: Vec<Block>) -> Vec<Block> {
    chain.into_iter()
        .filter(|b| !b.is_dummy()) // Skip dummy blocks
        .collect()
}
```

This linearized stream is what is fed to the execution engine (EVM or WASM runtime) to process transactions. The dummy blocks effectively disappear from the application layer perspective, serving only to bridge the gap in view numbers within the consensus layer.

## 7. Scalability, Optimization, and Future Outlook

While the base Simplex protocol is efficient, real-world deployment at scale (thousands of nodes) requires further optimization.

### 7.1 Cryptographic Subsampling (Algorand Approach)

The Simplex paper notes the compatibility with Subsampling. If we have 10,000 validators, asking all of them to broadcast votes creates $10,000^2$ (or $O(n)$ with gossip) messages, which is heavy.
We can implement **Cryptographic Sortition**:
*   For each view, a validator checks if $VRF(sk, view, seed) < Threshold$.
*   Only the lucky few (e.g., committee size $k=1000$) are eligible to vote.
*   The vote includes the VRF proof.
*   This bounds the message complexity to $O(k)$ instead of $O(n)$, allowing the network to scale indefinitely while keeping bandwidth constant.

### 7.2 Pipelined Execution

To maximize throughput ($2\delta$ block time), we must decouple consensus from execution.
*   **Consensus**: Orders bytes. It cares that the hash is correct, not that the transactions are valid smart contract calls.
*   **Execution**: Runs asynchronously. When block $h$ is finalized, it is pushed to an execution queue. This prevents the slow execution of complex smart contracts from stalling the consensus timer.

### 7.3 Client APIs and Optimistic Responsiveness

Clients submitting transactions care about latency. Simplex's Optimistic Responsiveness allows us to provide a tiered confirmation status in the API:
1.  **Soft Confirm**: "The leader has proposed a block with your tx." (Latency: $1\delta$)
2.  **Notarized**: "2/3 of validators have voted." (Latency: $2\delta$)
3.  **Finalized**: "The block is irreversibly committed." (Latency: $3\delta$)

This granularity allows UI/UX developers to build responsive interfaces (showing "Processing..." at stage 1, "Success" at stage 3) that feel much faster than waiting for a probabilistic 6-block confirmation in PoW chains.

## 8. Detailed Implementation Roadmap

To guide the engineering team, we structure the build into distinct phases.

**Phase 1: The Core Library (Weeks 1-4)**
*   Implement `Block`, `Vote`, `QC` structures in Rust using `serde`.
*   Implement the `SimplexState` machine unit logic (state transitions based on inputs).
*   Mock the crypto layer (use simple hashes instead of BLS).
*   **Milestone**: A unit test where a simulated list of messages results in a finalized chain.

**Phase 2: The Networked Prototype (Weeks 5-8)**
*   Integrate `libp2p` and `tokio`.
*   Implement the Gossipsub topics.
*   Implement the $3\Delta$ timer logic.
*   **Milestone**: 5 nodes running on localhost can reach consensus and recover from a killed leader node via dummy blocks.

**Phase 3: Cryptography and Storage (Weeks 9-12)**
*   Replace mocks with `blst` and real VRF.
*   Integrate RocksDB.
*   Implement the Sync protocol.
*   **Milestone**: A persistent testnet that can survive restarts and add new nodes dynamically.

**Phase 4: Optimization and Tooling (Weeks 13-16)**
*   Implement signature aggregation.
*   Build the JSON-RPC API.
*   Develop a block explorer.
*   **Milestone**: Public Testnet Launch.

## 9. Conclusion

The construction of a blockchain based on Simplex Consensus represents a strategic engineering choice to prioritize simplicity and latency. By strictly adhering to the $3\delta$ confirmation path and the $3\Delta$ dummy block recovery mechanism, we avoid the pitfalls of complex view-change protocols. The architecture proposed here—leveraging libp2p, tokio, blst, and RocksDB—provides a solid, modern foundation. The resulting network will not only satisfy the theoretical optimality proofs of the Simplex paper but also deliver a high-performance, robust platform for decentralized applications.

This framework transforms the academic insights of Simplex into a concrete, executable engineering plan, ensuring that the "from scratch" build is grounded in proven systems principles while reaching the cutting edge of consensus research.

## 10. Technical Appendix

### 10.1 Configuration Parameters Table

| Parameter | Symbol | Recommended Value | Description |
|:---|:---|:---|:---|
| Network Delay Bound | $\Delta$ | 2.0s | The safety timeout. $T_h = 3\Delta = 6.0s$. |
| Expected Latency | $\delta$ | 200ms | Observed one-way network delay (optimistic). |
| Committee Size | $n$ | Dynamic | Total active validators. |
| Quorum Size | $Q$ | $\lfloor \frac{2n}{3} \rfloor + 1$ | Votes required for Notarization/Finalization. |
| Block Size Limit | $B_{max}$ | 2MB | Max payload size per proposal. |
| Heartbeat Interval | - | 500ms | P2P mesh maintenance tick. |

### 10.2 API Reference (Internal Consensus Traits)

```rust
/// The interface for the core Consensus Engine
#[async_trait]
trait ConsensusDriver {
    /// Called when the view timer expires
    async fn on_timeout(&mut self, view: u64) -> Result<Vote>;
    
    /// Called when a complete QC is formed
    async fn on_qc(&mut self, qc: QuorumCertificate) -> Result<()>;
    
    /// Called when a Proposal is received
    async fn on_proposal(&mut self, block: Block) -> Result<Vote>;
    
    /// Verifies the cryptographic integrity of a QC
    fn verify_qc(&self, qc: &QuorumCertificate) -> bool;
}

/// The structure of a Simplex Vote
struct Vote {
    view: u64,
    block_hash: Hash, // ZeroHash if Dummy
    signature: Signature,
    signer_id: ValidatorId,
}
```
