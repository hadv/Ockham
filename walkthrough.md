# Simplex Consensus Walkthrough

## Phase 3: Sync Protocol

### 1. Sync Logic (Orphan Buffer)
Nodes now support a **Synchronization Protocol**. If a node receives a block whose parent is unknown (an "Orphan"), it:
1.  Buffers the Orphan block.
2.  Broadcasts a `RequestBlock(parent_hash)` message.
3.  When the missing parent arrives (`ResponseBlock`), it is processed, and then any buffered children are recursively processed.

### 2. Network Integration
- **Messages**: `SyncMessage::RequestBlock(Hash)` and `SyncMessage::ResponseBlock(Block)`.
- **Transport**: Currently uses Gossipsub broadcast for both requests and responses (MVP).

### 3. Verification
- `tests/sync_test.rs`:
    - Simulates a node (Bob) starting from Genesis.
    - Bob receives Block 3 (Orphan) -> Requests Block 2.
    - Bob receives Block 2 (Orphan) -> Requests Block 1.
    - Bob receives Block 1 -> Processes B1 -> B2 -> B3 recursively.
    - Verifies Bob's view advances to 3+.

## Phase 3: RocksDB Integration (Completed)
... [Previous Content] ...
