# Ockham: Simplex Consensus Implementation

> A high-performance, partially synchronous blockchain consensus engine built in Rust.

**Ockham** is a "from scratch" implementation of the **Simplex Consensus** protocol. It is designed to prioritize simplicity and latency, achieving optimal confirmation times without the complexity of traditional View Change protocols.

## Key Features

*   **Optimal Optimistic Confirmation**: $3\delta$ (three network hops to finalize).
*   **Optimal Block Time**: $2\delta$.
*   **Simplex Liveness**: Uses a unique "Dummy Block" mechanism to recover from leader failures without a dedicated view-change phase.
*   **Modular Architecture**: Built on the Actor Model using `tokio` for concurrency and `libp2p` for networking.

## Architecture

The project is structured into modular components:

*   **`consensus`**: The core State Machine. Handles proposals, vote aggregation, and the $3\Delta$ timeout logic.
*   **`types`**: Core data structures including `Block`, `Vote`, and `QuorumCertificate` (QC).
*   **`crypto`**: Abstracted cryptography layer (currently mocked for Phase 1, targeting BLS12-381).
*   **`network`**: `libp2p` implementation using Gossipsub for broadcasting Votes/Blocks and mDNS for peer discovery.

## Getting Started

### Prerequisites

*   [Rust Toolchain](https://rustup.rs/) (stable)

### Building

```bash
cargo build --release
```

### Running the Cluster (Phase 2)

We provide a script to spin up a local 4-node cluster for demonstration:

```bash
./scripts/test_cluster.sh
```

This will:
1.  Start a "Bootnode" (Node 0) on port 9000.
2.  Start 3 other nodes that dial Node 0.
3.  Wait for the cluster to form and run consensus.
4.  Print a summary of QCs formed and Blocks finalized.

### Running Tests

Run the simulation tests to verify the consensus logic:

```bash
cargo test
```

## Roadmap

This project is being developed in 4 phases:

- [x] **Phase 1: The Core Library**
    - Core data structures (`Block`, `Vote`, `QC`).
    - Consensus State Machine (`SimplexState`).
    - Mock Cryptography.
    - Simulation Tests.

- [x] **Phase 2: The Networked Prototype**
    - `libp2p` integration.
    - Gossipsub configuration.
    - Network-based consensus tests.

- [ ] **Phase 3: Cryptography and Storage**
    - [x] Replace mock crypto with `blst` (BLS12-381).
    - [ ] `RocksDB` integration for persistence.
    - [ ] Sync protocol.

- [ ] **Phase 4: Optimization and Tooling**
    - Signature Aggregation.
    - JSON-RPC API.
    - Block Explorer.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
