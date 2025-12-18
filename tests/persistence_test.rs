use ockham::consensus::SimplexState;
use ockham::crypto::generate_keypair;
use ockham::storage::RedbStorage;
use std::fs;

#[test]
fn test_redb_persistence() {
    let _ = env_logger::try_init();

    // 1. Setup temp DB path
    let db_path = "./db/test_persistence_redb.db";
    // Clean up if exists
    if std::path::Path::new(db_path).exists() {
        std::fs::remove_file(db_path).unwrap(); // Redb is a single file usually? Or directory?
        // Note: Redb creates a single file, unlike RocksDB which creates a dir.
        // If it's a dir, remove_dir_all, if file remove_file.
        // Database::create(path) takes a path.
    }
    // Also clean up just in case
    let _ = std::fs::remove_file(db_path);

    let (pk, sk) = generate_keypair();
    let committee = vec![pk.clone()];

    // 2. Start Node A (Fresh)
    {
        println!("--- Run 1: Init Genesis ---");
        let storage = std::sync::Arc::new(RedbStorage::new(db_path).unwrap());
        let tx_pool = std::sync::Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
        let state_manager = std::sync::Arc::new(std::sync::Mutex::new(
            ockham::state::StateManager::new(storage.clone(), None),
        ));
        let executor = ockham::vm::Executor::new(
            state_manager.clone(),
            ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
        );
        let state = SimplexState::new(
            pk.clone(),
            sk.clone(),
            committee.clone(),
            storage,
            tx_pool,
            executor,
            ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
        );

        assert_eq!(state.current_view, 1);
        assert_eq!(state.finalized_height, 0);
        // Genesis should be saved
    } // state dropped, DB closed

    // 3. Restart Node A (Load Persistence)
    {
        println!("--- Run 2: Restart & Load ---");
        let storage = std::sync::Arc::new(RedbStorage::new(db_path).unwrap());
        let tx_pool = std::sync::Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
        let state_manager = std::sync::Arc::new(std::sync::Mutex::new(
            ockham::state::StateManager::new(storage.clone(), None),
        ));
        let executor = ockham::vm::Executor::new(
            state_manager.clone(),
            ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
        );
        // Use same key/committee (irrelevant for loading state, but needed for struct)
        let state = SimplexState::new(
            pk.clone(),
            sk.clone(),
            committee.clone(),
            storage,
            tx_pool,
            executor,
            ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
        );

        // Should have loaded state
        assert_eq!(state.current_view, 1);
        assert_eq!(state.finalized_height, 0);

        // Verify we can retrieve Genesis block
        let genesis_block = state.storage.get_block(&state.preferred_block).unwrap();
        assert!(genesis_block.is_some());
    }

    let _ = fs::remove_dir_all(db_path);
}
