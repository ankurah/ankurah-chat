//! Direct messages: thread selection, race-safe find-or-create, and the send
//! path, plus the three surfaces built on them — the conversation view, the
//! sidebar section, and the read cursors.
//!
//! The one interesting problem here is that a two-party thread has no owner who
//! could allocate it. Both participants open it from their own client, so two
//! people (or two tabs of one person) can both find no thread and both create
//! one, and ankurah 0.9.0 has no entity deletion to clean the twin up with.
//!
//! The whole answer is agreement rather than prevention:
//!
//! 1. A CONVERSATION IS ITS TWO PEOPLE, never a row. Selection, the composer's
//!    target and every predicate below key on the participant pair, so there is
//!    no moment at which a reader is looking at "the wrong row" — the question
//!    does not arise.
//! 2. participants are stored in [`ankurah_chat_model::canonical_pair`] order
//!    and looked up in BOTH orders, so neither side can miss a thread the
//!    other created — not even one written with the pair reversed, which a
//!    policy has no way to refuse (see `find_or_create_thread`);
//! 3. when the lookup does return more than one, every reader picks the same
//!    one to WRITE into — [`ankurah_chat_model::canonical_thread`], the lowest
//!    entity id;
//! 4. and every READ spans all of the pair's rows, not just the winner
//!    ([`Conversation`], [`pair_rows`]) — the twin keeps whatever landed in it
//!    during the race, and agreeing on where to write next must not make what
//!    was already written unreachable.
//!
//! A deployment's own tests are where that convergence is proved end to end;
//! what this module owes them is that every path here agrees on
//! [`ankurah_chat_model::canonical_thread`].

use ankurah::{model::Mutable, EntityId, LiveQuery};
use ankurah_signals::Peek;
use ankurah_chat_model::{
    canonical_pair, canonical_thread, dm_partner, DmMessage, DmThread, DmThreadView, UserView, THREADS_FOR_PAIR,
};
use leptos::prelude::*;

mod message_list;
mod read_state;
mod sidebar;
mod thread_view;

pub use read_state::DmReadStateManager;
pub use sidebar::DmSidebar;
pub use thread_view::DmConversation;

use crate::context::{ChatContext, WriteSession};
use crate::queries;

/// One conversation per correspondent, as the UI has to treat it: the row
/// every reader agrees to call THE thread for that pair, plus every row the
/// pair has.
///
/// The extra rows are the losers of a first-DM race, and they are not inert.
/// Whoever wrote into one before the race resolved left their message THERE,
/// and no later message joins it. A view that reads only the agreed row can
/// therefore show an empty conversation — or hide it from the sidebar
/// entirely, since a thread with no messages is not listed — while the words
/// sit one row over. So activity, unread counts and the message timeline are
/// all read across `rows`; only what a click selects, and where a new message
/// is written, is [`Conversation::canonical`].
#[derive(Clone)]
pub struct Conversation {
    /// The lowest entity id for the pair — the row every client converges on.
    pub canonical: DmThreadView,
    /// Every row for the pair, canonical first, in id order.
    pub rows: Vec<EntityId>,
}

/// Group the viewer's threads by correspondent. Threads are keyed by their
/// participant pair rather than by their id, which is what makes duplicates
/// from a race collapse into one sidebar row — and the pair is canonicalized
/// on the way in, so a row stored in the reversed order (which policy permits;
/// see `find_or_create_thread`) groups with its twin rather than beside it.
pub fn conversations(threads: &[DmThreadView]) -> Vec<Conversation> {
    let mut by_pair: std::collections::HashMap<(EntityId, EntityId), Vec<DmThreadView>> = std::collections::HashMap::new();
    for thread in threads {
        let (Ok(a), Ok(b)) = (thread.a(), thread.b()) else { continue };
        by_pair.entry(canonical_pair(a.id(), b.id())).or_default().push(thread.clone());
    }
    let mut conversations: Vec<Conversation> = by_pair
        .into_values()
        .filter_map(|mut rows| {
            rows.sort_by_key(|t| t.id());
            let canonical = rows.first()?.clone();
            Some(Conversation { canonical, rows: rows.iter().map(|t| t.id()).collect() })
        })
        .collect();
    // Stable order for the caller to re-sort; ids are ULIDs, so this is
    // oldest-thread-first until the sidebar sorts by recent activity.
    conversations.sort_by_key(|c| c.canonical.id());
    conversations
}

/// Every thread row for the conversation between `viewer` and `partner`.
///
/// What it is for: a conversation opened while a first-message race is
/// resolving spans more than one row, and a view that read only one of them
/// would show an empty conversation with the words sitting one row over. Every
/// READ goes across the pair; only the write picks a row.
pub fn pair_rows(threads: &[DmThreadView], viewer: EntityId, partner: EntityId) -> Vec<EntityId> {
    let pair = canonical_pair(viewer, partner);
    let mut rows: Vec<EntityId> = threads
        .iter()
        .filter(|t| {
            let (Ok(ta), Ok(tb)) = (t.a(), t.b()) else { return false };
            canonical_pair(ta.id(), tb.id()) == pair
        })
        .map(|t| t.id())
        .collect();
    rows.sort();
    rows
}

