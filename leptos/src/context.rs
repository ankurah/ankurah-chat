//! The handshake between a host application and these components.
//!
//! What it is for: everything a chat surface needs that only the surrounding
//! application can answer. Which ankurah context to read and write through.
//! Who the reader is, if anyone. What to do when someone who is not signed in
//! reaches for the send button. Whether the connection is up. And, for the
//! places where a deployment's own collections show through — a public
//! moderation log, a link-preview cache, a member profile panel — what to
//! render or run there.
//!
//! A host builds one of these, calls [`ChatContextBuilder::provide`] above the
//! components it mounts, and is done. Nothing here is required beyond the
//! session signal itself: every hook is optional, and a surface with none of
//! them set is a working read-and-write chat that simply offers fewer doors
//! out to the rest of the application.
//!
//! # Signing in without remounting
//!
//! The session is the HOST'S OWN SIGNAL, handed in at mount and only ever read
//! from here. A host may open on an anonymous context — the components render,
//! the timeline fills, the composer refuses to send and calls the host's
//! auth-demand callback instead — and then, once the reader signs in, set that
//! signal to the authenticated context and the reader's id:
//!
//! ```ignore
//! let session = RwSignal::new((anonymous_context, None));
//! ChatContext::new(session).on_auth_demand(|| start_sign_in()).provide();
//! // …later, when sign-in returns:
//! session.set((authenticated_context, Some(user_id)));
//! ```
//!
//! That set is the whole of it, and it is the only write path: nothing in this
//! crate writes that signal. Both halves move together because [`Session`] is
//! one value, so there is no instant at which the context has changed and the
//! reader has not.
//!
//! Nothing unmounts, so the composer's draft, the armed reply, the selected
//! room, the open conversation, the message being edited and any popover the
//! host is showing all stay exactly as they were. Everything scoped to a
//! session — the members list, the rooms list, the DM thread set, both
//! read-cursor managers — belongs to this handshake, not to the host, and is
//! rebuilt here the moment the generation moves. There is nothing for a host
//! to co-swap and therefore no window in which half the surfaces are reading
//! through one session and half through another.
//!
//! The rebuild is one tick behind the set, because it is driven by an effect.
//! Within that tick a write that BEGAN under the departed session may still
//! complete through it — as itself, against its own session's rows. After the
//! tick, nothing of the old session runs again; the `Shared` cache below
//! states the whole of what that window admits.
//!
//! The reader may CHANGE, not merely appear: signing in as somebody else is a
//! legitimate swap, and it leaves whatever the previous reader had selected in
//! the host's own signals. Every write revalidates its author against the
//! session before committing (see [`WriteSession`]), and the direct-message
//! paths additionally refuse a conversation whose two ends have become the
//! same person.
//!
//! WHAT DOES NOT SURVIVE, stated plainly: the timeline's loaded window. A
//! `ScrollManager` takes its context as a constructor argument and
//! ankurah-virtual-scroll 0.9.0 offers no way to re-point one, so the pane
//! builds a fresh manager and the reader lands at the live tail rather than
//! wherever they had paged back to. Closing that needs the scroller to accept
//! a new context, or an anchor-restoring jump API.
//!
//! A host that would RATHER have teardown-and-rebuild semantics — everything
//! discarded, including the draft — can have them without asking anything of
//! this crate: key the subtree on [`ChatContext::generation`] and Leptos will
//! unmount and remount it on every swap. Neither model is privileged; the
//! difference is one `key=` in the host's view.
//!
//! # Reaching the handshake
//!
//! [`chat`] resolves through Leptos context, which walks the reactive owner
//! chain — so it answers inside a component body, a `Memo`, an `Effect` and an
//! event handler (tachys captures the owner when it attaches a listener and
//! re-enters it on invocation), and NOT inside a `spawn_local` future (whose
//! first poll is a microtask, by which time the body has returned) or an
//! ankurah subscription callback (which never had an owner at all). Take the
//! handle once in the body and move a clone into whatever runs later — that
//! way nothing depends on knowing which closure is which. Everything that writes should
//! take a [`WriteSession`] instead, which resolves the reader and the context
//! together and is the one place the auth demand is raised.

use std::cell::RefCell;
use std::sync::Arc;

use ankurah::{Context, EntityId, LiveQuery, View};
use ankurah_chat_model::{DmThreadView, MessageView, ReactionView, RoomView, UserView};
use leptos::prelude::*;
use send_wrapper::SendWrapper;

use crate::dm::DmReadStateManager;
use crate::query_registry::{self, QueryRegistration};
use crate::read_state::ReadStateManager;

