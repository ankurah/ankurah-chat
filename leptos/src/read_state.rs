//! Persistent per-room read state and unread badges (an earlier change).
//!
//! `ReadStateManager` owns two kinds of LiveQueries:
//!
//! - the reader's own `ReadState` rows (`user = ?`; a typical readstate policy
//!   scope enforces the same thing server-side, so the client predicate is
//!   belt-and-braces), collapsed into a `room id → last_read_ts` map;
//! - one `LIMIT 10` newest-messages window per room, from which a room's
//!   unread count is "messages in the window newer than `last_read_ts`,
//!   authored by someone else". Counts therefore cap at 10, which the badge
//!   renders as "10+".
//!
//! Cost model, stated plainly: ankurah 0.9.0 has no aggregate queries, so a
//! true unread *count* needs message rows on the client. This manager
//! subscribes to one LIMIT-10 window per room. For a handful of rooms that is
//! a handful of small subscriptions; a deployment with a long room list would
//! want the badge downgraded to a boolean dot rather than a count.
//!
//! Write path: `mark_read(room, ts)` is called by the room log whenever the
//! reader is looking at the bottom of a room (room switch, scroll-to-live, new
//! message while live). It no-ops unless `ts` advances the cursor, updates the local
//! map optimistically (badges clear instantly), then flushes an upsert of
//! the row. A per-room in-flight guard plus a "flushed" watermark coalesce
//! bursts into at most one trailing write, and a remembered created-row id
//! prevents duplicate rows when a create commits before the LiveQuery
//! catches up. Duplicate rows (e.g. two tabs racing their first write) stay
//! harmless: reads take the max across rows and edits converge on one row.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use ankurah::{changes::ChangeSet, EntityId, LiveQuery};
use ankurah_signals::{Get, Mut, Peek, Subscribe, SubscriptionGuard};
use ankurah_chat_model::{MessageView, ReadState, ReadStateView, RoomView};
use send_wrapper::SendWrapper;
use wasm_bindgen_futures::spawn_local;

use ankurah::Context;

use crate::queries;

#[derive(Clone)]
pub struct ReadStateManager(SendWrapper<Arc<Inner>>);

struct Inner {
    /// The ankurah context this manager reads and writes through, held rather
    /// than looked up: its subscription callbacks fire from the reactor and
    /// its flush loops from deferred futures, neither of which carries a
    /// reactive owner for the handshake to resolve through. Held for the manager's life, which
    /// is why the handshake builds a new manager per session.
    context: Context,
    /// Set by [`ReadStateManager::dispose`]. Every callback and every
    /// background task checks it before doing anything.
    disposed: AtomicBool,
    user_id: EntityId,
    /// The user's own ReadState rows, live.
    read_states: LiveQuery<ReadStateView>,
    /// room id (base64) → effective read cursor. Server rows merged with
    /// optimistic local advances (always the max of the two).
    last_read: Mut<HashMap<String, i64>>,
    /// room id → newest cursor value confirmed written to a row. `mark_read`
    /// keeps flushing while `last_read` is ahead of this watermark.
    flushed: Mutex<HashMap<String, i64>>,
    /// Rooms with an upsert currently in flight (coalesces write bursts).
    in_flight: Mutex<HashSet<String>>,
    /// room id → id of the row this client created, so a second upsert racing
    /// the LiveQuery round-trip edits that row instead of creating a twin.
    row_ids: Mutex<HashMap<String, EntityId>>,
    /// room id → unread count within the LIMIT-10 window.
    unread: Mut<HashMap<String, usize>>,
    /// Per-room newest-message windows.
    windows: Mutex<HashMap<String, RoomWindow>>,
    /// False until the user's ReadState rows have arrived once; badges render
    /// as zero before that instead of flashing "everything unread".
    ready: Mut<bool>,
    _rooms_guard: Mutex<Option<SubscriptionGuard>>,
    _read_states_guard: Mutex<Option<SubscriptionGuard>>,
}

/// The manager's own handle on itself, for a callback that must not keep it
/// alive.
///
/// WHY WEAK. `Inner` owns the subscription guards, and each guard owns the
/// callback that fires it — so a callback holding a strong `Arc<Inner>` closes
/// a cycle and the manager could never drop at all. Weak breaks the cycle.
///
/// Weak is NOT, on its own, disposal — see [`ReadStateManager::dispose`].
///
/// Upgrading fails once the last strong handle is gone, and the callback then
/// has nothing to update.
type WeakInner = Weak<Inner>;