/// The other participant of a thread, from the viewer's seat.
pub fn partner_of(thread: &DmThreadView, viewer: EntityId) -> Option<EntityId> {
    let (a, b) = (thread.a().ok()?.id(), thread.b().ok()?.id());
    dm_partner(a, b, viewer)
}

/// Open the conversation with `partner`, creating its thread row if this is
/// the first message either way.
///
/// The SELECTION is the partner's id, set immediately, so the conversation
/// opens without waiting on a round trip; the row is found or created behind
/// it. That ordering is also why the old converge-the-selection effect is
/// gone: a selection that names a person cannot drift onto the losing row of a
/// first-message race, because it never named a row.
///
/// Race-safe by construction rather than by locking: the lookup is on the
/// canonical pair, so it sees any thread the other side already created, and a
/// twin created in the same instant is read alongside it by every view (see
/// [`pair_rows`]). Fire-and-forget from a click handler; failures are logged.
pub fn open_thread_with(chat: &ChatContext, partner: EntityId, selected: RwSignal<Option<EntityId>>) {
    // Opening a conversation writes a thread row, and a thread has two named
    // participants — there is nothing to write until we know who the reader
    // is. The handshake comes in as an argument, and the session is resolved
    // here, because the future below cannot resolve either.
    let Some(session) = chat.write_session() else { return };
    let me = session.viewer;
    if partner == me {
        // The UI does not offer this (no "Message" button on your own card),
        // and a self-thread has no other participant to notify or name.
        tracing::warn!("refusing to open a DM thread with yourself");
        return;
    }
    selected.set(Some(partner));
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = find_or_create_thread(&session, partner).await {
            tracing::error!("Failed to open DM thread: {}", e);
        }
    });
}

async fn find_or_create_thread(session: &WriteSession, partner: EntityId) -> Result<DmThreadView, Box<dyn std::error::Error>> {
    let (a, b) = canonical_pair(session.viewer, partner);

    // Parameterized, never spliced. Both participants build this exact
    // query, which is what makes find-or-create converge.
    //
    // The lookup asks about both orderings, because the model cannot insist on
    // one — see `ankurah_chat_model::THREADS_FOR_PAIR`, where the source lives so
    // that a test can prove it parses (no test CI runs compiles this file).
    //
    // `deleted = false` is safe only while nothing tombstones threads, which is
    // today's ruling (see `DmThread::deleted`). The day something does, this
    // line stops finding the pair's thread and mints a second one beside it,
    // stranding the history in a row neither participant can reach again — so
    // whoever adds a thread tombstone owes this call an adoption path.
    let selection = queries::selection(
        &format!("{THREADS_FOR_PAIR} AND deleted = false"),
        [(&a).into(), (&b).into(), (&b).into(), (&a).into()],
    )?;
    let existing = session.context.fetch::<DmThreadView>(selection).await?;
    if let Some(winner) = canonical_thread(existing.iter().map(|t| t.id()))
        && let Some(row) = existing.into_iter().find(|t| t.id() == winner)
    {
        return Ok(row);
    }

    let trx = session.context.begin();
    let created = trx
        .create(&DmThread { a: a.into(), b: b.into(), created_at: js_sys::Date::now() as i64, deleted: false })
        .await?
        .read();
    trx.commit().await?;
    Ok(created)
}

/// Send a DM into `thread`.
///
/// `a`/`b` are copied verbatim from the thread — they are what lets a policy
/// read scope answer "may this reader see me" from the row alone — and the
/// sender is the session's reader, which a write scope pins to the caller
/// anyway. `wire_text` is already encoded by the composer (`@Name` runs
/// re-encoded to `<@id>` tokens), the same bytes a room message carries: DM
/// text renders mentions the same way, and what a server does about a mention
/// inside a private conversation is the server's affair.
///
/// The session is an argument rather than something looked up here, so the
/// author and the context are the pair the caller resolved before it deferred
/// — see [`crate::ChatContext::write_session`].
pub async fn send_dm(session: &WriteSession, partner: EntityId, wire_text: String) -> Result<(), Box<dyn std::error::Error>> {
    // A conversation with oneself has no other participant to name, notify or
    // scope a read to. `open_thread_with` refuses to start one; this refuses to
    // write into one, because the two ends can BECOME the same person without
    // anyone selecting that: a reader who had B open, and then signs in as B,
    // leaves a selection naming themselves. The session moved; the selection
    // did not.
    if partner == session.viewer {
        return Err("refusing to send a direct message to oneself".into());
    }
    // The row to write into is resolved from the pair, not carried in: a
    // conversation is its two participants, and which row represents it is
    // something a first-message race can still be deciding.
    let thread = find_or_create_thread(session, partner).await?;
    let a = thread.a()?;
    let b = thread.b()?;
    let trx = session.context.begin();
    trx.create(&DmMessage {
        thread: ankurah::Ref::from(&thread),
        a,
        b,
        user: session.viewer.into(),
        text: wire_text,
        timestamp: js_sys::Date::now() as i64,
        deleted: false,
        edited_at: None,
    })
    .await?;
    trx.commit().await?;
    Ok(())
}

/// Resolve a correspondent's display name from a users resultset, live.
pub fn display_name(users: &LiveQuery<UserView>, who: EntityId) -> String {
    users
        .peek()
        .iter()
        .find(|u| u.id() == who)
        .and_then(|u| u.display_name().ok())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Unknown".to_string())
}