/// Who the components are acting as, and the ankurah context they act
/// through: `(context, viewer)`.
///
/// `viewer` is the reader's own `User` entity id, or `None` for an anonymous
/// reader — what "is this my message", "did I react to this" and "who is
/// creating this room" are answered from.
///
/// ONE VALUE RATHER THAN TWO SIGNALS, so that a host cannot move one half
/// without the other: a single `.set()` carries both, and nothing here can
/// observe a context paired with the reader who was signed in before it.
pub type Session = (Context, Option<EntityId>);

/// A place for a host to render something of its own, given the message the
/// component is rendering.
pub type MessageSlot = Box<dyn Fn(MessageView) -> AnyView>;

/// Extra entries at the foot of a message's actions menu.
///
/// What it is for: keeping a host's own per-message tooling REACHABLE FROM THE
/// KEYBOARD. The menu is the only focusable route to anything a message row
/// offers — bubbles are not tab stops — so an affordance a host installs by
/// listening for clicks (an inspector over `data-entity-id`, say) is a
/// mouse-only affordance until it also appears here.
///
/// Given the message and a callback that closes the menu. Return an empty view
/// to add nothing. The entries a host renders should carry `role="menuitem"`
/// and the `contextMenuItem` class, so the menu's arrow-key cycling and its
/// styling pick them up.
pub type MenuActions = Box<dyn Fn(MessageView, Box<dyn Fn()>) -> AnyView>;

/// Deleting someone else's message.
///
/// The components tombstone the reader's OWN messages themselves — that is
/// plain chat — but a moderator's removal of another member's message is a
/// deployment's own affair: it may want a confirmation, a public log row
/// written in the same transaction, a different privilege check. The callback
/// receives the message and a way to close the actions menu, so it can hold
/// the menu open across a prompt and dismiss it when it is done.
///
/// Without this hook the actions menu offers Delete to authors only.
pub type ModeratorDelete = Box<dyn Fn(MessageView, Box<dyn Fn()>)>;

/// The doors out of a chat surface into the rest of a host application.
///
/// All optional. An unset hook removes the affordance that would have called
/// it rather than leaving a control that does nothing: no member-detail hook
/// means mention chips render as inert text, no moderator-delete hook means
/// the menu offers Delete to authors only.
#[derive(Default)]
pub struct ChatHooks {
    /// Rendered directly under a message bubble, above the reaction chips, and
    /// never on a tombstone. This is where a host puts what it knows about a
    /// message that the chat model does not — community renders link-preview
    /// cards from its own collection here.
    pub message_extras: Option<MessageSlot>,
    /// The body of a deleted message's tombstone row. A host that keeps a
    /// public moderation log can say who removed it; the default says only
    /// that it was removed.
    pub tombstone_body: Option<MessageSlot>,
    /// See [`ModeratorDelete`].
    pub moderator_delete: Option<ModeratorDelete>,
    /// A reader clicked a member's avatar or name in a message row. The
    /// coordinates are the trigger's bottom-left corner in viewport space,
    /// for a host that wants to anchor a popover there.
    pub member_preview: Option<Box<dyn Fn(EntityId, i32, i32)>>,
    /// A reader clicked an `@mention` chip. A host with a member profile
    /// surface opens it here.
    pub member_detail: Option<Box<dyn Fn(EntityId)>>,
    /// See [`MenuActions`].
    pub menu_actions: Option<MenuActions>,
}

/// Who is writing, and what through — resolved together, at one instant.
///
/// Two things are true of every write in these components: it needs an author,
/// and it needs a context that will still be the right one when the write
/// lands. Taking them apart invites both failures — a handler that checks the
/// reader, awaits, and then reads a context the host has swapped underneath
/// it, and a handler that resolves the context inside a future where the
/// handshake is no longer reachable at all.
///
/// The pair cannot be torn AT THE SOURCE: [`Session`] is one value, so the
/// host moves the context and the reader in a single `.set()` and there is no
/// instant at which one has moved and the other has not. What this type adds
/// is a snapshot across TIME — the host may set its signal while a write is
/// mid-flight, and that write finishes against the session it began under.
///
/// So every write path calls [`ChatContext::write_session`] BEFORE it defers,
/// and carries this into the future it spawns. No reader means no session and
/// no write: the auth demand has already been raised by the time `None` comes
/// back.
pub struct WriteSession {
    pub context: Context,
    /// The reader's own `User` entity id — the author of whatever is about to
    /// be written.
    pub viewer: EntityId,
}