struct RoomWindow {
    room_id: EntityId,
    query: LiveQuery<MessageView>,
    _guard: SubscriptionGuard,
}

impl ReadStateManager {
    /// Built by the handshake, once per session, from that session's context
    /// and reader — these rows belong to one reader, so a different reader
    /// means a different manager. The handshake builds the replacement and
    /// drops this one; a host neither constructs nor holds either. `None` if
    /// the cursor query cannot be created, which is logged: a rail without
    /// badges is better than a page that will not render.
    pub(crate) fn try_new(context: Context, rooms: LiveQuery<RoomView>, user_id: EntityId) -> Option<Self> {
        let selection = queries::selection("user = ?", [(&user_id).into()]).expect("static readstate selection parses");
        let read_states = match context.query::<ReadStateView>(selection) {
            Ok(query) => query,
            Err(e) => {
                tracing::error!("Failed to create the room read-cursor LiveQuery: {:?}", e);
                return None;
            }
        };

        let inner = Arc::new(Inner {
            context,
            disposed: AtomicBool::new(false),
            user_id,
            read_states: read_states.clone(),
            last_read: Mut::new(HashMap::new()),
            flushed: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashSet::new()),
            row_ids: Mutex::new(HashMap::new()),
            unread: Mut::new(HashMap::new()),
            windows: Mutex::new(HashMap::new()),
            ready: Mut::new(false),
            _rooms_guard: Mutex::new(None),
            _read_states_guard: Mutex::new(None),
        });

        // Own read-state rows → cursor map (and re-derive every badge).
        let inner_for_rs: WeakInner = Arc::downgrade(&inner);
        let rs_guard = read_states.subscribe(move |_: ChangeSet<ReadStateView>| {
            let Some(inner) = inner_for_rs.upgrade() else { return };
            if inner.disposed.load(Ordering::Relaxed) {
                return;
            }
            Self::rebuild_cursors(&inner);
            if !inner.ready.peek() {
                inner.ready.set(true);
            }
            Self::recompute_all(&inner);
        });
        *inner._read_states_guard.lock().unwrap() = Some(rs_guard);

        // One newest-messages window per room, following the rooms query.
        let inner_for_rooms: WeakInner = Arc::downgrade(&inner);
        let rooms_guard = rooms.subscribe(move |changeset: ChangeSet<RoomView>| {
            let Some(inner) = inner_for_rooms.upgrade() else { return };
            if inner.disposed.load(Ordering::Relaxed) {
                return;
            }
            for room in changeset.appeared() {
                Self::add_window(&inner, room);
            }
            for room in changeset.removed() {
                let key = room.id().to_base64();
                inner.windows.lock().unwrap().remove(&key);
                let mut unread = inner.unread.peek().clone();
                if unread.remove(&key).is_some() {
                    inner.unread.set(unread);
                }
            }
        });
        *inner._rooms_guard.lock().unwrap() = Some(rooms_guard);

