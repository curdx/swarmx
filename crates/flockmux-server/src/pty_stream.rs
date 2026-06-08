//! Per-agent byte stream with a bounded resume buffer.
//!
//! Why this exists
//!   The M1 pump fed `mpsc::Receiver<Bytes>` directly into the WebSocket
//!   sender — exclusive single-consumer, with no replay window. A page
//!   refresh dropped the WS and lost every byte buffered behind it.
//!
//! What replaces it
//!   The PTY-reader task drains the mpsc into `PtyStream::append`, which
//!   tags each chunk with a monotonic `seq` and stores it in a VecDeque
//!   bounded by `MAX_BUFFER_BYTES`. Subscribers (WebSocket writers) hold
//!   their own cursor and poll `fetch_since(cursor)` whenever
//!   `wait_changed` fires. On client reconnect with `Resume{last_seq}`,
//!   the new subscriber starts its cursor at `last_seq` and gets the
//!   intervening bytes immediately — provided we still have them.
//!
//! Buffer policy
//!   - Cap by aggregate byte size, not entry count (CLI tools emit chunks
//!     of wildly varying sizes; an entry-count cap would either be too
//!     loose on big writes or too tight on small ones).
//!   - When over cap, drop the oldest entry. We never drop the *only*
//!     entry — a single 2 MiB write is allowed to exceed the cap rather
//!     than evict itself.
//!   - 1 MiB ≈ 30s of healthy idle TUI output for codex/claude; comfortably
//!     covers a page refresh, comfortably bounds memory at N agents × 1 MiB.
//!
//! Gap signalling
//!   If a returning client's `last_seq` is older than the oldest entry we
//!   still have, we cannot satisfy the resume. `fetch_since` returns
//!   `FetchResult::Gap { current_seq }` and the caller surfaces an error
//!   to the client, who restarts at the live tail.

use bytes::Bytes;
use parking_lot::Mutex;
use std::collections::VecDeque;
use tokio::sync::Notify;

/// 1 MiB per agent. Big enough to cover a page refresh on a chatty TUI,
/// small enough that N=12 agents costs ~12 MiB worst-case.
pub const MAX_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
struct Entry {
    seq: u32,
    bytes: Bytes,
}

#[derive(Debug)]
struct StreamState {
    entries: VecDeque<Entry>,
    /// Aggregate `entries[*].bytes.len()` — kept in sync with the deque so
    /// the eviction loop is O(evictions), not O(entries).
    total_bytes: usize,
    /// The seq we will assign to the NEXT append. Starts at 1 so seq=0 is
    /// reserved for "no bytes received yet".
    next_seq: u32,
    /// PTY EOF reached; no more `append` calls coming.
    closed: bool,
}

#[derive(Debug)]
pub struct PtyStream {
    state: Mutex<StreamState>,
    /// Wakes every subscriber on any state change (append, close). Using
    /// `notify_waiters` so a burst of appends only delivers one wake-up
    /// per subscriber per drain cycle.
    notify: Notify,
}

#[derive(Debug, Clone, Copy)]
pub struct StreamSnapshot {
    /// Seq that will be assigned to the next `append`. Subtract 1 for the
    /// most-recently emitted seq (or 0 if nothing has been emitted yet).
    pub next_seq: u32,
    /// `Some(seq)` if the buffer has at least one entry; that entry's seq.
    pub oldest_buffered: Option<u32>,
    pub closed: bool,
}

#[derive(Debug)]
pub enum FetchResult {
    /// `entries` is everything in the buffer with seq > `after_seq` argument.
    /// Empty if caller is already caught up.
    Ok(Vec<(u32, Bytes)>),
    /// Caller's cursor is older than anything we still have. Caller should
    /// surface an error to the client and restart at `current_seq`.
    Gap { current_seq: u32 },
}

impl Default for PtyStream {
    fn default() -> Self {
        Self::new()
    }
}