/// The host handshake, as the components see it. Build one with
/// [`ChatContext::new`].
#[derive(Clone)]
pub struct ChatContext(SendWrapper<Arc<Inner>>);

/// What one session's worth of shared handles looks like once built.
///
/// These belong to the handshake rather than to any component or to the host.
/// Every surface wants the same members list; the unread badges in the rail
/// and the read cursor the log advances have to be the SAME manager, or a
/// badge clears on a round trip instead of instantly; and all of it is scoped
/// to the session, so all of it dies together when the session moves. One
/// owner, one lifetime, one rebuild.
///
/// WHEN IT DIES, EXACTLY. The host's `.set()` does not reach here; the
/// disposal effect does, and effects run a tick behind the set that woke them.
/// So there is a window of one tick, and this is what it admits: a write that
/// BEGAN under the departed session may complete through it — as itself, with
/// its own author, against its own session's rows. That is the old session
/// finishing its own bookkeeping, not this one being written by the wrong
/// hand.
///
/// After the tick nothing of it runs again. The cursor managers' `disposed`
/// flags are up, so every callback and every flush pass returns without doing
/// anything, and `discard_stale` has taken the whole struct out of the cache.
/// What is closed, and stays closed, is UNBOUNDED continuation: nothing here
/// waits on a refcount, so nothing outlives the tick.
#[derive(Default)]
struct Shared {
    /// The generation these were built against. Anything found here for an
    /// older one is taken out, DISPOSED, and dropped: the queries end when
    /// their registrations drop, and the cursor managers end because
    /// `discard_stale` calls their `dispose()` explicitly — a background task
    /// holding a strong handle would otherwise keep one alive past the swap.
    /// The weak subscription captures serve the other half: they break the
    /// cycle that would stop a manager from ever dropping at all.
    built_for: u64,
    members: Option<QuerySlot<UserView>>,
    reactions: Option<QuerySlot<ReactionView>>,
    rooms: Option<QuerySlot<RoomView>>,
    dm_threads: Option<QuerySlot<DmThreadView>>,
    room_cursors: Option<ReadStateManager>,
    dm_cursors: Option<DmReadStateManager>,
}

/// A cached query, and its registration ONCE IT HAS ONE.
///
/// The registration is separate and late because the order matters: the query
/// is published into the cache first, and only then announced to whatever
/// observer the host attached. See [`ChatContext::shared_query`].
type QuerySlot<R> = (LiveQuery<R>, Option<QueryRegistration>);

struct Inner {
    /// The host's signal, read and never written. Every session accessor here
    /// is a read of this, tracked or not.
    session: Signal<Session>,
    /// Which session this is, counting from zero.
    ///
    /// `ankurah::Context` has no equality, so "is this the same session I built
    /// against" is not a question the signal's value can answer, and this
    /// counter is the answer instead: everything session-scoped keys on it, and
    /// reading it tracked is how a component subscribes to the swap.
    ///
    /// Bumped by the swap effect ([`ChatContextBuilder::build`]) rather than by
    /// anything a host calls, which is why it moves one tick behind the host's
    /// `.set()`.
    generation: ArcRwSignal<u64>,
    /// Which rooms this deployment offers, as an AnkQL predicate. The room
    /// SET is a host choice — a page may want one room, or three — and it is
    /// declared once here rather than per component, because the rail's unread
    /// badges have to window exactly the rooms the selector lists, and the
    /// badges are shared.
    rooms_where: String,
    /// Built lazily, per session. `RefCell` because this is a browser crate on
    /// one thread, and the whole handshake already lives inside a
    /// `SendWrapper`.
    shared: RefCell<Shared>,
    online: Box<dyn Fn() -> bool>,
    can_moderate: Box<dyn Fn() -> bool>,
    demand_auth: Option<Box<dyn Fn()>>,
    hooks: ChatHooks,
}

/// Collects the handshake's parts. Only the session signal is required.
pub struct ChatContextBuilder {
    session: Signal<Session>,
    rooms_where: String,
    online: Option<Box<dyn Fn() -> bool>>,
    can_moderate: Option<Box<dyn Fn() -> bool>>,
    demand_auth: Option<Box<dyn Fn()>>,
    hooks: ChatHooks,
}

impl ChatContext {
    /// Start a handshake that reads its session from the host's own signal.
    ///
    /// The session VARIES, the host owns it, and it flows one way: this crate
    /// reads that signal and never writes it, and a host that signs a reader in
    /// mid-visit does so by setting it — see the module docs. A host whose
    /// session never changes sets its signal once and never again.
    ///
    /// Takes anything that converts, so a plain `RwSignal`, a `Memo` or a
    /// derived `Signal` all pass without ceremony.
    pub fn new(session: impl Into<Signal<Session>>) -> ChatContextBuilder {
        ChatContextBuilder {
            session: session.into(),
            rooms_where: "true".to_string(),
            online: None,
            can_moderate: None,
            demand_auth: None,
            hooks: ChatHooks::default(),
        }
    }

