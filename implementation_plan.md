# Issue #12: Sync Protocol Implementation

## Goal
Implement a block synchronization mechanism to allow nodes to fetch missing blocks from peers. This is critical for:
1.  **Catch-up**: New nodes or nodes that were offline need to download the history.
2.  **Gap Filling**: If a node receives a block but misses its parent (e.g., due to network packet loss), it must fetch the parent before processing the child.

## Proposed Changes

### 1. Types & Messages
#### [MODIFY] [src/types.rs](file:///Users/hadv/Ockham/src/types.rs) or [src/network.rs](file:///Users/hadv/Ockham/src/network.rs)
- Define `SyncMessage`:
    ```rust
    pub enum SyncMessage {
        RequestBlock(Hash),
        ResponseBlock(Block),
    }
    ```
- Update `NetworkEvent`:
    - `SyncMessageReceived(SyncMessage, PeerId)`

### 2. Consensus Actions
#### [MODIFY] [src/consensus.rs](file:///Users/hadv/Ockham/src/consensus.rs)
- Update `ConsensusAction`:
    ```rust
    pub enum ConsensusAction {
        // ... existing
        BroadcastRequest(Hash), // Or SendRequest(PeerId, Hash)
        SendBlock(Block, PeerId), // Respond to a specific peer
    }
    ```

### 3. State Management (Orphans)
#### [MODIFY] [src/consensus.rs](file:///Users/hadv/Ockham/src/consensus.rs)
- Add `orphans: HashMap<Hash, Vec<Block>>` to `SimplexState`.
    - Key: `parent_hash` (the hash we are waiting for).
    - Value: List of blocks that list this `parent_hash` as their parent.
- **Update `on_proposal`**:
    - If `parent_hash` is not known (and not Dummy):
        - Store block in `orphans` under `parent_hash`.
        - Return `ConsensusAction::BroadcastRequest(parent_hash)`.
- **Implement `on_block_request(hash)`**:
    - Check DB/Mem for block.
    - If found, return `ConsensusAction::SendBlock`.
- **Implement `on_block_response(block)`**:
    - Call `on_proposal(block)` (recursive safety check needed or separate flow).
    - If block is accepted:
        - Check `orphans` for any blocks waiting for this `block.hash`.
        - Recursively process valid orphans.

### 4. Network Layer
#### [MODIFY] [src/network.rs](file:///Users/hadv/Ockham/src/network.rs)
- Update `Behaviour` to handle `SyncMessage`.
- We can repurpose the existing Gossipsub for requests (efficient for "who has this?").
- Responses can be Direct (using Libp2p `RequestResponse` or just Unicast).
- *MVP Approach*: Broadcast Requests via Gossip. Broadcast Responses via Gossip (simplest, though noisy) OR use `Response` if we have peer ID.

### 5. Main Loop
#### [MODIFY] [src/main.rs](file:///Users/hadv/Ockham/src/main.rs)
- Handle `NetworkEvent::SyncMessageReceived`.
- Dispatch to `state.on_block_request` or `state.on_block_response`.

## Verification Plan

### Automated Tests
- **New Test**: `tests/sync_test.rs`
    - Setup 2 nodes.
    - Node A produces 10 blocks.
    - Node B is offline (or just not receiving).
    - Connect Node B to A.
    - Node B should request Hash(0), then Hash(1), ... until caught up.
    - Assert Node B `current_view` and `preferred_block` catch up.

### Manual Verification
- Use `scripts/test_cluster.sh`.
- Kill a node, let others advance 5 views.
- Restart node.
- Watch logs for "Requesting Block..." and "Processed Orphan...".
