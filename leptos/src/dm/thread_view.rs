//! One conversation, laid out like a room.
//!
//! It stands on the same [`crate::scroll_pane::ScrollPane`] the room log does,
//! so the pinned-to-bottom contract, pagination and the `data-msg-id` DOM
//! contract are the same code rather than a parallel implementation. The
//! composer is the same component too ([`crate::Composer`]), with a DM target:
//! mention autocomplete, `@Name` re-encoding at send and `:emoji:` completion
//! all behave exactly as they do in a room.
//!
//! What a conversation deliberately does NOT carry: edit, delete, reply,
//! reactions, extras under the bubble, and the actions menu. Those are room
//! affordances with their own model fields and server-side workers behind
//! them. `DmMessage.edited_at` is rendered when present, so the field is
//! honest on the read side, but nothing here writes it.
//!
//! THE FILING KEY IS `thread`, NEVER `a`/`b`. The participants are denormalized
//! onto every message so the policy read scope can answer "may this user see
//! me" row-locally — they are not an index. A message whose `a`/`b` disagree
//! with its thread (which a participant CAN hand-craft; see the `DmMessage`
//! model doc) is simply mis-filed into a view nobody looks at, and that
//! containment is exactly this predicate.

use leptos::prelude::*;

use ankurah::LiveQuery;
use ankurah_signals::Get as AnkurahGet;
use ankurah_chat_model::{DmMessageView, DmThreadView, MessageView, UserView};

use super::message_list::DmMessageList;
use super::read_state::DmReadStateManager;
use crate::composer::{Composer, ComposerTarget};
use crate::context::{chat, Live};
use crate::scroll_pane::ScrollPane;
use crate::dm;