    /// Which session this is, counting from zero. Tracked.
    ///
    /// For components that build something against a session and must know
    /// when to rebuild it: `ankurah::Context` carries no equality, so "is this
    /// the one I built against" is answered by comparing this instead. Reading
    /// it is also how such a component subscribes to the swap.
    pub fn generation(&self) -> u64 { self.0.generation.get() }

    /// Which session this is, without subscribing.
    pub fn generation_untracked(&self) -> u64 { self.0.generation.get_untracked() }

    /// The ankurah context to read and write through. Reading this in an
    /// `Effect` or a `Memo` is what makes a query re-point when the host sets
    /// its session signal.
    pub fn context(&self) -> Context { self.0.session.get().0 }

    /// The context without subscribing — for event handlers and write paths,
    /// which want the session as of now and must not re-run on a swap.
    pub fn context_untracked(&self) -> Context { self.0.session.get_untracked().0 }

    /// The reader's own entity id, tracked.
    pub fn viewer(&self) -> Option<EntityId> { self.0.session.with(|(_, viewer)| *viewer) }

    /// The reader's own entity id, untracked.
    pub fn viewer_untracked(&self) -> Option<EntityId> { self.0.session.with_untracked(|(_, viewer)| *viewer) }

    /// Whether someone is signed in, tracked. Drives the composer's choice
    /// between sending and calling [`Self::demand_auth`].
    pub fn is_authenticated(&self) -> bool { self.viewer().is_some() }

    /// Take the handle a deferred write needs, or raise the auth demand.
    ///
    /// Call this from wherever the reader acted — a click handler, a keydown —
    /// and move the result into the future that does the work. Returning
    /// `None` means nobody is signed in; the host has been asked to fix that
    /// and the caller should simply stop.
    pub fn write_session(&self) -> Option<WriteSession> {
        let (context, viewer) = self.0.session.get_untracked();
        match viewer {
            Some(viewer) => Some(WriteSession { context, viewer }),
            None => {
                self.demand_auth();
                None
            }
        }
    }

    /// Ask the host to sign the reader in. Called when an anonymous reader
    /// reaches for an affordance that writes. A host that set no callback
    /// gets a warning in the log and nothing else — the write is refused
    /// either way.
    pub fn demand_auth(&self) {
        match &self.0.demand_auth {
            Some(demand) => demand(),
            None => tracing::warn!("a write was attempted with no reader signed in, and the host set no auth-demand callback"),
        }
    }

    /// Whether the transport is up, tracked. False disables the composer.
    pub fn online(&self) -> bool { (self.0.online)() }

    /// Whether this reader may act on other members' messages, tracked. UI
    /// gating only: the server's policy is what actually decides.
    pub fn can_moderate(&self) -> bool { (self.0.can_moderate)() }

    pub(crate) fn hooks(&self) -> &ChatHooks { &self.0.hooks }

    /// Every member, live — for author names, mention rendering and the
    /// composer's autocomplete.
    ///
    /// Built here rather than by a host and rather than per component: every
    /// surface wants the same rows, the timelines remount whenever the reader
    /// changes room, and a query per mount would mean a registration per mount.
    /// Reading this SUBSCRIBES to the session, so a caller re-runs and gets the
    /// rebuilt query when the reader signs in.
    ///
    /// The handle is a BORROW of the session's, not something to keep. Park it
    /// somewhere that outlives the session and it will go on reading through a
    /// context the reader has left. Ask again instead; it is a cache lookup.
    ///
    /// `None` only if the query could not be created, which is logged; a
    /// surface without it shows ids instead of names rather than failing.
    pub fn members(&self) -> Option<LiveQuery<UserView>> {
        self.shared_query("true", "members", |shared| &mut shared.members)
    }

    /// Every active reaction, live.
    ///
    /// One standing query rather than one per row: `Reaction` carries no room
    /// ref, so a room-scoped predicate is inexpressible, and a query per row
    /// would churn subscriptions with every virtual-scroll mount. Here rather
    /// than in the timeline because the timeline remounts on every room
    /// change, and this does not have to. A borrow, like the rest — see
    /// [`Self::members`].
    pub fn reactions(&self) -> Option<LiveQuery<ReactionView>> {
        self.shared_query("active = true", "reactions", |shared| &mut shared.reactions)
    }