        Some(Self(SendWrapper::new(inner)))
    }

    /// End this manager now, rather than whenever the last reference happens
    /// to go.
    ///
    /// WHY REFCOUNT DEATH WAS NOT ENOUGH. `Inner` owns the subscription guards
    /// and the per-room windows, so those live exactly as long as `Inner` does
    /// — and a background task holds a strong `Arc<Inner>` for its duration.
    /// The order that breaks it:
    ///
    /// 1. the reader reaches the live tail, `mark_read` spawns a flush, and
    ///    that task holds a strong reference while it awaits its commit;
    /// 2. the surface unmounts;
    /// 3. the host sets its session signal, and the handshake drops this
    ///    manager;
    /// 4. the task wakes. `Inner` is still alive because the task held it, so
    ///    every subscription is still live and the write goes through the
    ///    context of a session the reader has left.
    ///
    /// So disposal is explicit. The flag goes up, and the guards and windows
    /// are dropped HERE rather than whenever the refcount reaches zero: a task
    /// or callback that wakes afterwards sees the flag and does nothing.
    ///
    /// WHAT THIS DOES NOT CATCH, and does not try to. A flush that had already
    /// passed its last `disposed` check when the flag went up completes: it
    /// commits the write it always was, by the author who started it, against
    /// that session's own cursor rows, through the context it was built with —
    /// and its commit may resolve several ticks after the flag, so this is not
    /// "dead within a tick". It is the departed session finishing its own
    /// bookkeeping, and it stops there: the next pass of the loop reads the
    /// flag and returns. What is closed is NEW work, and a manager continuing
    /// for as long as some task happens to hold it.
    ///
    /// Called by the handshake from its discard path, outside the borrow on
    /// the cache — dropping guards runs unsubscribe code that must not be
    /// holding it. Whichever of the three arrives first raises the flag: the
    /// accessor that reads the moved session, in the same tick as the host's
    /// set; the swap effect, a tick later, for a surface that has unmounted and
    /// asks for nothing; or the teardown cleanup, if the owner goes before that
    /// effect runs.
    pub(crate) fn dispose(&self) {
        let inner = &self.0;
        inner.disposed.store(true, Ordering::Relaxed);
        let guards = (
            inner._read_states_guard.lock().unwrap_or_else(|e| e.into_inner()).take(),
            inner._rooms_guard.lock().unwrap_or_else(|e| e.into_inner()).take(),
            std::mem::take(&mut *inner.windows.lock().unwrap_or_else(|e| e.into_inner())),
        );
        drop(guards);
    }

    /// Reactive unread count for one room's badge. Zero until the user's own
    /// read-state rows have loaded (reads track both signals).
    pub fn unread_count(&self, room_id: &str) -> usize {
        if !self.0.ready.get() {
            return 0;
        }
        self.0.unread.get().get(room_id).copied().unwrap_or(0)
    }

    /// Record that the user has seen this room up to `ts` (the newest visible
    /// message timestamp). No-ops unless the cursor advances; otherwise the
    /// local map updates immediately and a row upsert is flushed in the
    /// background.
    pub fn mark_read(&self, room_id: &str, ts: i64) {
        let inner: &Arc<Inner> = &self.0;
        if inner.disposed.load(Ordering::Relaxed) {
            return;
        }
        {
            let cursors = inner.last_read.peek();
            if ts <= cursors.get(room_id).copied().unwrap_or(0) {
                return;
            }
        }
        let mut cursors = inner.last_read.peek().clone();
        cursors.insert(room_id.to_string(), ts);
        inner.last_read.set(cursors);
        Self::recompute_room(inner, room_id);

        if !inner.in_flight.lock().unwrap().insert(room_id.to_string()) {
            return; // a flush loop is already running; it will pick this up
        }
        let inner = Arc::clone(inner);
        let room_id = room_id.to_string();
        spawn_local(async move {
            Self::flush(&inner, &room_id).await;
            inner.in_flight.lock().unwrap().remove(&room_id);
        });
    }

    /// Rebuild the cursor map from the row resultset, keeping local optimistic
    /// advances (max wins) and moving the flushed watermark up to row values.
    fn rebuild_cursors(inner: &Arc<Inner>) {
        let mut cursors = inner.last_read.peek().clone();
        let mut flushed = inner.flushed.lock().unwrap();
        for row in inner.read_states.peek() {
            let (Ok(room), Ok(ts)) = (row.room(), row.last_read_ts()) else { continue };
            let key = room.id().to_base64();
            let entry = cursors.entry(key.clone()).or_insert(0);
            *entry = (*entry).max(ts);
            let watermark = flushed.entry(key).or_insert(0);
            *watermark = (*watermark).max(ts);
        }
        drop(flushed);
        inner.last_read.set(cursors);
    }

    fn add_window(inner: &Arc<Inner>, room: RoomView) {
        let key = room.id().to_base64();
        if inner.windows.lock().unwrap().contains_key(&key) {
            return;
        }
        let selection = queries::selection(
            "room = ? AND deleted = false ORDER BY timestamp DESC LIMIT 10",
            [(&room.id()).into()],
        )
        .expect("static unread window selection parses");
        let query = match inner.context.query::<MessageView>(selection) {
            Ok(q) => q,
            Err(e) => {
                tracing::error!("Failed to create unread window for room {}: {:?}", key, e);
                return;
            }
        };

        let inner_for_sub: WeakInner = Arc::downgrade(inner);
        let key_for_sub = key.clone();
        let guard = query.subscribe(move |_: ChangeSet<MessageView>| {
            let Some(inner) = inner_for_sub.upgrade() else { return };
            if inner.disposed.load(Ordering::Relaxed) {
                return;
            }
            Self::recompute_room(&inner, &key_for_sub);
        });

        inner.windows.lock().unwrap().insert(key.clone(), RoomWindow { room_id: room.id(), query, _guard: guard });
        // If the window's initial changeset fired before the map insert above,
        // that recompute found no window and skipped; run once now (idempotent).
        Self::recompute_room(inner, &key);
    }

    /// Unread for one room = messages in its window newer than the cursor and
    /// authored by someone else (your own messages are read by definition).
    fn recompute_room(inner: &Arc<Inner>, room_id: &str) {
        let Some(items) = inner.windows.lock().unwrap().get(room_id).map(|w| w.query.peek()) else { return };
        let cursor = inner.last_read.peek().get(room_id).copied().unwrap_or(0);
        let count = items
            .iter()
            .filter(|m| m.timestamp().map(|ts| ts > cursor).unwrap_or(false))
            .filter(|m| m.user().map(|u| u.id() != inner.user_id).unwrap_or(true))
            .count();

        let mut unread = inner.unread.peek().clone();
        let changed = unread.get(room_id).copied().unwrap_or(0) != count;
        if changed {
            if count == 0 {
                unread.remove(room_id);
            } else {
                unread.insert(room_id.to_string(), count);
            }
            inner.unread.set(unread);
        }
    }

    fn recompute_all(inner: &Arc<Inner>) {
        let keys: Vec<String> = inner.windows.lock().unwrap().keys().cloned().collect();
        for key in keys {
            Self::recompute_room(inner, &key);
        }
    }

    /// Keep upserting until the row watermark catches the local cursor, so a
    /// burst of `mark_read`s collapses into one trailing write.
    async fn flush(inner: &Arc<Inner>, room_id: &str) {
        loop {
            // Checked EVERY pass, not once: this loop awaits a commit between
            // passes, and the session can move while it is suspended.
            if inner.disposed.load(Ordering::Relaxed) {
                return;
            }
            let desired = inner.last_read.peek().get(room_id).copied().unwrap_or(0);
            let watermark = inner.flushed.lock().unwrap().get(room_id).copied().unwrap_or(0);
            if desired <= watermark {
                return;
            }
            match Self::upsert(inner, room_id, desired).await {
                Ok(()) => {
                    let mut flushed = inner.flushed.lock().unwrap();
                    let entry = flushed.entry(room_id.to_string()).or_insert(0);
                    *entry = (*entry).max(desired);
                }
                Err(e) => {
                    tracing::error!("Failed to persist read state for room {}: {}", room_id, e);
                    return;
                }
            }
        }
    }

    async fn upsert(inner: &Arc<Inner>, room_id: &str, ts: i64) -> Result<(), Box<dyn std::error::Error>> {
        let room_eid = match inner.windows.lock().unwrap().get(room_id) {
            Some(w) => w.room_id,
            None => EntityId::from_base64(room_id)?,
        };

        // Prefer a row from the LiveQuery, then a row this client created that
        // the LiveQuery hasn't delivered yet.
        let existing = inner
            .read_states
            .peek()
            .into_iter()
            .find(|r| r.room().map(|rf| rf.id() == room_eid).unwrap_or(false));
        let existing = match existing {
            Some(row) => Some(row),
            None => {
                let recorded = inner.row_ids.lock().unwrap().get(room_id).copied();
                match recorded {
                    Some(id) => inner.context.get::<ReadStateView>(id).await.ok(),
                    None => None,
                }
            }
        };

        let trx = inner.context.begin();
        match existing {
            Some(row) => {
                row.edit(&trx)?.last_read_ts().set(&ts)?;
            }
            None => {
                let created = trx
                    .create(&ReadState { user: inner.user_id.into(), room: room_eid.into(), last_read_ts: ts })
                    .await?;
                inner.row_ids.lock().unwrap().insert(room_id.to_string(), created.id());
            }
        }
        // Re-checked at the last moment before the write leaves: resolving
        // `existing` may have suspended in `context.get`, and `create` awaits
        // too. A disposal landing in either window must end with this
        // transaction dropped uncommitted, not written through the departed
        // session's context.
        if inner.disposed.load(Ordering::Relaxed) {
            return Ok(());
        }
        trx.commit().await?;
        Ok(())
    }
}
