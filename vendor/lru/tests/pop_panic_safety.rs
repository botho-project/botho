extern crate lru;
use lru::LruCache;
use std::{
    borrow::Borrow,
    num::NonZeroUsize,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::atomic::{AtomicBool, Ordering},
};

static ARMED: AtomicBool = AtomicBool::new(false);
#[derive(PartialEq, Eq, Hash)]
struct PanicKey(u32);
impl Borrow<u32> for PanicKey {
    fn borrow(&self) -> &u32 {
        &self.0
    }
}
impl Drop for PanicKey {
    fn drop(&mut self) {
        if ARMED.swap(false, Ordering::SeqCst) {
            panic!("stored key destructor panic");
        }
    }
}

#[test]
fn pop_panicking_destructor_all_list_positions() {
    for size in 1..=4u32 {
        for removed in 1..=size {
            let mut cache = LruCache::new(NonZeroUsize::new(size as usize).unwrap());
            for key in 1..=size {
                cache.put(PanicKey(key), key * 10);
            }
            ARMED.store(true, Ordering::SeqCst);
            assert!(catch_unwind(AssertUnwindSafe(|| {
                cache.pop(&removed);
            }))
            .is_err());
            assert!(!ARMED.load(Ordering::SeqCst));
            assert_eq!(cache.len(), (size - 1) as usize);
            assert!(!cache.contains(&removed));
            let expected: Vec<_> = (1..=size)
                .rev()
                .filter(|x| *x != removed)
                .map(|x| (x, x * 10))
                .collect();
            assert_eq!(
                cache.iter().map(|(k, v)| (k.0, *v)).collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                cache
                    .iter()
                    .rev()
                    .map(|(k, v)| (k.0, *v))
                    .collect::<Vec<_>>(),
                expected.iter().rev().cloned().collect::<Vec<_>>()
            );
            // Refill past capacity to exercise eviction after the failed pop.
            for key in 5..=10 {
                cache.put(PanicKey(key), key * 10);
            }
            assert_eq!(cache.len(), size as usize);
            assert_eq!(cache.get(&10), Some(&100));
            assert_eq!(
                cache.iter().map(|(k, v)| (k.0, *v)).collect::<Vec<_>>(),
                ((11 - size)..=10)
                    .rev()
                    .map(|x| (x, x * 10))
                    .collect::<Vec<_>>()
            );
            cache.clear();
            assert_eq!(cache.len(), 0);
        }
    }
}
