# Offline Node Slashing Walkthrough

## Logic Overview
We implemented a **Leader-Based Slashing** mechanism to punish validators who fail to produce blocks when selected as leaders (causing timeouts).


### Rules
1. **Inactivity Score**: Each validator has a score (initially 0).
2. **Liveness Penalty (Missed Leader Slot)**:
   - If a consensus view times out (represented by a `Timeout QC` / ZeroHash QC in the next block), the **leader of that failed view** is penalized.
   - **Score**: Incremented by 1.
   - **Slashing**: **10 units** are deducted from the validator's **Staking Balance** (`state.stakes`).
3. **Equivocation Penalty (Double Vote)**:
   - If a validator double votes (two different conflicting Block Hashes in the same View), they are penalized.
   - **Note**: Voting for a Timeout (ZeroHash) *does not* count as equivocation, allowing validators to safely vote for a View Change even if they previously voted for a block proposal.
   - **Slashing**: **1000 units** are deducted from the validator's **Staking Balance**.
   - **Removal**: If remaining stake drops below **2000 units**, they are removed from the committee.
4. **Reward (Successful Block)**:
   - The author of a valid block has their score decremented by 1 (clamped at 0).
5. **Threshold Action (Liveness)**:
   - If `inactivity_score > 50`, the validator is removed from the committee.

## Changes

### 1. Persistent State (`storage.rs`, `consensus.rs`)
- Added `inactivity_scores: HashMap<PublicKey, u64>` to `ConsensusState`.
- Updated initialization and persistence logic to include this field.

### 2. Execution Logic (`vm.rs`)
- Modified `execute_block` to include "Process Liveness" step.
- Implemented identifying the `failed_leader` from `block.justify.view` when a Timeout QC is detected.
- Implemented the incremental slashing (targeting **Stake**) and committee removal logic.

## Verification
We created a new test `tests/liveness_test.rs` to verify the complete flow.

### Test Scenarios
1. **Timeout Detection**: Simulated a block containing a QC for a failed view.
2. **Penalty Application**: Verified the failed leader's **Stake** was reduced by 10 and score increased by 1.
3. **Reward Application**: Verified a successful leader's score decremented.
4. **Member Removal**: Simulated 50 consecutive failures and verified the validator was removed from the committee.

### Results
```
running 1 test
test test_liveness_slashing ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
```