    /// The rooms this deployment offers, live — the set the host declared with
    /// [`ChatContextBuilder::rooms_where`], or all of them. A borrow, like the
    /// rest — see [`Self::members`].
    pub fn rooms(&self) -> Option<LiveQuery<RoomView>> {
        let predicate = format!("{} ORDER BY name ASC", self.0.rooms_where);
        self.shared_query(&predicate, "rooms", |shared| &mut shared.rooms)
    }

    /// Whether the host narrowed the room set. A curated embed is not a place
    /// to create rooms — see [`crate::RoomSelector`].
    pub fn rooms_narrowed(&self) -> bool { self.0.rooms_where.trim() != "true" }

    /// The reader's own conversations, live.
    ///
    /// Self-shaping where a deployment scopes `dmthread` reads to its
    /// participants: a plain `deleted = false` returns exactly the reader's
    /// own. There is no client-side membership filter on purpose — one would
    /// read as though the privacy came from here, and it comes from the
    /// server's policy. A borrow, like the rest — see [`Self::members`].
    pub fn dm_threads(&self) -> Option<LiveQuery<DmThreadView>> {
        self.shared_query("deleted = false", "dm threads", |shared| &mut shared.dm_threads)
    }

    /// The reader's per-room read cursors, and the unread counts they drive.
    ///
    /// One manager for the whole handshake, deliberately: the rail draws the
    /// badge and the log advances the cursor, and two managers would mean the
    /// badge cleared on a server round trip instead of the moment the reader
    /// reached the bottom. `None` for an anonymous reader, who has no cursors.
    /// A borrow, like the rest — see [`Self::members`].
    pub fn room_cursors(&self) -> Option<ReadStateManager> {
        let mut generation = self.0.generation.get();
        loop {
            self.discard_stale(generation);
            if let Some(existing) = self.peek(generation, |shared| shared.room_cursors.clone()) {
                return Some(existing);
            }
            // Dependencies first, outside any borrow — `rooms()` takes it too.
            let rooms = self.rooms()?;
            let viewer = self.viewer_untracked()?;
            // Those two must be from the SAME session: a rooms query from
            // before a swap, paired with the reader from after it, would window
            // one deployment's rooms for another's reader. `rooms()` can end up
            // running an observer, and an observer that sets the host's session
            // signal from its callback tears exactly that pair — for one tick,
            // until the swap effect bumps the counter and this rebuilds. The
            // re-read below closes any bump that has already landed.
            let Some(next) = self.crossed(generation) else {
                let manager = ReadStateManager::try_new(self.context_untracked(), rooms, viewer)?;
                let (published, loser) = self.publish(generation, manager, |shared| &mut shared.room_cursors);
                drop(loser);
                match published {
                    Some(manager) => return Some(manager),
                    None => {
                        generation = self.0.generation.get_untracked();
                        continue;
                    }
                }
            };
            generation = next;
        }
    }

    /// The reader's per-conversation read cursors. `None` for an anonymous
    /// reader; see [`Self::room_cursors`] for why there is exactly one, and
    /// [`Self::members`] for why the handle is a borrow.
    pub fn dm_cursors(&self) -> Option<DmReadStateManager> {
        let mut generation = self.0.generation.get();
        loop {
            self.discard_stale(generation);
            if let Some(existing) = self.peek(generation, |shared| shared.dm_cursors.clone()) {
                return Some(existing);
            }
            let threads = self.dm_threads()?;
            let viewer = self.viewer_untracked()?;
            let Some(next) = self.crossed(generation) else {
                let manager = DmReadStateManager::try_new(self.context_untracked(), threads, viewer)?;
                let (published, loser) = self.publish(generation, manager, |shared| &mut shared.dm_cursors);
                drop(loser);
                match published {
                    Some(manager) => return Some(manager),
                    None => {
                        generation = self.0.generation.get_untracked();
                        continue;
                    }
                }
            };
            generation = next;
        }
    }

