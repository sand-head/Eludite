//! A bounded log with monotonically increasing cursors: the console (2,000 entries per tab) and network (5,000)
//! rings. Entries get `seq` 1, 2, 3, ...; a reader passes the last `seq` it saw (`since`, 0 at first) and gets what
//! came after, the cursor to pass next time, and how many entries after its cursor the ring has overwritten.

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Ring<T> {
    cap: usize,
    items: VecDeque<T>,
    /// The `seq` of `items[0]`.
    front: u64,
    /// The `seq` the next push gets.
    next: u64,
}

/// What [`Ring::read`] answers.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<(u64, T)>,
    /// Pass as `since` next time.
    pub next: u64,
    /// Entries after `since` the ring no longer has.
    pub dropped: u64,
}

impl<T: Clone> Ring<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            items: VecDeque::new(),
            front: 1,
            next: 1,
        }
    }

    /// Append; answers the entry's `seq`.
    pub fn push(&mut self, item: T) -> u64 {
        if self.items.len() == self.cap {
            self.items.pop_front();
            self.front += 1;
        }
        self.items.push_back(item);
        let seq = self.next;
        self.next += 1;
        seq
    }

    /// The entry `seq`, while the ring still has it.
    pub fn get_mut(&mut self, seq: u64) -> Option<&mut T> {
        let ix = seq.checked_sub(self.front)?;
        self.items.get_mut(usize::try_from(ix).ok()?)
    }

    /// The entry `seq`, while the ring still has it.
    pub fn get(&self, seq: u64) -> Option<&T> {
        let ix = seq.checked_sub(self.front)?;
        self.items.get(usize::try_from(ix).ok()?)
    }

    /// The `seq` of the newest entry (0 when nothing was ever pushed).
    pub fn last_seq(&self) -> u64 {
        self.next - 1
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Entries after `since` that pass `keep`, at most `max`. `next` is the `seq` of the last entry examined, so
    /// the following read continues after it, also past entries `keep` refused.
    pub fn read(&self, since: u64, max: usize, mut keep: impl FnMut(&T) -> bool) -> Page<T> {
        let dropped = self.front.saturating_sub(since.saturating_add(1));
        let mut items = Vec::new();
        let mut next = since
            .max(self.front.saturating_sub(1))
            .min(self.last_seq().max(since));
        let start = since.saturating_add(1).max(self.front);
        for seq in start..self.next {
            if items.len() == max {
                break;
            }
            let item = &self.items[(seq - self.front) as usize];
            next = seq;
            if keep(item) {
                items.push((seq, item.clone()));
            }
        }
        Page {
            items,
            next,
            dropped,
        }
    }

    /// Entries after `since`, all of them.
    pub fn since(&self, since: u64) -> impl Iterator<Item = (u64, &T)> {
        let start = since.saturating_add(1).max(self.front);
        (start..self.next).map(move |seq| (seq, &self.items[(seq - self.front) as usize]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_and_dropped() {
        let mut r = Ring::new(3);
        assert_eq!(
            r.read(0, 10, |_| true),
            Page {
                items: vec![],
                next: 0,
                dropped: 0
            }
        );
        for i in 1..=2 {
            assert_eq!(r.push(i * 10), i);
        }
        let p = r.read(0, 10, |_| true);
        assert_eq!(p.items, vec![(1, 10), (2, 20)]);
        assert_eq!((p.next, p.dropped), (2, 0));
        // Nothing new.
        assert_eq!(
            r.read(2, 10, |_| true),
            Page {
                items: vec![],
                next: 2,
                dropped: 0
            }
        );
        for i in 3..=5 {
            r.push(i * 10);
        }
        // The ring holds 3, 4, 5: a reader at 0 lost 1 and 2; one at 2 lost nothing.
        let p = r.read(0, 10, |_| true);
        assert_eq!(p.items, vec![(3, 30), (4, 40), (5, 50)]);
        assert_eq!((p.next, p.dropped), (5, 2));
        let p = r.read(2, 10, |_| true);
        assert_eq!((p.items.len(), p.dropped), (3, 0));
        // `max` stops the page; `next` continues it.
        let p = r.read(0, 1, |_| true);
        assert_eq!((p.items.clone(), p.next), (vec![(3, 30)], 3));
        let p = r.read(p.next, 10, |_| true);
        assert_eq!(p.items, vec![(4, 40), (5, 50)]);
        // A filter skips entries but the cursor moves past them.
        let p = r.read(0, 10, |v| *v == 40);
        assert_eq!((p.items, p.next), (vec![(4, 40)], 5));
        // Updating an entry in place, while it is kept.
        *r.get_mut(4).unwrap() = 41;
        assert_eq!(r.get_mut(1), None);
        assert_eq!(r.get_mut(6), None);
        assert_eq!(
            r.since(3).map(|(s, v)| (s, *v)).collect::<Vec<_>>(),
            vec![(4, 41), (5, 50)]
        );
        // A cursor from the future answers nothing and keeps it.
        assert_eq!(
            r.read(9, 10, |_| true),
            Page {
                items: vec![],
                next: 9,
                dropped: 0
            }
        );
        assert_eq!(r.last_seq(), 5);
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn the_console_ring_keeps_two_thousand() {
        let mut r = Ring::new(2000);
        for i in 0..2500u32 {
            r.push(i);
        }
        let p = r.read(0, 5000, |_| true);
        assert_eq!(p.items.len(), 2000);
        assert_eq!(p.dropped, 500);
        assert_eq!(p.items[0].0, 501);
        assert_eq!(p.next, 2500);
    }
}
