//! The "Direct messages" section of a sidebar, typically below the room list.
//!
//! The resultset is SELF-SHAPING: where a deployment's `dmthread` read scope
//! is `a = $jwt.sub OR b = $jwt.sub`, a plain `deleted = false` LiveQuery
//! returns exactly the reader's own conversations and nothing else. There is
//! no client-side membership filter on purpose — one would read as though the
//! privacy came from this file, and it does not. It comes from the server's
//! policy, which is where a deployment proves it.
//!
//! Each row names the OTHER participant and carries an unread badge from the
//! reader's own `DmReadState` cursor. Rows are ordered by their newest message,
//! most recent first, which is derived from the same per-thread windows the
//! unread counts come from — `DmThread` deliberately carries no `last_msg_ts`
//! field, because two participants racing to maintain one would be a write
//! conflict on every message for no reader benefit.
//!
//! Threads with no messages do not appear. That is the containment for empty
//! thread rows: anyone may open a thread with anyone (a write scope only stops
//! you opening threads between OTHER people), so a thread row on its own must
//! not be able to occupy space in a stranger's sidebar. The conversation
//! appears for the recipient when the first message does.

use leptos::prelude::*;

use ankurah::EntityId;
use ankurah_chat_model::DmThreadView;
use ankurah_signals::Get as AnkurahGet;

use crate::context::chat;
use crate::{dm, fmt};

#[component]
pub fn DmSidebar(
    /// Which correspondent's conversation is open, by id. A conversation is
    /// keyed on the person, not on the row representing it.
    selected_dm: RwSignal<Option<EntityId>>,
) -> impl IntoView {
    // Nobody signed in means no conversations of one's own. The section still
    // renders — a host that laid out a rail gets its heading and an empty
    // state, not a hole.
    //
    // TRACKED, and threaded down as a signal: a reader who signs in mid-visit
    // must start seeing correspondents' names in rows this component may
    // already have built.
    let chat = chat();
    let me = {
        let chat = chat.clone();
        Signal::derive(move || chat.viewer())
    };

    // Duplicate threads from a concurrent first-DM race collapse to one row per
    // correspondent; conversations with no messages are hidden, and the rest
    // sort by most recent activity.
    //
    // Activity is read across EVERY row of the pair rather than the one the
    // sidebar row stands for. A message written into the losing twin during a
    // race stays there forever, and a conversation whose only message landed
    // there would otherwise be filtered out of this list as empty — the
    // correspondent would simply disappear (see `dm::Conversation`).
    let rows = {
        let chat = chat.clone();
        Signal::derive(move || {
            let Some(read_state) = chat.dm_cursors() else { return Vec::new() };
            let threads = chat.dm_threads().map(|q| q.get()).unwrap_or_default();
            let mut rows: Vec<(dm::Conversation, i64)> = dm::conversations(&threads)
                .into_iter()
                .map(|conversation| {
                    let newest = conversation.rows.iter().map(|id| read_state.newest_ts(&id.to_base64())).max().unwrap_or(0);
                    (conversation, newest)
                })
                .filter(|(_, newest)| *newest > 0)
                .collect();
            rows.sort_by(|(a, a_ts), (b, b_ts)| b_ts.cmp(a_ts).then_with(|| a.canonical.id().cmp(&b.canonical.id())));
            rows.into_iter().map(|(conversation, _)| conversation).collect::<Vec<_>>()
        })
    };

    let rows_for_empty = rows;

    view! {
        <div class="ankurah-chat sidebarHeader dmSectionHeader">
            <span class="sidebarTitle">"Direct messages"</span>
        </div>
        <div class="ankurah-chat roomList dmList">
            <Show when=move || rows_for_empty.get().is_empty()>
                <div class="emptyRooms">
                    "No conversations yet — open a member and choose Message."
                </div>
            </Show>
            <For
                each=move || rows.get()
                // Keyed on the WHOLE row set, not on the canonical row. The
                // child reads `rows` once, so it has to be rebuilt whenever
                // that set changes — and half the time it does not change the
                // canonical row: a first-DM race twin that arrives with a
                // HIGHER id joins the pair without displacing the lowest id, so
                // a key of `canonical` alone would hold the old child, and its
                // badge would go on counting one row of a two-row conversation
                // for the rest of the session. The row set is unique per
                // conversation (rows are grouped by participant pair), so this
                // is still one stable key per sidebar entry.
                key=|conversation: &dm::Conversation| conversation.rows.clone()
                children=move |conversation: dm::Conversation| {
                    view! {
                        <DmListItem
                            thread=conversation.canonical
                            rows=conversation.rows
                            selected_dm=selected_dm
                            me=me
                        />
                    }
                }
            />
        </div>
    }
}

#[component]
fn DmListItem(
    thread: DmThreadView,
    /// Every thread row this conversation is spread across (see
    /// `dm::Conversation`) — what the unread badge counts over.
    rows: Vec<EntityId>,
    selected_dm: RwSignal<Option<EntityId>>,
    /// The reader, when there is one. Without one there is no "other
    /// participant" to name — and signing in mid-visit has to fill the name
    /// in, so this tracks rather than being read once.
    me: Signal<Option<EntityId>>,
) -> impl IntoView {
    let chat = chat();
    let thread_for_partner = thread.clone();
    let partner = Signal::derive(move || me.get().and_then(|me| dm::partner_of(&thread_for_partner, me)));

    // Reactive: a rename retitles the row without a reload.
    let partner_name = {
        let chat = chat.clone();
        move || match partner.get() {
            Some(p) => {
                let Some(members) = chat.members() else { return "Unknown".to_string() };
                let _ = members.get();
                dm::display_name(&members, p)
            }
            // A self-thread has no other participant. The UI never creates
            // one; naming it honestly beats rendering "Unknown".
            None => "You".to_string(),
        }
    };
    let partner_name_for_initials = partner_name.clone();
    let hue = move || fmt::hue_class(&partner.get().map(|p| p.to_base64()).unwrap_or_default());

    let is_selected = move || selected_dm.get().is_some() && selected_dm.get() == partner.get();
    let row_keys: Vec<String> = rows.iter().map(|id| id.to_base64()).collect();

    view! {
        <div
            class=move || if is_selected() { "roomItem dmItem selected" } else { "roomItem dmItem" }
            on:click=move |_| selected_dm.set(partner.get_untracked())
        >
            <span class=move || format!("dmAvatar {}", hue()) aria-hidden="true">
                {move || fmt::initials(&partner_name_for_initials())}
            </span>
            <span class="roomLabel">{partner_name}</span>
            {move || {
                // Across the pair's rows, for the same reason the sidebar's
                // activity is: unread messages can be sitting in a race twin.
                let unread_count: usize =
                    chat.dm_cursors().map(|c| row_keys.iter().map(|key| c.unread_count(key)).sum()).unwrap_or(0);
                (unread_count > 0).then(|| {
                    let badge_text = if unread_count >= 10 { "10+".to_string() } else { unread_count.to_string() };
                    view! { <span class="unreadBadge">{badge_text}</span> }
                })
            }}
        </div>
    }
}