    /// Build, publish and register one shared query — the mechanism the four
    /// query accessors share, and the reason they are safe to call from inside
    /// each other and from inside an observer's callback.
    ///
    /// FOUR THINGS HAVE TO HOLD AT ONCE, and they are one design rather than
    /// four guards:
    ///
    /// 1. PUBLISH BEFORE NOTIFY. `query_registry::register` calls whatever
    ///    observer the host attached, and that observer may ask this handshake
    ///    for a query — including this one. So the built query goes into the
    ///    cache under a short borrow FIRST; a re-entrant call then finds it
    ///    and returns, instead of building again and recursing until the stack
    ///    is gone. The registration is attached afterwards, which is why a
    ///    slot holds `Option<QueryRegistration>`.
    /// 2. NOTHING DROPS UNDER THE BORROW. Dropping a `QueryRegistration` tells
    ///    the observers, and dropping a cursor manager can end subscriptions —
    ///    either can re-enter. So the stale state and every losing build are
    ///    taken OUT under the borrow and dropped after it is released.
    /// 3. NEVER CACHE ACROSS A GENERATION. The generation is re-read after
    ///    building and again after registering; if it moved, what was built
    ///    belongs to a session nobody is in, and the loop starts over against
    ///    the new one. Note WHEN it can move: the counter is bumped by the swap
    ///    effect, so an observer that sets the host's session signal from its
    ///    own callback does not move it inside this call — what is built here
    ///    is published, and the effect discards it a tick later. The re-reads
    ///    stay because the rule is about the cache, not about who moved the
    ///    counter: nothing may be published under a generation that is no
    ///    longer current, whenever it stopped being current.
    /// 4. THE LOOP CANNOT SPIN. Each retry re-reads the generation, which only
    ///    ever increases, so each pass observes a strictly newer session than
    ///    the last. A host that sets its session signal on every single
    ///    notification would never converge — that host is not supported, and
    ///    nothing else can produce it.
    ///
    /// THE ORDERS THE FIRST THREE FORBID, written out because nothing here is
    /// covered by a running test — the crate is `cfg(target_arch = "wasm32")`
    /// and has no harness, and every one of these needs a live ankurah node
    /// plus an observer that calls back. (Rule 4 has no order to give: it is a
    /// termination property, and what it rules out is a loop that never ends
    /// rather than an event sequence.) A reader checking this code should
    /// check it against these:
    ///
    /// 1. `members()` builds → `register` → observer calls `members()` →
    ///    cache empty → builds → `register` → … (stack exhausted). Forbidden
    ///    by the publish happening before the register: the second call finds
    ///    the query. The same shape crosses kinds — `room_cursors()` →
    ///    `rooms()` → `register` → observer calls `room_cursors()` — and is
    ///    forbidden the same way, because `rooms()` has published by then.
    /// 2. any accessor holds `borrow_mut` → replaces stale `Shared` → old
    ///    `QueryRegistration` drops → `query_unregistered` → observer calls an
    ///    accessor → `borrow_mut` panics. Forbidden by taking the stale value
    ///    out and dropping it after the borrow ends. Same for a losing build.
    /// 3. `members()` reads generation 4 → builds → `register` → the counter
    ///    reaches 5 mid-call → `members()` resumes, resets the cache to 4, and
    ///    returns a query on a context nobody is in. Forbidden by re-reading
    ///    the generation after building and after registering.
    ///    Its cursor form: `rooms()` returns generation 4's rooms, the
    ///    counter reaches 5, and `viewer_untracked()` answers generation 5 — a
    ///    manager windowing one session's rooms for another's reader.
    fn shared_query<R>(
        &self,
        predicate: &str,
        label: &'static str,
        slot: fn(&mut Shared) -> &mut Option<QuerySlot<R>>,
    ) -> Option<LiveQuery<R>>
    where
        R: View + Clone + Send + Sync + 'static,
    {
        // Tracked once: this read is what re-runs the caller on a swap.
        let mut generation = self.0.generation.get();
        loop {
            self.discard_stale(generation);
            if let Some(existing) = self.peek(generation, |shared| slot(shared).as_ref().map(|(query, _)| query.clone())) {
                return Some(existing);
            }

            let query = match self.context_untracked().query::<R>(predicate) {
                Ok(query) => query,
                Err(e) => {
                    tracing::error!("Failed to create the shared {} LiveQuery: {:?}", label, e);
                    return None;
                }
            };
            if let Some(next) = self.crossed(generation) {
                generation = next;
                continue;
            }

            // (1) Publish. A loser here means a re-entrant call got there
            // first; (2) it is dropped after the borrow is released.
            let (published, loser) = self.publish_query(generation, query, slot);
            drop(loser);
            let Some((published, ours)) = published else {
                generation = self.0.generation.get_untracked();
                continue;
            };
            if !ours {
                return Some(published);
            }

            // Now, and only now, tell the observers.
            let registration = query_registry::register(label, &published);
            let orphan = self.attach_registration(generation, registration, slot);
            drop(orphan);
            // (3) The observer may have moved the session out from under this.
            if let Some(next) = self.crossed(generation) {
                generation = next;
                continue;
            }
            return Some(published);
        }
    }

