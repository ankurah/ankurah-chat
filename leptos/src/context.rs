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
//! the timeline fills, and the composer calls the host's auth-demand callback
//! instead of opening a draft: a reader with no viewer who PRESSES ON the
//! message box, or has it focused programmatically, or drags text onto it,
//! raises the demand there and then and is left without a caret, once per
//! gesture; their Tab skips the box rather than landing on it; the box is
//! `readonly` while they have no viewer, so nothing they type, paste, drop or
//! compose reaches the draft; and an anonymous write raises the demand too (see
//! [`ChatContext::demand_auth`]). A signed-in reader meets none of it. Then,
//! once the reader signs in, the host sets that signal to the authenticated
//! context and the reader's id:
//!
//! ```ignore
//! let session = RwSignal::new((anonymous_context, None));
//! ChatContext::new(session).on_auth_demand(|| start_sign_in()).provide();
//! // …later, when sign-in returns:
//! session.set((authenticated_context, Some(user_id)));
//! ```
//!
//! That set is the whole of it, and it is the only write path: nothing in this
//! crate writes that signal. Every read here takes [`Session`] as ONE VALUE, so
//! nothing inside this crate can pair the arriving context with the reader who
//! was signed in before it. What that cannot do is reassemble halves the host
//! moved separately — see [`ChatContext::new`], which asks a host to set the
//! pair as one value and says what goes wrong when it does not.
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
//! THREE MOMENTS, AND ONLY THE LAST TWO WAIT FOR A TICK. Validity moves WITH
//! the set: the cache is keyed to a version of the host's signal that
//! recomputes on the first read after a `.set()`, so the first accessor to
//! arrive afterwards — in the same tick, before any effect has run — throws
//! out what the departed session built and rebuilds against the arriving one.
//! The DISCARD is one tick behind when nobody asks, because the effect that
//! makes disposal eager runs a tick behind the set that woke it. The REBUILD
//! is later still: it happens when consumers re-run on the version's
//! notification and ask again.
//!
//! A write that had already passed its last check when the discard lands
//! completes as itself — its own author, its own session's context, its own
//! rows — however long its commit takes, and then stops. No new work begins.
//! The `Shared` cache below states the whole of it.
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

