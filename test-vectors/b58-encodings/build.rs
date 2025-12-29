use bth_account_keys::{AccountKey, RootIdentity};
use bth_api::printable::{printable_wrapper, PrintableWrapper};
use bth_test_vectors_definitions::b58_encodings::*;
use bth_util_test_vector::write_jsonl;

fn main() {
    write_jsonl("../vectors", || {
        (0..10)
            .map(|n| {
                let account_key = AccountKey::from(&RootIdentity::from(&[n; 32]));
                let public_address = account_key.default_subaddress();
                let wrapper = PrintableWrapper {
                    wrapper: Some(printable_wrapper::Wrapper::PublicAddress(
                        (&public_address).into(),
                    )),
                };
                let b58_encoded = wrapper.b58_encode().unwrap();
                B58EncodePublicAddressWithoutFog {
                    view_public_key: public_address.view_public_key().to_bytes(),
                    spend_public_key: public_address.spend_public_key().to_bytes(),
                    b58_encoded,
                }
            })
            .collect::<Vec<_>>()
    })
    .expect("Unable to write test vectors");

    // Note: B58EncodePublicAddressWithFog test vectors removed - fog support removed
}
