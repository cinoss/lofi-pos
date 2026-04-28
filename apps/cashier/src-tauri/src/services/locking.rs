use dashmap::DashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, MutexGuard};

/// Process-wide mutex map keyed by `K`. Each key gets its own `Mutex<()>`;
/// `lock(key)` returns a guard that serializes callers using the same key.
///
/// Uses `DashMap` so distinct keys don't contend. The inner `Arc<Mutex<()>>`
/// is cheap to clone (one atomic increment); the guard holds it for the
/// duration of the critical section.
pub struct KeyMutex<K: Eq + Hash + Clone> {
    map: DashMap<K, Arc<Mutex<()>>>,
}

impl<K: Eq + Hash + Clone> KeyMutex<K> {
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    /// Acquire the mutex for `key`. Blocking; if another caller holds the
    /// mutex for the same key, this blocks until released.
    ///
    /// The returned `KeyGuard` keeps the inner `Arc<Mutex<()>>` alive for the
    /// duration of the critical section. Dropping the guard releases the
    /// mutex.
    pub fn lock(&self, key: K) -> KeyGuard<'static> {
        // TODO: evict KeyMutex entries when Arc::strong_count == 1 to bound DashMap growth
        let arc = self
            .map
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let guard = arc.lock().unwrap();
        // SAFETY: we transmute the lifetime to 'static because we keep the
        // Arc alive inside the same struct as the guard. The MutexGuard
        // borrows from the Mutex owned by the Arc; as long as the Arc is
        // alive (which it is, stored in `_arc` of the same KeyGuard), the
        // borrow remains valid. Drop order in Rust drops fields top-to-bottom,
        // but for soundness here only the joint lifetime matters: both fields
        // drop within the same statement before the KeyGuard is freed.
        // No panic points exist between the transmute and KeyGuard construction;
        // the guard cannot be leaked here.
        let guard: MutexGuard<'static, ()> =
            unsafe { std::mem::transmute::<MutexGuard<'_, ()>, MutexGuard<'static, ()>>(guard) };
        KeyGuard {
            _guard: guard,
            _arc: arc,
        }
    }
}

impl<K: Eq + Hash + Clone> Default for KeyMutex<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// Drop releases the inner Mutex. The Arc keeps the Mutex alive even if
/// `KeyMutex::map` evicts the entry.
///
/// Field order matters: `_guard` is declared first so it is dropped before
/// `_arc`, ensuring the MutexGuard releases before its backing Arc.
pub struct KeyGuard<'a> {
    _guard: MutexGuard<'a, ()>,
    _arc: Arc<Mutex<()>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[test]
    fn distinct_keys_do_not_contend() {
        let km: KeyMutex<&'static str> = KeyMutex::new();
        let _a = km.lock("a");
        let _b = km.lock("b");
        // both acquired without deadlock
    }

    #[test]
    fn same_key_serializes_threads() {
        let km: Arc<KeyMutex<&'static str>> = Arc::new(KeyMutex::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];
        for _ in 0..10 {
            let km = km.clone();
            let counter = counter.clone();
            handles.push(thread::spawn(move || {
                let _g = km.lock("k");
                let v = counter.load(Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(2));
                counter.store(v + 1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }
}
