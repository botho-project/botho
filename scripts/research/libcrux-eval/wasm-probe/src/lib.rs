//! Minimal no-alloc C ABI over the three operations the web wallet /
//! wasm-signer / mobile bridge need: decapsulate, encapsulate (with caller
//! randomness), and ML-DSA verify. Used only to prove the candidate builds
//! for wasm32-unknown-unknown and the mobile targets and to compare sizes.
#![cfg_attr(target_arch = "wasm32", no_std)]

/// Scratch buffer the JS test driver writes inputs into (no allocator here).
static mut BUF: [u8; 65536] = [0u8; 65536];

#[no_mangle]
pub extern "C" fn probe_buf() -> *mut u8 {
    core::ptr::addr_of_mut!(BUF) as *mut u8
}

#[cfg(all(feature = "libcrux", not(feature = "rustcrypto")))]
mod imp {
    use libcrux_ml_dsa::ml_dsa_65 as dsa;
    use libcrux_ml_kem::mlkem768 as kem;

    pub fn decaps(sk: &[u8; 2400], ct: &[u8; 1088]) -> [u8; 32] {
        kem::decapsulate(
            &kem::MlKem768PrivateKey::from(sk),
            &kem::MlKem768Ciphertext::from(ct),
        )
    }
    pub fn encaps(pk: &[u8; 1184], m: [u8; 32], ct: &mut [u8; 1088]) -> [u8; 32] {
        let (c, ss) = kem::encapsulate(&kem::MlKem768PublicKey::from(pk), m);
        ct.copy_from_slice(c.as_slice());
        ss
    }
    pub fn verify(pk: &[u8; 1952], msg: &[u8], ctx: &[u8], sig: &[u8; 3309]) -> bool {
        dsa::verify(
            &dsa::MLDSA65VerificationKey::new(*pk),
            msg,
            ctx,
            &dsa::MLDSA65Signature::new(*sig),
        )
        .is_ok()
    }
}

#[cfg(feature = "rustcrypto")]
mod imp {
    use ml_dsa::{MlDsa65, Signature, VerifyingKey};
    use ml_kem::{
        kem::Decapsulate, EncapsulateDeterministic, Encoded, EncodedSizeUser, KemCore, MlKem768,
        B32,
    };
    type Dk = <MlKem768 as KemCore>::DecapsulationKey;
    type Ek = <MlKem768 as KemCore>::EncapsulationKey;

    pub fn decaps(sk: &[u8; 2400], ct: &[u8; 1088]) -> [u8; 32] {
        let dk = Dk::from_bytes(&Encoded::<Dk>::try_from(&sk[..]).unwrap());
        let ct = ml_kem::Ciphertext::<MlKem768>::try_from(&ct[..]).unwrap();
        dk.decapsulate(&ct).unwrap().as_slice().try_into().unwrap()
    }
    pub fn encaps(pk: &[u8; 1184], m: [u8; 32], ct: &mut [u8; 1088]) -> [u8; 32] {
        let ek = Ek::from_bytes(&Encoded::<Ek>::try_from(&pk[..]).unwrap());
        let (c, ss) = ek
            .encapsulate_deterministic(&B32::try_from(&m[..]).unwrap())
            .unwrap();
        ct.copy_from_slice(c.as_slice());
        ss.as_slice().try_into().unwrap()
    }
    pub fn verify(pk: &[u8; 1952], msg: &[u8], ctx: &[u8], sig: &[u8; 3309]) -> bool {
        let vk = VerifyingKey::<MlDsa65>::decode(&hybrid_array::Array::try_from(&pk[..]).unwrap());
        match Signature::<MlDsa65>::decode(&hybrid_array::Array::try_from(&sig[..]).unwrap()) {
            Some(s) => vk.verify_with_context(msg, ctx, &s),
            None => false,
        }
    }
}

/// # Safety
/// Pointers must reference buffers of the documented sizes.
#[no_mangle]
pub unsafe extern "C" fn probe_decaps(sk: *const u8, ct: *const u8, ss_out: *mut u8) {
    let ss = imp::decaps(&*(sk as *const [u8; 2400]), &*(ct as *const [u8; 1088]));
    core::ptr::copy_nonoverlapping(ss.as_ptr(), ss_out, 32);
}

/// # Safety
/// Pointers must reference buffers of the documented sizes.
#[no_mangle]
pub unsafe extern "C" fn probe_encaps(
    pk: *const u8,
    m: *const u8,
    ct_out: *mut u8,
    ss_out: *mut u8,
) {
    let ss = imp::encaps(
        &*(pk as *const [u8; 1184]),
        *(m as *const [u8; 32]),
        &mut *(ct_out as *mut [u8; 1088]),
    );
    core::ptr::copy_nonoverlapping(ss.as_ptr(), ss_out, 32);
}

/// # Safety
/// Pointers must reference buffers of the documented sizes.
#[no_mangle]
pub unsafe extern "C" fn probe_verify(
    pk: *const u8,
    msg: *const u8,
    msg_len: usize,
    ctx: *const u8,
    ctx_len: usize,
    sig: *const u8,
) -> bool {
    imp::verify(
        &*(pk as *const [u8; 1952]),
        core::slice::from_raw_parts(msg, msg_len),
        core::slice::from_raw_parts(ctx, ctx_len),
        &*(sig as *const [u8; 3309]),
    )
}

#[cfg(all(not(test), target_arch = "wasm32"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
