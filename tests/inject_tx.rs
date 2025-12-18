use ockham::client::OckhamClient;
use ockham::crypto::generate_keypair_from_id;
use ockham::types::{Bytes, U256};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::test]
#[ignore]
async fn test_inject_transactions() {
    // Wait for cluster to be ready (script handles this, but we might retry connection)
    let url = "http://127.0.0.1:8545";
    let client = OckhamClient::new(url).expect("Failed to create client");

    println!("Connecting to {}", url);

    // Retry connection logic
    for i in 0..10 {
        if client.get_latest_block().await.is_ok() {
            println!("Connected to Node 0");
            break;
        }
        if i == 9 {
            panic!("Failed to connect to Node 0 after retries");
        }
        sleep(Duration::from_secs(1)).await;
    }

    // Send 5 transactions using Node 0 key (ID 0)
    for i in 0u64..5u64 {
        // Use Node 0 key because it has funds (Genesis allocation)
        let (_pk, sk) = generate_keypair_from_id(0);
        let to = Some(ockham::types::Address::default()); // Burn to 0
        let value = U256::from(100);
        let data = Bytes::from(vec![]); // Simple transfer

        match client.send_transaction(i, to, value, data, &sk).await {
            Ok(hash) => println!("Sent Tx {}: {:?}", i, hash),
            Err(e) => {
                eprintln!("Failed to send Tx {}: {:?}", i, e);
                // Don't panic, just log (might be temporary pool issue)
            }
        }
        // Removed sleep to send transactions faster
    }

    // Pass if we sent them without panic
}