use std::cell::{Cell, RefCell};
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
/// ONE VALUE RATHER THAN TWO SIGNALS, so that every read here takes the pair
/// as it stands: a single `.set()` carries both, and nothing in this crate can
/// observe a context paired with the reader who was signed in before it. The
/// atomicity is over THIS crate's reads. A host that assembles the pair from
/// two signals of its own, or moves one half in place and the other after,
/// tears it before it ever arrives — see [`ChatContext::new`].
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
/// The pair is taken IN ONE READ: [`Session`] is one value, so a write
/// resolves a context and a reader that stood together — as far back as the
/// host's own signal, which is where a host that assembles them separately can
/// still tear them ([`ChatContext::new`]). What this type adds is a snapshot
/// across TIME — the host may set its signal while a write is mid-flight, and
/// that write finishes against the session it began under.
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
/// WHEN IT DIES, EXACTLY. Two different moments, and only one of them waits
/// for a tick.
///
/// STALE THE INSTANT THE SESSION MOVES. `built_for` is compared against a
/// version of the host's signal that recomputes on the first read after a
/// `.set()` (see [`Inner::generation`]), so an accessor arriving after one —
/// in the same tick, before any effect has run — finds this struct stale, takes
/// it out and builds against the session that is now current. There is no read
/// through which one session's queries can reach another session's context or
/// reader.
///
/// DISPOSED A TICK LATER, at the latest. Whoever gets there first calls
/// `discard_stale`: the accessor above, in the same tick, or the swap effect on
/// the next one, which is what makes disposal eager for a surface that has
/// unmounted and asks for nothing. A teardown of the owner the handshake was
/// built in disposes them too, synchronously, rather than letting the queued
/// effect die with the owner.
///
/// What that gap admits is a write already past its last `disposed` check: it
/// completes AS ITSELF — its own author, its own session's context, its own
/// rows — for as long as its commit takes, which may be several ticks, and then
/// stops. That is the departed session finishing its own bookkeeping, not this
/// one being written by the wrong hand. What is closed is NEW work: once the
/// discard lands, the cursor managers' `disposed` flags are up, so every
/// callback and every later flush pass returns without doing anything, and the
/// whole struct is out of the cache. Nothing here waits on a refcount, so
/// nothing continues for as long as some task happens to hold it.
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
    /// Which session this is, counting from zero: a VERSION of the signal
    /// above.
    ///
    /// `ankurah::Context` has no equality — not value equality, and not
    /// identity either, since the handle it wraps is private — so "is this the
    /// same session I built against" is not a question the signal's value can
    /// answer. This memo answers it instead: it tracks the signal and counts,
    /// everything session-scoped keys on the number, and reading it tracked is
    /// how a component subscribes to the swap.
    ///
    /// A MEMO RATHER THAN A COUNTER SOMETHING BUMPS, and that is the whole of
    /// why the cache cannot serve a mixed object. A memo marked dirty by a
    /// `.set()` recomputes AT THE NEXT READ, tracked or untracked, so the first
    /// accessor after a set sees the new number in the same tick — while a
    /// counter bumped by the swap effect still read as the departed session
    /// until that effect ran, and every build in between stamped the arriving
    /// context with the departed session's number.
    ///
    /// It moves once per GENERATION-OBSERVED change: sets with no generation
    /// read between them coalesce into a single recompute, so a host that sets
    /// B and then C gets one number, standing for C. A read of the session
    /// itself — [`ChatContext::context_untracked`], a write session — does NOT
    /// advance the version: the host can set B, hand B to a write, set C, and
    /// the version still moves once, to the number standing for C. That is the
    /// session anything rebuilds against; B was written through, never built
    /// against.
    ///
    /// Arc-flavoured rather than arena-flavoured so the NUMBER survives its
    /// owner: a CLEAN read from outside the reactive system — a query
    /// observer's callback, a background task holding a handle — answers
    /// instead of panicking on a disposed arena slot, and every read during
    /// teardown's own cleanup answers too (cleanups run before the owner's
    /// arena nodes drop). What it does not survive: a RECOMPUTE after the
    /// host's owner tree is gone, because the closure reads `session`, and the
    /// signal wrapper is arena-allocated under whatever owner was current at
    /// [`ChatContext::new`]. A set landing after that owner's disposal marks
    /// the memo dirty, and the next read would recompute through the dead
    /// slot. The crate's own paths stop first — `ended` below turns every
    /// accessor away before it reads — and a host that keeps the handle past
    /// teardown and reads the raw session accessors is reaching into its own
    /// disposed signal.
    generation: ArcMemo<u64>,
    /// TERMINAL. Raised by [`ChatContext::discard_all`] before it takes the
    /// cache, so everything that runs downstream of teardown — including the
    /// observer callbacks teardown's own unregistrations fire — finds the
    /// handshake ended rather than an empty cache it would repopulate with
    /// managers nothing will ever dispose. Checked at the top of every
    /// accessor loop and inside every cache write. Never lowered.
    ended: Cell<bool>,
    /// The version as of teardown, for [`ChatContext::generation`] to answer
    /// after `ended`: reading the memo then might recompute through the
    /// host's disposed signal, so the last live number is frozen here instead.
    final_generation: Cell<u64>,
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
    /// SET THE PAIR AS ONE VALUE. What this crate guarantees is that every read
    /// takes the context and the reader as they stand together; it cannot
    /// reassemble halves a host moved separately. A signal derived from a
    /// context signal and a viewer signal, or an
    /// `update(|s| s.0 = new_context)` with the reader set afterwards, hands
    /// the components two sessions — and the one in between is the arriving
    /// context paired with the departed reader, which is the very thing
    /// [`Session`] being one value is here to rule out.
    ///
    /// Takes anything that converts, so a plain `RwSignal` or a derived
    /// `Signal` passes without ceremony, and a host whose session never changes
    /// can hand over the bare `(context, viewer)` value: the blanket `From<T>`
    /// wraps it in a signal that never moves. (Not a `Memo`, though —
    /// `Memo::new` wants `PartialEq`, which [`Session`] does not have, and
    /// `Memo::new_with_compare` is ceremony for a pair a host already holds.)
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
    /// it is also how such a component subscribes to the swap, and a subscriber
    /// is notified a tick after the host's `.set()` — which is when a subtree
    /// keyed on this remounts.
    ///
    /// The NUMBER never lags, whichever way it is read: it versions the
    /// session signal, and a read recomputes it if the host has set since the
    /// last one. Sets with no generation read between them coalesce, so it
    /// moves once per session a generation read observed rather than once per
    /// `.set()` — a set that only ever reached a write path advances nothing.
    ///
    /// After the owner the handshake was built in is disposed, this stops
    /// following the signal and answers the version as of teardown, frozen:
    /// recomputing would read the host's own arena-allocated signal, which
    /// dies with the host's owners.
    pub fn generation(&self) -> u64 {
        if self.0.ended.get() {
            return self.0.final_generation.get();
        }
        self.0.generation.get()
    }

    /// Which session this is, without subscribing. Recomputed on this read like
    /// [`Self::generation`]: untracked means no subscription, not an older
    /// number. Frozen after teardown, like [`Self::generation`].
    pub fn generation_untracked(&self) -> u64 {
        if self.0.ended.get() {
            return self.0.final_generation.get();
        }
        self.0.generation.get_untracked()
    }

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
    /// between opening a draft and calling [`Self::demand_auth`] — when the
    /// reader reaches for the box, and again at send. TRACKED is what makes
    /// signing in mid-visit enough on its own: the composer re-reads this from
    /// an attribute closure and from its own listeners, so a reader who signs
    /// in gets a live message box without anything remounting.
    ///
    /// FALSE AFTER TEARDOWN, like every other question this handshake answers:
    /// an ended handshake has nobody signed in. It does not fall through to the
    /// host's signal the way [`Self::viewer`] deliberately does, because this
    /// one is read from DOM listeners rather than from the host's own code, and
    /// refusing is the rule [`Self::write_session`] already follows. It travels
    /// with `can_demand_auth`, which is false after teardown too — so the
    /// composer's gate, "no viewer AND a ceremony to offer", reads false on an
    /// ended handshake instead of treating the departed reader as a guest to
    /// demand at.
    pub fn is_authenticated(&self) -> bool {
        if self.0.ended.get() {
            return false;
        }
        self.viewer().is_some()
    }

    /// Take the handle a deferred write needs, or raise the auth demand.
    ///
    /// Call this from wherever the reader acted — a click handler, a keydown —
    /// and move the result into the future that does the work. Returning
    /// `None` means nobody is signed in; the host has been asked to fix that
    /// and the caller should simply stop.
    ///
    /// THE COMPOSER'S OWN DEMAND DOES NOT REPLACE THIS. Most writes never go
    /// near the message box at all — a reaction, an author's own delete,
    /// opening a conversation, creating a room — and a host that builds its own
    /// send affordance over this handshake has no composer in the path either.
    /// Every write asks here, whatever was raised earlier.
    pub fn write_session(&self) -> Option<WriteSession> {
        // An ended handshake refuses without demanding: the surface is gone,
        // so there is nobody to sign in — and the signal read below would
        // reach the host's disposed arena slot.
        if self.0.ended.get() {
            return None;
        }
        let (context, viewer) = self.0.session.get_untracked();
        match viewer {
            Some(viewer) => Some(WriteSession { context, viewer }),
            None => {
                self.demand_auth();
                None
            }
        }
    }

    /// Ask the host to sign the reader in.
    ///
    /// TWO MOMENTS RAISE IT, and only for a reader with no viewer. REACHING FOR
    /// THE MESSAGE BOX: a pointer press on the composer, a programmatic focus,
    /// text dragged onto it, or — for a session that dropped to anonymous under
    /// a caret that had already landed — a keystroke that would have changed or
    /// sent the draft. The composer drops the focus rather than opening a
    /// caret, and holds the box `readonly` so none of those write anything.
    /// ONCE PER GESTURE, counted rather than timed: a double-click raises it
    /// once, and a ceremony that hands focus back on close raises it no further
    /// times at all. An anonymous reader's Tab skips the box entirely rather
    /// than landing on something that would blur. WRITE:
    /// [`Self::write_session`] demands for anything that would commit, which
    /// covers every affordance that never touches the composer. A signed-in
    /// reader meets neither.
    ///
    /// MAKE THE CALLBACK IDEMPOTENT. A genuinely new gesture while the ceremony
    /// is already open raises it again — that is the point of the per-gesture
    /// rule, since a reader who dismissed it and clicked again must get it back
    /// — so a callback that opens a second popup, or restarts a redirect, on
    /// being asked twice is the host's to make safe. Raising an already-open
    /// ceremony should be a no-op.
    ///
    /// A host that set no callback gets a warning in the log and nothing else.
    /// It also gets no composer behaviour: taking the caret away from a reader
    /// with no ceremony to offer instead would be a dead end, so the composer
    /// asks whether a callback exists at all before it interferes, and leaves an
    /// anonymous reader free to type into a box whose send is still refused
    /// here.
    pub fn demand_auth(&self) {
        match &self.0.demand_auth {
            Some(demand) => demand(),
            None => tracing::warn!("a write was attempted with no reader signed in, and the host set no auth-demand callback"),
        }
    }

    /// Whether this handshake will actually raise a ceremony if asked.
    ///
    /// What it is for: an affordance that would change SHAPE for an anonymous
    /// reader rather than merely refuse. Refusing needs nothing — the write
    /// simply does not happen — but the composer's demand replaces the caret
    /// with the host's ceremony, and with no ceremony installed that is a
    /// message box a reader cannot focus and is told nothing about. So the
    /// composer's gate stands down here and behaves as it did before it
    /// existed.
    ///
    /// False after teardown for the same reason [`Self::write_session`] refuses
    /// without demanding: the surface is gone, so there is nobody to sign in.
    pub(crate) fn can_demand_auth(&self) -> bool { !self.0.ended.get() && self.0.demand_auth.is_some() }

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
        if self.0.ended.get() {
            return None;
        }
        let mut generation = self.0.generation.get();
        loop {
            // Re-checked every pass: teardown can land mid-call, through an
            // observer this very loop's registrations fire.
            if self.0.ended.get() {
                return None;
            }
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
            // signal from its callback moves the session between these two
            // reads. The re-read below is what refuses the pair: the version
            // recomputes on that read, so a set landed anywhere in this call is
            // seen here, before anything is built from the halves.
            let Some(next) = self.crossed(generation) else {
                // `crossed` also answers `None` for an ENDED handshake, where
                // "unchanged" would be the wrong reading: `rooms()` above can
                // run an observer that tears the owner down synchronously, and
                // the session read below would then reach the host's dropped
                // arena slot. Ended is re-asked by name before anything raw.
                if self.0.ended.get() {
                    return None;
                }
                let manager = ReadStateManager::try_new(self.context_untracked(), rooms, viewer)?;
                let (published, loser) = self.publish(generation, manager, |shared| &mut shared.room_cursors);
                // A losing manager is DISPOSED, not merely dropped — the
                // crate's own rule for cursor managers, and construction is
                // why it applies here: building one can already have started
                // work holding a strong reference (the DM side's windows can
                // reach a repair task), and nobody will ever publish or end a
                // loser. The flag goes up before the drop, outside the borrow.
                if let Some(loser) = &loser {
                    loser.dispose();
                }
                drop(loser);
                // That dispose-and-drop is a re-entrancy point (rule 2: ending
                // a manager's subscriptions can run code), so ended is re-asked
                // before the successful return — one rule, no exceptions. The
                // published winner under ended has already been taken out and
                // disposed by the teardown; None is the honest answer.
                if self.0.ended.get() {
                    return None;
                }
                match published {
                    Some(manager) => return Some(manager),
                    None => {
                        generation = self.generation_untracked();
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
        if self.0.ended.get() {
            return None;
        }
        let mut generation = self.0.generation.get();
        loop {
            if self.0.ended.get() {
                return None;
            }
            self.discard_stale(generation);
            if let Some(existing) = self.peek(generation, |shared| shared.dm_cursors.clone()) {
                return Some(existing);
            }
            let threads = self.dm_threads()?;
            let viewer = self.viewer_untracked()?;
            let Some(next) = self.crossed(generation) else {
                // Same re-ask as `room_cursors`: `crossed`'s `None` covers the
                // ended handshake too, and the read below must not run then.
                if self.0.ended.get() {
                    return None;
                }
                let manager = DmReadStateManager::try_new(self.context_untracked(), threads, viewer)?;
                let (published, loser) = self.publish(generation, manager, |shared| &mut shared.dm_cursors);
                // Same rule as `room_cursors`: a loser is disposed before the
                // drop, because its construction may already have spawned a
                // repair task whose disposed re-check would otherwise pass
                // forever.
                if let Some(loser) = &loser {
                    loser.dispose();
                }
                drop(loser);
                // Same re-ask as `room_cursors`, same reason.
                if self.0.ended.get() {
                    return None;
                }
                match published {
                    Some(manager) => return Some(manager),
                    None => {
                        generation = self.generation_untracked();
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
    ///    the new one. Each re-read is a live question rather than a
    ///    formality: the generation is a memo over the host's signal, so a set
    ///    that lands mid-call — an observer setting the session from its own
    ///    callback is how that happens — recomputes on the re-read and is
    ///    caught inside the call that would otherwise have published against
    ///    it.
    /// 4. THE LOOP CANNOT SPIN. Each retry re-reads the generation, which only
    ///    ever increases, so each pass observes a strictly newer session than
    ///    the last. A host that sets its session signal on every single
    ///    notification would never converge — that host is not supported, and
    ///    nothing else can produce it. (Teardown is the one thing that stops
    ///    the generation moving; rule 5 is what ends the loop then.)
    /// 5. ENDED IS FINAL. Teardown raises the handshake's `ended` bit before
    ///    it empties the cache, and every accessor checks it at the top of
    ///    every pass, every cache write refuses under it, and `discard_stale`
    ///    will not reset `built_for` after it. Without this, the observer
    ///    callbacks teardown's own unregistrations fire could walk back in,
    ///    find a coherent-looking empty cache, and publish a manager that
    ///    nothing would ever dispose — alive until its refcount died, which
    ///    is exactly the outcome `discard_all` exists to rule out.
    ///
    /// THE ORDERS THE FIRST THREE FORBID, written out because nothing here is
    /// covered by a running test — the crate is `cfg(target_arch = "wasm32")`
    /// and has no harness, and every one of these needs a live ankurah node
    /// plus an observer that calls back. (The one thing that IS tested is the
    /// reactive fact rule 3 stands on, in `tests/session_version.rs`: a memo
    /// marked dirty by a `.set()` recomputes on the next read of any kind.
    /// Rule 4 has no order to give: it is a termination property, and what it
    /// rules out is a loop that never ends rather than an event sequence.) A
    /// reader checking this code should check it against these:
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
    /// 3. `members()` reads generation 4 → builds → `register` → the observer
    ///    sets the host's session mid-call → `members()` resumes, resets the
    ///    cache to 4, and returns a query on a context nobody is in. Forbidden
    ///    by re-reading the generation after building and after registering:
    ///    that read recomputes the version and answers 5.
    ///    Its cursor form: `rooms()` returns generation 4's rooms, the session
    ///    moves, and `viewer_untracked()` answers generation 5's reader — a
    ///    manager windowing one session's rooms for another's reader, whose
    ///    `mark_read` would then write a cursor row through the arriving
    ///    context. Forbidden by the same re-read, standing in `room_cursors`
    ///    between those two reads and the manager they would build.
    /// 4. the owner is disposed → `on_cleanup` → `discard_all` empties the
    ///    cache → dropping the old registrations fires `query_unregistered` →
    ///    an observer holding the handshake calls an accessor → it rebuilds
    ///    into the emptied cache, and the manager it publishes outlives
    ///    teardown with its flag never raised. Forbidden by rule 5: `ended`
    ///    goes up before the cache is taken, and the observer's call answers
    ///    `None` at the top of its first pass.
    fn shared_query<R>(
        &self,
        predicate: &str,
        label: &'static str,
        slot: fn(&mut Shared) -> &mut Option<QuerySlot<R>>,
    ) -> Option<LiveQuery<R>>
    where
        R: View + Clone + Send + Sync + 'static,
    {
        if self.0.ended.get() {
            return None;
        }
        // Tracked once: this read is what re-runs the caller on a swap.
        let mut generation = self.0.generation.get();
        loop {
            // Rule 5: re-checked every pass, because teardown can land
            // mid-call through this loop's own registrations.
            if self.0.ended.get() {
                return None;
            }
            self.discard_stale(generation);
            if let Some(existing) = self.peek(generation, |shared| slot(shared).as_ref().map(|(query, _)| query.clone())) {
                return Some(existing);
            }

            // `discard_stale` can run an observer (dropped registrations tell
            // them), and an observer can tear the owner down synchronously —
            // after which `peek`'s miss reads like an ordinary cache miss and
            // the session read below would reach the host's dropped arena
            // slot. Re-ask by name before anything raw.
            if self.0.ended.get() {
                return None;
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
            // That drop is a re-entrancy point too — a bare query drop can
            // run an unsubscribe path that tears the owner down — and the
            // `!ours` arm below is a successful return a cursor accessor
            // follows into a raw session read. The rule is one rule: ended is
            // re-asked before every successful return, this one included.
            if self.0.ended.get() {
                return None;
            }
            let Some((published, ours)) = published else {
                generation = self.generation_untracked();
                continue;
            };
            if !ours {
                return Some(published);
            }

            // Now, and only now, tell the observers.
            let registration = query_registry::register(label, &published);
            let orphan = self.attach_registration(generation, registration, slot);
            drop(orphan);
            // The observer may have torn the owner down synchronously. The
            // query in hand was already taken out and ended by that teardown,
            // and a caller like `room_cursors` would follow a returned query
            // straight into a raw session read — so an ended handshake answers
            // `None` here, which is also the only answer the README promises
            // after teardown.
            if self.0.ended.get() {
                return None;
            }
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
    /// still write through the departed session's context. [`Self::end`] puts
    /// the flags up, so a task that wakes afterwards does nothing.
    ///
    /// THREE CALLERS, ONE PATH. Every accessor calls this before it reads the
    /// cache, and after a session moves the accessor is usually FIRST: the
    /// generation it passes recomputed on its own read, so the discard happens
    /// in the same tick as the host's set. The swap effect calls it a tick
    /// later, which is what makes disposal eager for a surface that has
    /// unmounted and asks for nothing. The teardown cleanup calls
    /// [`Self::discard_all`], which is this path with the generation test taken
    /// out. Idempotent: a call arriving on the generation the cache is already
    /// built for takes nothing out and disposes nothing.
    fn discard_stale(&self, generation: u64) {
        // After teardown the cache is already empty and [`Self::discard_all`]
        // had the last word — resetting `built_for` here would hand a
        // late accessor a coherent-looking empty cache to repopulate.
        if self.0.ended.get() {
            return;
        }
        let stale = {
            let mut shared = self.0.shared.borrow_mut();
            if shared.built_for == generation {
                None
            } else {
                Some(std::mem::replace(&mut *shared, Shared { built_for: generation, ..Shared::default() }))
            }
        };
        Self::end(stale);
    }

    /// End everything the cache holds, whatever session it was built for — the
    /// teardown path, run when the owner the handshake was built in is
    /// disposed.
    ///
    /// What it is for: the swap effect is a QUEUED effect, and an owner
    /// disposed before it next runs takes it away unrun. Nothing would then
    /// raise the managers' flags, and a flush already holding a strong handle
    /// would wake, find `disposed` still false, and commit through the departed
    /// session's context — the exact order
    /// [`crate::read_state::ReadStateManager::dispose`] is there to close.
    /// Dropping this handshake's own references is not enough either: that
    /// task's strong reference keeps the manager, its subscriptions and its
    /// context alive for as long as it runs.
    ///
    /// And it is TERMINAL — emptying the cache is only half the job, because
    /// ending it fires observer callbacks, and an observer that answers by
    /// calling an accessor would rebuild into the cache just emptied (rule 5
    /// and forbidden order 4 on [`Self::shared_query`]). So the `ended` bit
    /// goes up first, and after this returns every QUERY AND CURSOR accessor
    /// answers `None`, [`Self::write_session`] refuses without demanding,
    /// [`Self::is_authenticated`] and `can_demand_auth` both answer false — the
    /// pair that keeps the composer's gate standing down rather than treating a
    /// departed reader as a guest to raise a ceremony at — and the public
    /// generation answers its frozen final number. The raw session
    /// accessors — [`ChatContext::context`], [`ChatContext::viewer`] and
    /// their untracked forms — are the deliberate exception: they have no
    /// `None` to answer, and what they read is the HOST'S signal, whose
    /// lifetime is the host's own (the boundary paragraph on
    /// `Inner::generation` says exactly what survives). A handle kept past
    /// teardown must not call them.
    fn discard_all(&self) {
        // The frozen number first, while the arena is still alive — cleanups
        // run before the owner's nodes drop, so this read is safe even if a
        // set left the memo dirty. Then the terminal bit, BEFORE the cache is
        // taken: [`Self::end`] fires observer callbacks (dropped
        // registrations), and an observer that calls back into an accessor
        // during this teardown must find the handshake ended, not an empty
        // cache it would repopulate with a manager nothing will ever dispose.
        self.0.final_generation.set(self.0.generation.get_untracked());
        self.0.ended.set(true);
        let held = {
            let mut shared = self.0.shared.borrow_mut();
            let built_for = shared.built_for;
            std::mem::replace(&mut *shared, Shared { built_for, ..Shared::default() })
        };
        Self::end(Some(held));
    }

    /// Put the cursor managers' flags up and drop what was taken out of the
    /// cache — always after the borrow is released, because unsubscribing runs
    /// code that must not be holding it.
    fn end(taken: Option<Shared>) {
        if let Some(taken) = &taken {
            if let Some(cursors) = &taken.room_cursors {
                cursors.dispose();
            }
            if let Some(cursors) = &taken.dm_cursors {
                cursors.dispose();
            }
        }
        drop(taken);
    }

    /// The current generation if it has moved past `generation`, else `None`.
    ///
    /// The read recomputes the version, so a `.set()` that landed since the
    /// caller took its number is answered HERE rather than a tick later.
    ///
    /// `None` under `ended`, without touching the memo: after teardown there
    /// is no session to cross INTO — nothing may be published anyway — and a
    /// recompute would read the host's arena-allocated signal, which an owner
    /// disposed mid-call has already dropped. This and the continue arms going
    /// through [`Self::generation_untracked`] are what make rule 5's "every
    /// pass" hold on the straddling call too, not just the fresh one.
    fn crossed(&self, generation: u64) -> Option<u64> {
        if self.0.ended.get() {
            return None;
        }
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
        if self.0.ended.get() {
            return None;
        }
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
        // Ended is a refusal here, not just at the accessor's door: teardown
        // can land between a build and its publication, through the very
        // callbacks the build fired.
        if self.0.ended.get() {
            return (None, Some(query));
        }
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
        if self.0.ended.get() {
            return Some(registration);
        }
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
        if self.0.ended.get() {
            return (None, Some(value));
        }
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

    /// What to do when an anonymous reader reaches for the message box, or for
    /// something that writes. The host owns sign-in entirely; this is the whole
    /// of what the components know about it.
    ///
    /// OPTIONAL, and its absence is not merely a missing prompt: with no
    /// callback here the composer does not take focus away from an anonymous
    /// reader either, because there would be nothing to offer in its place.
    /// Such a host keeps exactly the behaviour it had before the composer's
    /// demand existed — a message box anyone may type into, whose send is
    /// refused with a warning in the log.
    ///
    /// MUST BE IDEMPOTENT: a new gesture while the ceremony is open raises it
    /// again, deliberately. See [`ChatContext::demand_auth`], which states both
    /// moments that raise it.
    pub fn on_auth_demand(mut self, demand: impl Fn() + 'static) -> Self {
        self.demand_auth = Some(Box::new(demand));
        self
    }

    pub fn hooks(mut self, hooks: ChatHooks) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn build(self) -> ChatContext {
        // WHICH SESSION THIS IS, as a version of the host's signal. The number
        // has to agree with the signal at every instant, so it is DERIVED from
        // it rather than moved alongside it: a memo marked dirty by a `.set()`
        // recomputes on the next read, whoever reads it and however, so no
        // accessor can find a number the host's set has already outrun. What
        // the number itself is does not matter — only that it DIFFERS from the
        // one before it — so the closure is `prev + 1` and reads nothing but
        // the session. That "differs" is load-bearing twice over: the swap
        // effect below re-runs only because the memo reports its value CHANGED
        // (an effect woken by a memo walks its sources and compares), and
        // `prev + 1` is strictly increasing, so it always does. A closure that
        // could ever repeat a value would silently stop the swap effect.
        // See `Inner::generation`; the reactive facts this stands on are
        // pinned by `tests/session_version.rs`, which carries a copy of this
        // closure — CHANGE THEM TOGETHER.
        let session = self.session;
        let generation = ArcMemo::new(move |prev: Option<&u64>| {
            session.track();
            prev.map_or(0, |p| p + 1)
        });

        let chat = ChatContext(SendWrapper::new(Arc::new(Inner {
            session,
            generation,
            ended: Cell::new(false),
            final_generation: Cell::new(0),
            rooms_where: self.rooms_where,
            shared: RefCell::new(Shared::default()),
            online: self.online.unwrap_or_else(|| Box::new(|| true)),
            can_moderate: self.can_moderate.unwrap_or_else(|| Box::new(|| false)),
            demand_auth: self.demand_auth,
            hooks: self.hooks,
        })));

        // THE SWAP, for a session nobody asks about. Everything the previous
        // session built is ENDED here — without waiting for a surface to call
        // an accessor and notice, because a surface that has since unmounted
        // never will, and a cursor manager left alive keeps live subscriptions
        // and (on the conversation side) goes on writing cursor repairs through
        // a context the reader has left.
        //
        // The version above is what makes this a disposal and nothing more:
        // validity has already moved with the host's set, so there is no
        // counter to bump here and no first-run case to skip. Reading the
        // generation both subscribes this effect and recomputes the version, so
        // the first run sees a set that landed in the mount tick — the run that
        // used to be skipped as "the mount, not a swap", which swallowed that
        // set for the life of the mount. On a mount with no set at all it hands
        // `discard_stale` the generation the cache is already on, and that is a
        // no-op by idempotence.
        //
        // Sets with no GENERATION read between them coalesce into one
        // recompute and therefore one disposal, against the last of them. That
        // is what should happen: a generation read is what makes a session
        // observed by the cache — a plain session read (a write path taking
        // its snapshot) observes a value without advancing the version — so
        // nothing was built against the ones in between, and the final
        // session is the one everything rebuilds against.
        //
        // Created under the owner that builds the handshake — the owner
        // `provide` runs in — and it lives exactly as long as that scope.
        Effect::new({
            let chat = chat.clone();
            move |_: Option<()>| {
                // The one tracked read. Everything below is untracked, so this
                // re-runs for the session and for nothing else.
                let generation = chat.0.generation.get();
                untrack(|| chat.discard_stale(generation));
            }
        });

        // AND WHEN THE TREE ITSELF GOES. The effect above is queued, so an
        // owner disposed before it next runs takes it away unrun — and dropping
        // this handshake's references is not disposal: a flush holding a strong
        // handle would find `disposed` still false and commit through the
        // departed session. So teardown raises the flags itself, synchronously.
        on_cleanup({
            let chat = chat.clone();
            move || chat.discard_all()
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