    /// Take anything built for an older session OUT under the borrow, END it,
    /// and drop it once the borrow is gone. See rule 2 on
    /// [`Self::shared_query`] for why nothing may drop under the borrow.
    ///
    /// The cursor managers are DISPOSED rather than merely dropped. Letting a
    /// refcount decide when they stop is not enough: their background tasks
    /// hold a strong reference for the length of a commit, and while one does,
    /// the manager's subscriptions are all still live and its repair path can
    /// still write through the departed session's context. `dispose` puts the
    /// flag up and drops the guards here, so a task that wakes afterwards does
    /// nothing. Both calls happen after the borrow is released, because
    /// unsubscribing runs code that must not be holding it.
    ///
    /// TWO CALLERS, ONE PATH. The swap effect calls this the tick after the
    /// host sets its session signal, which is what makes disposal eager;
    /// every accessor calls it too, which is belt — and, since the counter
    /// only ever moves inside that effect, belt that finds the work already
    /// done. Idempotent either way: whichever call arrives on a generation the
    /// cache is already built for takes nothing out and disposes nothing.
    fn discard_stale(&self, generation: u64) {
        let stale = {
            let mut shared = self.0.shared.borrow_mut();
            if shared.built_for == generation {
                None
            } else {
                Some(std::mem::replace(&mut *shared, Shared { built_for: generation, ..Shared::default() }))
            }
        };
        if let Some(stale) = &stale {
            if let Some(cursors) = &stale.room_cursors {
                cursors.dispose();
            }
            if let Some(cursors) = &stale.dm_cursors {
                cursors.dispose();
            }
        }
        drop(stale);
    }

    /// The current generation if it has moved past `generation`, else `None`.
    fn crossed(&self, generation: u64) -> Option<u64> {
        let now = self.0.generation.get_untracked();
        (now != generation).then_some(now)
    }

    /// Read one already-built handle, if the cache is on this generation.
    ///
    /// Takes the borrow mutably even though it only reads: the slot accessors
    /// are `fn(&mut Shared) -> &mut Option<_>` so that one set of pointers
    /// serves both the read and the write, and reaching a `&mut` through a
    /// shared borrow is not something to be clever about.
    fn peek<T>(&self, generation: u64, pick: impl Fn(&mut Shared) -> Option<T>) -> Option<T> {
        let mut shared = self.0.shared.borrow_mut();
        if shared.built_for != generation {
            return None;
        }
        pick(&mut shared)
    }

    /// Put a built query in the cache unless one is already there. Returns
    /// (what the caller should use, whether the caller published it) and, for
    /// the caller to drop AFTER the borrow, whatever lost.
    #[allow(clippy::type_complexity)]
    fn publish_query<R>(
        &self,
        generation: u64,
        query: LiveQuery<R>,
        slot: fn(&mut Shared) -> &mut Option<QuerySlot<R>>,
    ) -> (Option<(LiveQuery<R>, bool)>, Option<LiveQuery<R>>)
    where
        R: View + Clone + Send + Sync + 'static,
    {
        let mut shared = self.0.shared.borrow_mut();
        if shared.built_for != generation {
            return (None, Some(query));
        }
        match slot(&mut shared) {
            Some((existing, _)) => (Some((existing.clone(), false)), Some(query)),
            empty => {
                *empty = Some((query.clone(), None));
                (Some((query, true)), None)
            }
        }
    }

    /// Give a published query its registration. Returns the registration to
    /// drop after the borrow if the session moved while it was being made.
    fn attach_registration<R>(
        &self,
        generation: u64,
        registration: QueryRegistration,
        slot: fn(&mut Shared) -> &mut Option<QuerySlot<R>>,
    ) -> Option<QueryRegistration>
    where
        R: View + Clone + Send + Sync + 'static,
    {
        let mut shared = self.0.shared.borrow_mut();
        if shared.built_for != generation {
            return Some(registration);
        }
        match slot(&mut shared) {
            Some((_, held)) => {
                *held = Some(registration);
                None
            }
            None => Some(registration),
        }
    }

    /// [`Self::publish_query`] for the cursor managers, which carry no
    /// registration.
    fn publish<T: Clone>(
        &self,
        generation: u64,
        value: T,
        slot: fn(&mut Shared) -> &mut Option<T>,
    ) -> (Option<T>, Option<T>) {
        let mut shared = self.0.shared.borrow_mut();
        if shared.built_for != generation {
            return (None, Some(value));
        }
        match slot(&mut shared) {
            Some(existing) => (Some(existing.clone()), Some(value)),
            empty => {
                *empty = Some(value.clone());
                (Some(value), None)
            }
        }
    }

}