impl PtyStream {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(StreamState {
                entries: VecDeque::new(),
                total_bytes: 0,
                next_seq: 1,
                closed: false,
            }),
            notify: Notify::new(),
        }
    }

    /// Append a chunk; assign it a fresh monotonic seq. Returns the seq
    /// assigned. Evicts oldest entries while total bytes exceed
    /// `MAX_BUFFER_BYTES` AND there is more than one entry — a single
    /// oversize entry is kept (otherwise we'd lose the only data we just
    /// got).
    pub fn append(&self, bytes: Bytes) -> u32 {
        let seq = {
            let mut s = self.state.lock();
            let seq = s.next_seq;
            // Saturating wrap: at 2^32 chunks we restart at 1. Realistically
            // that's >100 years of 1 chunk/ms — not a concern, but cheap to
            // protect against.
            s.next_seq = s.next_seq.wrapping_add(1).max(1);
            s.total_bytes += bytes.len();
            s.entries.push_back(Entry { seq, bytes });
            while s.total_bytes > MAX_BUFFER_BYTES && s.entries.len() > 1 {
                let evicted = s.entries.pop_front().expect("non-empty checked above");
                s.total_bytes -= evicted.bytes.len();
            }
            seq
        };
        self.notify.notify_waiters();
        seq
    }

    /// Mark the producer side closed. Subscribers waiting in `wait_changed`
    /// wake up and can shut down once they've drained.
    pub fn close(&self) {
        {
            let mut s = self.state.lock();
            s.closed = true;
        }
        self.notify.notify_waiters();
    }

    pub fn snapshot(&self) -> StreamSnapshot {
        let s = self.state.lock();
        StreamSnapshot {
            next_seq: s.next_seq,
            oldest_buffered: s.entries.front().map(|e| e.seq),
            closed: s.closed,
        }
    }

    /// Return entries with seq > `after_seq`. If `after_seq` is older than
    /// the oldest still-buffered entry, returns `Gap`.
    ///
    /// The caller can also pass `after_seq = next_seq - 1` (or 0 for fresh)
    /// to mean "give me only future bytes." That's how a no-resume attach
    /// avoids replay.
    pub fn fetch_since(&self, after_seq: u32) -> FetchResult {
        let s = self.state.lock();
        let current_seq = s.next_seq.saturating_sub(1);
        if after_seq >= current_seq {
            // Caller is at or beyond the head — nothing to send.
            return FetchResult::Ok(Vec::new());
        }
        match s.entries.front() {
            None => {
                // Producer has never written anything yet, but caller
                // expected something to be there. Treat as caught-up.
                FetchResult::Ok(Vec::new())
            }
            Some(oldest) => {
                // If caller wanted bytes older than what we still hold,
                // signal a gap.
                if after_seq + 1 < oldest.seq {
                    return FetchResult::Gap { current_seq };
                }
                let out: Vec<(u32, Bytes)> = s
                    .entries
                    .iter()
                    .filter(|e| e.seq > after_seq)
                    .map(|e| (e.seq, e.bytes.clone()))
                    .collect();
                FetchResult::Ok(out)
            }
        }
    }

    /// Wait until either:
    ///   - the buffer gains an entry with seq > `after_seq`, or
    ///   - the stream is closed.
    ///
    /// We register the notification BEFORE re-checking state so that any
    /// append that fires `notify_waiters` between our check and our await
    /// still wakes us up.
    pub async fn wait_changed(&self, after_seq: u32) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            {
                let s = self.state.lock();
                let head = s.next_seq.saturating_sub(1);
                if head > after_seq || s.closed {
                    return;
                }
            }
            notified.as_mut().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn append_assigns_monotonic_seq_from_one() {
        let s = PtyStream::new();
        assert_eq!(s.append(Bytes::from_static(b"a")), 1);
        assert_eq!(s.append(Bytes::from_static(b"bc")), 2);
        let snap = s.snapshot();
        assert_eq!(snap.next_seq, 3);
        assert_eq!(snap.oldest_buffered, Some(1));
    }

    #[test]
    fn fetch_since_returns_only_newer_entries() {
        let s = PtyStream::new();
        s.append(Bytes::from_static(b"a"));
        s.append(Bytes::from_static(b"b"));
        s.append(Bytes::from_static(b"c"));
        match s.fetch_since(1) {
            FetchResult::Ok(v) => {
                assert_eq!(v.len(), 2);
                assert_eq!(v[0].0, 2);
                assert_eq!(v[1].0, 3);
            }
            other => panic!("expected Ok, got {:?}", other),
        }
    }

    #[test]
    fn fetch_since_head_returns_empty() {
        let s = PtyStream::new();
        s.append(Bytes::from_static(b"a"));
        s.append(Bytes::from_static(b"b"));
        match s.fetch_since(2) {
            FetchResult::Ok(v) => assert!(v.is_empty()),
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn fetch_since_signals_gap_when_buffer_evicted() {
        let s = PtyStream::new();
        // Append a chunk larger than the cap so the next append evicts it.
        let big = Bytes::from(vec![0u8; MAX_BUFFER_BYTES]);
        s.append(big); // seq=1
        s.append(Bytes::from_static(b"tiny")); // seq=2; evicts seq=1
        let snap = s.snapshot();
        assert_eq!(snap.oldest_buffered, Some(2));
        match s.fetch_since(0) {
            FetchResult::Gap { current_seq } => assert_eq!(current_seq, 2),
            other => panic!("expected Gap, got {:?}", other),
        }
    }

    #[test]
    fn single_oversize_entry_is_kept() {
        let s = PtyStream::new();
        let huge = Bytes::from(vec![0u8; MAX_BUFFER_BYTES * 2]);
        s.append(huge);
        let snap = s.snapshot();
        // Cap is exceeded but the only entry must survive — losing it
        // would mean dropping the only data we ever had.
        assert_eq!(snap.oldest_buffered, Some(1));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wait_changed_wakes_on_append() {
        let s = Arc::new(PtyStream::new());
        let s2 = s.clone();
        let h = tokio::spawn(async move {
            s2.wait_changed(0).await;
            s2.snapshot().next_seq
        });
        // Yield to let the waiter register.
        tokio::task::yield_now().await;
        s.append(Bytes::from_static(b"x"));
        let next = h.await.unwrap();
        assert_eq!(next, 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wait_changed_wakes_on_close() {
        let s = Arc::new(PtyStream::new());
        let s2 = s.clone();
        let h = tokio::spawn(async move {
            s2.wait_changed(0).await;
            s2.snapshot().closed
        });
        tokio::task::yield_now().await;
        s.close();
        assert!(h.await.unwrap());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wait_changed_returns_immediately_if_already_ahead() {
        let s = PtyStream::new();
        s.append(Bytes::from_static(b"x")); // head=1
                                            // Caller at 0 is behind head=1; should return immediately.
        tokio::time::timeout(std::time::Duration::from_millis(50), s.wait_changed(0))
            .await
            .expect("wait_changed should not block when already ahead");
    }
}
