use ockham::consensus::SimplexState;
use ockham::crypto::generate_keypair;
use ockham::storage::RocksStorage;
use std::fs;

#[test]
fn test_rocksdb_persistence() {
    let db_path = "./target/test_db_persistence";
    let _ = fs::remove_dir_all(db_path);

    let (pk, sk) = generate_keypair();
    let committee = vec![pk.clone()];

    // 1. First run: Initialize and create Genesis
    {
        let storage = Box::new(RocksStorage::new(db_path).unwrap());
        let state = SimplexState::new(pk.clone(), sk.clone(), committee.clone(), storage);

        assert_eq!(state.current_view, 1);
        assert_eq!(state.finalized_height, 0);
        // Genesis should be saved
    } // state dropped, DB closed

    // 2. Second run: Load from DB
    {
        let storage = Box::new(RocksStorage::new(db_path).unwrap());
        // Use same key/committee (irrelevant for loading state, but needed for struct)
        let state = SimplexState::new(pk.clone(), sk.clone(), committee.clone(), storage);

        // Should have loaded state
        assert_eq!(state.current_view, 1);
        assert_eq!(state.finalized_height, 0);

        // Verify we can retrieve Genesis block
        let genesis_block = state.storage.get_block(&state.preferred_block).unwrap();
        assert!(genesis_block.is_some());
    }

    let _ = fs::remove_dir_all(db_path);
}
