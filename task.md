# Task: Implement Sync Protocol (Issue #12)

- [x] **Design & types** <!-- id: 0 -->
    - [x] Define `SyncMessage` enum (`RequestBlock`, `ResponseBlock`) in `src/types.rs` or `src/network.rs`? <!-- id: 1 -->
    - [x] Update `ConsensusAction` to include `SendRequest(Hash)` and `SendResponse(Block)`. <!-- id: 2 -->
- [x] **State Management** <!-- id: 3 -->
    - [x] Add `orphans: HashMap<Hash, Block>` to `SimplexState`. <!-- id: 4 -->
    - [x] Implement `on_block_request(hash)` -> `Option<Block>`. <!-- id: 5 -->
    - [x] Implement `on_block_response(block)`. <!-- id: 6 -->
- [x] **Orphan Logic** <!-- id: 7 -->
    - [x] Modify `on_proposal`: If parent missing -> Buffer & Emit Request. <!-- id: 8 -->
    - [x] Modify `on_block_response`: Save block, then try process orphans. <!-- id: 9 -->
- [x] **Network Layer** <!-- id: 10 -->
    - [x] Update `NetworkEvent` decoding. <!-- id: 11 -->
    - [x] Update `Network` struct to handle sync messages (Libp2p Req/Resp or Gossip). <!-- id: 12 -->
    - [x] Update `main.rs` event loop. <!-- id: 13 -->
- [x] **Verification** <!-- id: 14 -->
    - [x] Create `tests/sync_test.rs`. <!-- id: 15 -->
    - [x] Verify node can catch up from fresh start. <!-- id: 16 -->