impl ChatContextBuilder {
    /// Which rooms this deployment offers, as an AnkQL predicate over `Room`.
    /// Defaults to all of them. This is the configurable room set, expressed
    /// as data rather than as a query object — a page showing one room passes
    /// `"name = 'general'"`. It scopes the room selector and the unread
    /// windows together, which is why it is declared once here.
    pub fn rooms_where(mut self, predicate: impl Into<String>) -> Self {
        self.rooms_where = predicate.into();
        self
    }

    /// Whether the transport is up. Read reactively; defaults to always up,
    /// which is right for a host that has no connection state to report.
    pub fn online(mut self, online: impl Fn() -> bool + 'static) -> Self {
        self.online = Some(Box::new(online));
        self
    }

    /// Whether this reader may act on other members' messages. Read
    /// reactively; defaults to false.
    pub fn moderator(mut self, can_moderate: impl Fn() -> bool + 'static) -> Self {
        self.can_moderate = Some(Box::new(can_moderate));
        self
    }

    /// What to do when an anonymous reader reaches for something that writes.
    /// The host owns sign-in entirely; this is the whole of what the
    /// components know about it.
    pub fn on_auth_demand(mut self, demand: impl Fn() + 'static) -> Self {
        self.demand_auth = Some(Box::new(demand));
        self
    }

    pub fn hooks(mut self, hooks: ChatHooks) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn build(self) -> ChatContext {
        let chat = ChatContext(SendWrapper::new(Arc::new(Inner {
            session: self.session,
            generation: ArcRwSignal::new(0),
            rooms_where: self.rooms_where,
            shared: RefCell::new(Shared::default()),
            online: self.online.unwrap_or_else(|| Box::new(|| true)),
            can_moderate: self.can_moderate.unwrap_or_else(|| Box::new(|| false)),
            demand_auth: self.demand_auth,
            hooks: self.hooks,
        })));

        // THE SWAP. The host's set lands here and nowhere else: the counter
        // moves, and everything the previous session built is ENDED.
        //
        // Disposal is eager rather than waiting to be noticed. A surface that
        // has since unmounted must not leave a cursor manager alive with live
        // subscriptions — and, on the conversation side, still writing cursor
        // repairs through a context the reader has left. `discard_stale` runs
        // from the accessors too, but as belt: nobody has to read anything for
        // the old session to end.
        //
        // An effect runs a tick behind the set that woke it, so eager means
        // one tick, not immediately. What that tick admits is stated on
        // `Shared` and repeated at each manager's `dispose`.
        //
        // Created under the owner that builds the handshake — the owner
        // `provide` runs in — and it lives exactly as long as that scope,
        // which is the lifetime of the cache it guards: when the scope goes,
        // the effect goes with it and nothing is left to bump a counter for a
        // tree that has been torn down.
        Effect::new({
            let chat = chat.clone();
            move |ran_before: Option<()>| {
                // The one tracked read. Everything below is untracked, so this
                // re-runs for the session and for nothing else.
                chat.0.session.track();
                if ran_before.is_none() {
                    return; // the first run is the mount, not a swap
                }
                untrack(|| {
                    chat.0.generation.update(|n| *n += 1);
                    chat.discard_stale(chat.0.generation.get_untracked());
                });
            }
        });

        chat
    }

    /// Build and install the handshake for everything mounted below this
    /// point, returning the handle for whatever else the host wants from it —
    /// [`ChatContext::generation`] to key a subtree on, or the session's own
    /// queries.
    pub fn provide(self) -> ChatContext {
        let chat = self.build();
        provide_context(chat.clone());
        chat
    }
}

/// The handshake, from inside a component body, a `Memo`, an `Effect` or an
/// event handler.
///
/// NOT from a `spawn_local` future or an ankurah subscription callback. It
/// resolves through Leptos context, which walks the reactive owner chain: a
/// future's first poll is a microtask, by which time the body has returned,
/// and a reactor callback never had an owner at all. An event handler DOES
/// carry one — tachys captures `Owner::current()` when it attaches the
/// listener and re-enters it on invocation — so a click handler can resolve
/// this; the crate hoists anyway, because a handler that goes on to defer
/// cannot, and hoisting makes the whole component owner-independent by
/// construction rather than by auditing which closure runs where.
///
/// Panics when a component is mounted without a handshake above it, which is a
/// wiring mistake rather than a runtime condition.
pub fn chat() -> ChatContext {
    use_context::<ChatContext>().expect("mount chat components below ChatContext::new(..).provide()")
}
