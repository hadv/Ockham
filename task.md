# Slashing Refactor Task List

- [x] Refactor Liveness Slashing to target `stakes`
- [x] Refactor Equivocation Slashing (Duplicate Vote) to target `stakes` <!-- id: 1 -->
    - [x] Inspect `src/vm.rs` logic
    - [x] Modify `src/vm.rs` to deduct from `ConsensusState.stakes`
    - [x] Update `tests/slashing_test.rs` to verify stake reduction
- [x] Verify both slashing mechanisms with tests
- [x] Fix "no stake entry" warning in `test_failure.sh`
- [x] Fix flaky `test_failure.sh` logging check
- [x] Fix `try_propose` leader self-equivocation bug
- [x] Exempt Timeout (`ZeroHash`) votes from Equivocation slashing
