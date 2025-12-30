#![no_main]

use libfuzzer_sys::fuzz_target;

use botho::network::{SyncRequest, SyncResponse};

// Fuzz target for network message deserialization.
//
// Network messages are received from untrusted peers and deserialized
// before any validation. Malformed messages must not cause crashes.
//
// Security rationale:
// - SyncRequest/SyncResponse are used during initial sync (high volume)
// - An attacker on the network could craft malicious messages
fuzz_target!(|data: &[u8]| {
    // Test sync protocol messages
    // These are used during initial block download
    let _ = bincode::deserialize::<SyncRequest>(data);
    let _ = bincode::deserialize::<SyncResponse>(data);

    // If SyncResponse deserializes successfully, access its data
    if let Ok(response) = bincode::deserialize::<SyncResponse>(data) {
        match response {
            SyncResponse::Status { height, tip_hash } => {
                let _ = height;
                let _ = tip_hash;
            }
            SyncResponse::Blocks { blocks, has_more } => {
                // Iterate over blocks without panicking
                for block in &blocks {
                    let _ = block.hash();
                }
                let _ = has_more;
            }
            SyncResponse::Error(msg) => {
                let _ = msg.len();
            }
        }
    }
});
