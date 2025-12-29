use bth_api::external::KnownTokenId;
use bth_transaction_core::{tokens, Token};

// Test that protobuf KnownTokens enum matches the tokens in bth-transaction-core
#[test]
fn test_known_tokens_enum_vs_bth_transaction_core_tokens() {
    let known_tokens = [KnownTokenId::Bth];
    for token in known_tokens.iter() {
        match token {
            KnownTokenId::Bth => {
                assert_eq!(*token as u64, *tokens::Bth::ID);
            }
        }
    }
}