/// A DM conversation: its timeline and its composer.
///
/// Mountable on its own — a page that only offers "message me" mounts this and
/// nothing else.
///
/// Named for the conversation rather than the row, because `DmThread` is the
/// model's own struct name and a host that glob-imports both crates should not
/// have to disambiguate a component from a collection.
#[component]
pub fn DmConversation(
    thread: RwSignal<Option<DmThreadView>>,
    /// The reader's whole thread set, so an open conversation can be read
    /// across every row its pair has (see [`crate::dm::pair_rows`]).
    /// [`Live`], like the other host-supplied handles: signing in mid-visit
    /// means a new query, and swapping it in place is what keeps this
    /// conversation open across that.
    #[prop(into)]
    threads: Live<LiveQuery<DmThreadView>>,
    #[prop(into)]
    users: Live<LiveQuery<UserView>>,
    #[prop(into)]
    read_state: Live<DmReadStateManager>,
) -> impl IntoView {
    let pane = ScrollPane::<DmMessageView>::new();
    pane.install();

    // Which rows this conversation is spread across: normally just the
    // selected one, and more when a first-DM race left twins. Reactive,
    // because the losing twin can arrive after the view is already open.
    //
    // A MEMO, NOT A DERIVED SIGNAL. This reads the viewer's whole thread set,
    // and the effect below re-runs on anything it tracks. Unmemoized, that
    // effect tracks every thread the viewer has: someone opening a new
    // conversation with them, while they are scrolled back through history in a
    // different one, would call `set_source` again — tearing down the
    // ScrollManager, resetting pagination and snapping them to the live tail.
    // A memo only notifies when the row set actually differs, so the effect is
    // back to tracking the selection.
    let rows = {
        let threads = threads.clone();
        Memo::new(move |_| match thread.get() {
            Some(t) => dm::pair_rows(&threads.current().get(), &t),
            None => Vec::new(),
        })
    };

    // The composer's edit and reply state. A conversation never arms either —
    // they exist because the composer is shared with the room log — and owning
    // them here, rather than sharing the room's, is what guarantees a reply
    // armed in a room can never follow the reader into a private thread.
    let editing_message = RwSignal::new(None::<MessageView>);
    let replying_to = RwSignal::new(None::<MessageView>);
    let no_room_messages = Signal::derive(Vec::<MessageView>::new);

    Effect::new(move |_| {
        // The timeline is the union of the pair's rows, so a message written
        // into a race twin before the clients agreed on a winner is still part
        // of the conversation the reader sees. With no twins — the normal case
        // — this is the plain `thread = ?` it has always been.
        //
        // Tombstones stay in the timeline like room tombstones, so the
        // scroll shape does not jump when one appears.
        let ids = rows.get();
        let predicate = (!ids.is_empty()).then(|| {
            let src = vec!["thread = ?"; ids.len()].join(" OR ");
            crate::queries::predicate(&src, ids.iter().map(|id| id.into())).expect("dm message predicate parses")
        });
        pane.set_source(predicate, "timestamp DESC");
    });

    let messages = pane.items;

    let chat = chat();
    let partner_name = {
        let users = users.clone();
        Signal::derive(move || {
            let Some(t) = thread.get() else { return String::new() };
            // Track display-name edits: a rename retitles the open thread.
            let _ = users.current().get();
            // Tracked: signing in mid-visit has to name the correspondent.
            match chat.viewer().and_then(|me| dm::partner_of(&t, me)) {
                Some(partner) => dm::display_name(&users.current(), partner),
                None => "Yourself".to_string(),
            }
        })
    };

    // Advance the read cursor whenever the viewer is at the live tail — the
    // room rule, per thread. Every row of the pair gets the cursor, because
    // the sidebar's badge counts across all of them: leaving a twin's cursor
    // behind would leave a badge nothing can clear.
    let mark_read_at_tail = {
        let read_state = read_state.clone();
        move || {
            let Some(ts) = newest_timestamp(&messages.get_untracked()) else { return };
            let read_state = read_state.current_untracked();
            for id in rows.get_untracked() {
                read_state.mark_read(&id.to_base64(), ts);
            }
        }
    };
    Effect::new({
        let mark_read_at_tail = mark_read_at_tail.clone();
        move |_| {
            let _ = messages.get();
            if pane.is_live() {
                mark_read_at_tail();
            }
        }
    });

    view! {
        {
            let users = users.clone();
            let mark_read_at_tail = mark_read_at_tail.clone();
            move || {
                let current_thread = thread.get()?;
                let users = users.clone();

                let handle_scroll = pane.scroll_handler(mark_read_at_tail.clone());
                let handle_jump = {
                    let mark_read_at_tail = mark_read_at_tail.clone();
                    move |_| {
                        pane.scroll_to_bottom();
                        mark_read_at_tail();
                    }
                };

                Some(view! {
                    <div class="ankurah-chat chatContainer">
                        <div class="dmThreadHeader">
                            <span class="dmThreadWith">{move || partner_name.get()}</span>
                            <span class="dmThreadPrivacy" title="Only the two of you can read this conversation — not moderators.">
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                                    stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                                    <rect x="4" y="11" width="16" height="9" rx="2" />
                                    <path d="M8 11V7a4 4 0 0 1 8 0v4" />
                                </svg>
                                "Private"
                            </span>
                        </div>

                        <div class="messagesContainer" node_ref=pane.container_ref on:scroll=handle_scroll>
                            <div class="messagesContent" node_ref=pane.content_ref>
                                <DmMessageList
                                    messages=messages
                                    users=users.clone()
                                    partner_name=partner_name
                                />
                            </div>
                        </div>

                        <Show when=move || pane.show_jump_to_current.get()>
                            <button class="jumpToCurrent" on:click=handle_jump.clone()>
                                "Jump to latest"
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"
                                    stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                                    <path d="M12 5v14" />
                                    <path d="m6 13 6 6 6-6" />
                                </svg>
                            </button>
                        </Show>

                        <Composer
                            target=ComposerTarget::Dm(current_thread.clone())
                            editing_message=editing_message
                            replying_to=replying_to
                            messages=no_room_messages
                        />
                    </div>
                })
            }
        }
    }
}

fn newest_timestamp(messages: &[DmMessageView]) -> Option<i64> {
    messages.iter().filter_map(|m| m.timestamp().ok()).max()
}
