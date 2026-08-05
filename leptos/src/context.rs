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
//! ankurah context itself: every hook is optional, and a surface with none of
//! them set is a working read-and-write chat that simply offers fewer doors
//! out to the rest of the application.
//!
//! # Signing in without remounting
//!
//! The session is a signal, not a value handed over once. A host may open on
//! an anonymous context — the components render, the timeline fills, the
//! composer refuses to send and calls the host's auth-demand callback instead
//! — and then, once the reader signs in, call [`ChatContext::set_session`]
//! with the authenticated context. The components re-point their queries where
//! they stand: the scroll position, the draft text, the open thread and the
//! armed reply all survive, because nothing unmounted.
//!
//! Read-cursor managers are the exception, and deliberately so: they are
//! constructed by the host (they need the reader's own id to scope their rows)
//! and a host that upgrades a session builds a new one.

use std::sync::Arc;

use ankurah::{Context, EntityId};
use ankurah_chat_model::MessageView;
use leptos::prelude::*;
use send_wrapper::SendWrapper;

/// Who the components are acting as, and the ankurah context they act through.
#[derive(Clone)]
pub struct Session {
    pub context: Context,
    /// The reader's own `User` entity id, or `None` for an anonymous reader.
    /// This is what "is this my message", "did I react to this" and "who is
    /// creating this room" are answered from.
    pub viewer: Option<EntityId>,
}

/// A place for a host to render something of its own, given the message the
/// component is rendering.
pub type MessageSlot = Box<dyn Fn(MessageView) -> AnyView>;

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
}

/// The host handshake, as the components see it. Build one with
/// [`ChatContext::new`].
#[derive(Clone)]
pub struct ChatContext(SendWrapper<Arc<Inner>>);

struct Inner {
    session: ArcRwSignal<SendWrapper<Session>>,
    online: Box<dyn Fn() -> bool>,
    can_moderate: Box<dyn Fn() -> bool>,
    demand_auth: Option<Box<dyn Fn()>>,
    hooks: ChatHooks,
}

/// Collects the handshake's parts. Only the ankurah context is required.
pub struct ChatContextBuilder {
    session: Session,
    online: Option<Box<dyn Fn() -> bool>>,
    can_moderate: Option<Box<dyn Fn() -> bool>>,
    demand_auth: Option<Box<dyn Fn()>>,
    hooks: ChatHooks,
}

impl ChatContext {
    /// Start a handshake against `context`, with no reader signed in.
    pub fn new(context: Context) -> ChatContextBuilder {
        ChatContextBuilder {
            session: Session { context, viewer: None },
            online: None,
            can_moderate: None,
            demand_auth: None,
            hooks: ChatHooks::default(),
        }
    }

    /// Point the mounted components at a different ankurah context, reader, or
    /// both — the sign-in path. Queries re-point in place; nothing unmounts.
    pub fn set_session(&self, context: Context, viewer: Option<EntityId>) {
        self.0.session.set(SendWrapper::new(Session { context, viewer }));
    }

    /// The ankurah context to read and write through. Reading this in an
    /// `Effect` or a `Memo` is what makes a query re-point when the host
    /// swaps the session.
    pub fn context(&self) -> Context { self.0.session.get().context.clone() }

    /// The context without subscribing — for event handlers and write paths,
    /// which want the session as of now and must not re-run on a swap.
    pub fn context_untracked(&self) -> Context { self.0.session.get_untracked().context.clone() }

    /// The reader's own entity id, tracked.
    pub fn viewer(&self) -> Option<EntityId> { self.0.session.get().viewer }

    /// The reader's own entity id, untracked.
    pub fn viewer_untracked(&self) -> Option<EntityId> { self.0.session.get_untracked().viewer }

    /// Whether someone is signed in, tracked. Drives the composer's choice
    /// between sending and calling [`Self::demand_auth`].
    pub fn is_authenticated(&self) -> bool { self.viewer().is_some() }

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
}

impl ChatContextBuilder {
    /// Who is signed in right now. Leave unset for an anonymous start.
    pub fn viewer(mut self, viewer: Option<EntityId>) -> Self {
        self.session.viewer = viewer;
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
        ChatContext(SendWrapper::new(Arc::new(Inner {
            session: ArcRwSignal::new(SendWrapper::new(self.session)),
            online: self.online.unwrap_or_else(|| Box::new(|| true)),
            can_moderate: self.can_moderate.unwrap_or_else(|| Box::new(|| false)),
            demand_auth: self.demand_auth,
            hooks: self.hooks,
        })))
    }

    /// Build and install the handshake for everything mounted below this
    /// point, returning the handle for a later [`ChatContext::set_session`].
    pub fn provide(self) -> ChatContext {
        let chat = self.build();
        provide_context(chat.clone());
        chat
    }
}

/// The handshake, from inside a component. Panics when a component is mounted
/// without one, which is a wiring mistake rather than a runtime condition.
pub(crate) fn chat() -> ChatContext {
    use_context::<ChatContext>().expect("mount chat components below ChatContext::new(..).provide()")
}

/// The ankurah context, tracked. Shorthand for the reads that appear on every
/// query-building path.
pub(crate) fn ctx() -> Context { chat().context() }

/// The ankurah context, untracked — for write paths and event handlers.
pub(crate) fn ctx_untracked() -> Context { chat().context_untracked() }
