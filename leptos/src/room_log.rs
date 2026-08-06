use leptos::prelude::*;

use ankurah::EntityId;
use ankurah_chat_model::MessageView;

use crate::composer::{ComposerTarget, WiredComposer};
use crate::context::chat;
use crate::debug_header::TimelineDebugHeader;
use crate::message_list::MessageList;
use crate::scroll_pane::ScrollPane;

/// One room's timeline, with its composer underneath.
///
/// The timeline machinery — the `ScrollManager`, the pinned-to-bottom
/// contract, the pagination handler — lives in [`crate::scroll_pane`], shared
/// with the DM thread view so the two cannot drift apart. What is here is what
/// is specific to a room: the room predicate, the reply and edit state the
/// composer and the rows share, and the read cursor.
///
/// Mountable on its own. A page that shows exactly one room passes that room's
/// id and never mounts a [`crate::RoomSelector`] at all.
///
/// It takes an ID, not a room. Everything it needs beyond the id — the members
/// list for author names, the read cursors — comes from the handshake, which
/// owns them for the session and rebuilds them when the session moves. A host
/// holds an id and nothing else.
#[component]
pub fn RoomLog(
    /// Which room to show, or none for the empty state.
    #[prop(into)]
    room: Signal<Option<EntityId>>,
    /// Whether to offer the timeline's diagnostic header — scroll mode,
    /// pagination flags, item count — behind a small toggle. Off by default:
    /// an embedded panel has no use for it.
    #[prop(optional)]
    debug_header: bool,
) -> impl IntoView {
    let show_debug = RwSignal::new(false);
    let editing_message = RwSignal::new(None::<MessageView>);
    // The message the next send attaches as `re`. Owned here, like
    // editing_message, because both the rows' actions menus and the composer
    // read and write it.
    let replying_to = RwSignal::new(None::<MessageView>);

    let chat = chat();
    let pane = ScrollPane::<MessageView>::new();
    pane.install();

    // (Re)point the pane whenever the selected room changes — and whenever the
    // host swaps the session, since `set_source` reads the ankurah context
    // tracked.
    Effect::new(move |_| {
        let predicate = room.get().map(|room_id| {
            // Deleted messages are deliberately NOT filtered out: they render
            // as tombstone rows, so the scroll timeline keeps its shape.
            crate::queries::predicate("room = ?", [(&room_id).into()]).expect("static message predicate parses")
        });
        pane.set_source(predicate, "timestamp DESC");
    });

    let messages = pane.items;

    // A reply armed in one room must not attach to a message sent from
    // another: an actual room CHANGE disarms it. `prev` carries the previous
    // room id so re-selecting the same room keeps the chip.
    Effect::new(move |prev: Option<Option<EntityId>>| {
        let id = room.get();
        if let Some(prev) = prev {
            if prev != id {
                replying_to.set(None);
            }
        }
        id
    });

    // Advance the persistent read cursor whenever the reader is looking at the
    // live tail of a room: on room switch, and again as new messages arrive
    // while live (this effect tracks `messages`). A scrolled-up reader keeps
    // their cursor — browsing history marks nothing read.
    let mark_read_at_tail = {
        let chat = chat.clone();
        move || {
            let Some(cursors) = chat.room_cursors() else { return };
            if let (Some(room_id), Some(ts)) = (room.get_untracked(), newest_timestamp(&messages.get_untracked())) {
                cursors.mark_read(&room_id.to_base64(), ts);
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
        <Show
            when=move || room.get().is_some()
            fallback=|| {
                view! {
                    <div class="ankurah-chat chatContainer">
                        <div class="emptyState">
                            <div class="emptyStateArt" aria-hidden="true">
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
                                    stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M21 11.5a8.4 8.4 0 0 1-9 8.4 8.9 8.9 0 0 1-3.9-.9L3 20l1-4.9a8.3 8.3 0 0 1-1-4A8.4 8.4 0 0 1 12 3a8.4 8.4 0 0 1 9 8.5z" />
                                    <path d="M8 10.5h8" />
                                    <path d="M8 14h5" />
                                </svg>
                            </div>
                            <div class="emptyStateTitle">"Select a room to start chatting"</div>
                            <div class="emptyStateHint">
                                "Pick a room from the sidebar — every message syncs live."
                            </div>
                        </div>
                    </div>
                }
            }
        >
            {
                let mark_read_at_tail = mark_read_at_tail.clone();
                move || {
                    let current_room = room.get()?;

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
                            <Show when=move || debug_header && show_debug.get()>
                                <TimelineDebugHeader
                                    mode=pane.mode_str
                                    has_more_preceding=pane.has_more_preceding
                                    has_more_following=pane.has_more_following
                                    should_auto_scroll=pane.should_auto_scroll
                                    item_count=pane.item_count
                                />
                            </Show>

                            <Show when=move || debug_header>
                                <button
                                    class="debugToggle"
                                    on:click=move |_| show_debug.update(|v| *v = !*v)
                                    title=move || if show_debug.get() { "Hide debug info" } else { "Show debug info" }
                                >
                                    {move || if show_debug.get() { "▼" } else { "▲" }}
                                </button>
                            </Show>

                            <div class="messagesContainer" node_ref=pane.container_ref on:scroll=handle_scroll>
                                <div class="messagesContent" node_ref=pane.content_ref>
                                    <MessageList
                                        messages=messages
                                        editing_message=editing_message
                                        replying_to=replying_to
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

                            <WiredComposer
                                target=ComposerTarget::Room(current_room)
                                editing_message=editing_message
                                replying_to=replying_to
                                messages=messages
                            />
                        </div>
                    })
                }
            }
        </Show>
    }
}

/// Newest message timestamp in a visible set (they arrive ordered, but max()
/// is cheap and immune to ordering changes).
fn newest_timestamp(messages: &[MessageView]) -> Option<i64> {
    messages.iter().filter_map(|m| m.timestamp().ok()).max()
}
