//! #1748: a V2 snapshot is judged by the bytes its sections actually
//! decompress to, so forged headers must not be able to drive allocation.
//!
//! This binary installs a peak-tracking global allocator (hence its own test
//! file) and asserts that decoding forged sections -- huge declared row
//! counts over tiny bodies, a byte-vector length of 2^40, a mismatched zeros
//! bomb -- fails while reserving only a small, fixed amount of memory,
//! instead of preallocating from the untrusted counts.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};

use botho::ledger::{ChainState, SnapshotV2Contents, UtxoSnapshotV2};

struct PeakAlloc;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for PeakAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static GLOBAL: PeakAlloc = PeakAlloc;

/// Peak bytes allocated above the starting level while running `f`.
fn peak_during<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = CURRENT.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let out = f();
    (out, PEAK.load(Ordering::Relaxed).saturating_sub(base))
}

/// Fixed reservation bound: zstd decoder buffers plus serde's capped `Vec`
/// preallocation (1 MiB per sequence). A decoder that preallocated from the
/// forged counts below would reserve gigabytes.
const RESERVATION_BOUND: usize = 16 << 20;

fn zstd(raw: &[u8]) -> Vec<u8> {
    zstd::encode_all(raw, 3).unwrap()
}

fn base_snapshot() -> UtxoSnapshotV2 {
    let contents = SnapshotV2Contents {
        utxos: vec![],
        contexts: vec![],
        address_index: vec![],
        key_images: vec![],
        cluster_wealth: vec![],
        import_clusters: vec![],
    };
    UtxoSnapshotV2::new(
        [7; 32],
        1,
        [1; 32],
        ChainState::default(),
        0,
        b"tip",
        &contents,
    )
    .unwrap()
}

#[test]
fn forged_sections_fail_with_bounded_reservation() {
    let base = base_snapshot();
    base.contents().unwrap();

    let mut cases: Vec<(&str, UtxoSnapshotV2)> = Vec::new();

    // Header and section prefix both claim 10M UTXOs; the body is empty.
    let mut s = base.clone();
    s.utxo_count = 10_000_000;
    s.utxo_data = zstd(&10_000_000u64.to_le_bytes());
    cases.push(("10M declared UTXOs, empty body", s));

    // 50M key images (fits the ceiling at exactly 40 B each); empty body.
    let mut s = base.clone();
    s.key_image_count = 50_000_000;
    s.key_image_data = zstd(&50_000_000u64.to_le_bytes());
    cases.push(("50M declared key images, empty body", s));

    // One context whose value claims 2^40 bytes, then a short body.
    let mut raw = 1u64.to_le_bytes().to_vec();
    raw.extend_from_slice(&36u64.to_le_bytes());
    raw.extend_from_slice(&[0u8; 36]);
    raw.extend_from_slice(&(1u64 << 40).to_le_bytes());
    raw.extend_from_slice(&[0u8; 100]);
    let mut s = base.clone();
    s.context_count = 1;
    s.context_data = zstd(&raw);
    cases.push(("2^40-byte context value", s));

    // A 64 MiB zeros bomb whose row count (0) disagrees with the header.
    let mut s = base.clone();
    s.utxo_count = 1;
    s.utxo_data = zstd(&vec![0u8; 64 << 20]);
    cases.push(("mismatched zeros bomb", s));

    for (what, snapshot) in cases {
        let (result, peak) = peak_during(|| snapshot.contents());
        assert!(result.is_err(), "{what}: forged section decoded");
        assert!(
            peak < RESERVATION_BOUND,
            "{what}: reserved {peak} B (bound {RESERVATION_BOUND} B)"
        );
        let (result, peak) = peak_during(|| snapshot.verify());
        assert!(result.is_err(), "{what}: forged snapshot verified");
        assert!(
            peak < RESERVATION_BOUND,
            "{what}: verify reserved {peak} B (bound {RESERVATION_BOUND} B)"
        );
    }
}
