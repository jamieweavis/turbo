//! Per-window memory, keyed by process and window number, kept small by dropping windows
//! that have closed and, past a hard cap, the least recently used.

use std::collections::HashMap;

use super::layout::Memory;

/// A window: its owner's pid and its CGWindowID (unique while the window exists).
pub(super) type WindowKey = (i32, u32);

/// Above this many windows, closed ones are pruned on the next move.
const PRUNE_ABOVE: usize = 16;
/// Never remember more than this many, however many are still open.
const MAX_WINDOWS: usize = 64;

#[derive(Default)]
pub(super) struct WindowMemory {
    windows: HashMap<WindowKey, (Memory, u64)>,
    /// Bumped on every use, for least-recently-used eviction.
    clock: u64,
}

impl WindowMemory {
    pub fn get(&self, key: WindowKey) -> Memory {
        self.windows
            .get(&key)
            .map(|(memory, _)| *memory)
            .unwrap_or_default()
    }

    /// Stores `memory` for `key` (or forgets it, for `None`). `is_open` answers whether a
    /// remembered window still exists; it's only asked once there are enough to bother.
    pub fn put(
        &mut self,
        key: WindowKey,
        memory: Option<Memory>,
        is_open: impl Fn(WindowKey) -> bool,
    ) {
        self.clock += 1;
        match memory {
            Some(memory) => {
                self.windows.insert(key, (memory, self.clock));
            }
            None => {
                self.windows.remove(&key);
            }
        }
        if self.windows.len() > PRUNE_ABOVE {
            self.windows.retain(|&k, _| k == key || is_open(k));
        }
        while self.windows.len() > MAX_WINDOWS {
            let oldest = self
                .windows
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(&k, _)| k);
            match oldest {
                Some(k) => self.windows.remove(&k),
                None => break,
            };
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.windows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::Rect;
    use super::*;

    fn memory(x: f64) -> Option<Memory> {
        Some(Memory {
            original: Some(Rect::new(x, 0.0, 100.0, 100.0)),
            last: None,
        })
    }

    #[test]
    fn stores_and_forgets() {
        let mut store = WindowMemory::default();
        store.put((1, 10), memory(5.0), |_| true);
        assert_eq!(store.get((1, 10)), memory(5.0).unwrap());
        assert_eq!(store.get((1, 11)), Memory::default());
        store.put((1, 10), None, |_| true);
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn prunes_closed_windows_once_there_are_many() {
        let mut store = WindowMemory::default();
        for id in 0..PRUNE_ABOVE as u32 {
            store.put((1, id), memory(0.0), |_| {
                panic!("not asked below the threshold")
            });
        }
        // Everything from pid 1 has gone away; the window being stored always stays.
        store.put((2, 99), memory(0.0), |(pid, _)| pid != 1);
        assert_eq!(store.len(), 1);
        assert_eq!(store.get((2, 99)), memory(0.0).unwrap());
    }

    #[test]
    fn evicts_least_recently_used_past_the_cap() {
        let mut store = WindowMemory::default();
        for id in 0..(MAX_WINDOWS as u32 + 10) {
            store.put((1, id), memory(id as f64), |_| true);
        }
        assert_eq!(store.len(), MAX_WINDOWS);
        assert_eq!(store.get((1, 0)), Memory::default());
        assert_eq!(
            store.get((1, MAX_WINDOWS as u32 + 9)),
            memory((MAX_WINDOWS + 9) as f64).unwrap()
        );
    }
}
