//! The DM thread's message list and rows.
//!
//! Deliberately narrower than the room timeline's list and row: a DM row renders
//! the avatar gutter, the author/time meta, the markdown body with mention
//! chips and `:emoji:` glyphs, the "(edited)" marker and the tombstone — and
//! nothing else. Reactions, replies, link previews, the context menu and the
//! moderator actions are room affordances a private thread does not carry, so
//! the row does not carry their machinery.
//!
//! What IS shared, on purpose, because divergence there would be visible:
//! the CSS classes (`.messageRow`, `.messageBubble`, `.messageText`, the
//! `own`/`groupFirst`/`groupLast`/`tombstone` modifiers), the grouping rule
//! ([`crate::grouping`]), the markdown renderer, and the `data-msg-id`
//! contract the scroll pane locates rows by.
//!
//! A DM bubble carries the same `data-entity-id` / `data-collection` pair a
//! room bubble does, so a host's inspector reaches both the same way. What an
//! inspector may then SHOW of a deleted private message is that inspector's
//! call, not this row's.

use std::collections::HashMap;

use leptos::prelude::*;

use ankurah::{LiveQuery, View as _};
use ankurah_chat_model::{DmMessageView, UserView};
use ankurah_signals::Get as AnkurahGet;

use crate::context::{chat, Live};
use crate::fmt;

/// The thread's messages, grouped by author and day.
#[component]
pub fn DmMessageList(
    #[prop(into)] messages: Signal<Vec<DmMessageView>>,
    #[prop(into)] users: Live<LiveQuery<UserView>>,
    /// Who the reader is talking to — for the empty state.
    #[prop(into)] partner_name: Signal<String>,
) -> impl IntoView {
    // Identity from the session, for the reason the room list gives: a host's
    // own `User` row resolves asynchronously, and a thread that waited for it
    // rendered the reader's own messages as their correspondent's.
    let chat = chat();
    let viewer = Signal::derive(move || chat.viewer().map(|id| id.to_base64()));
    // Mention rendering: one id → display-name map shared by every row,
    // rebuilt when any display name changes. DM text carries the same `<@id>`
    // tokens room text does and renders them the same way. Whether a server
    // notifies the member named inside a private conversation is that
    // server's affair.
    let mention_names = Memo::new({
        let users = users.clone();
        move |_| {
            users
                .current()
                .get()
                .iter()
                .filter_map(|u| {
                    let name = u.display_name().unwrap_or_default();
                    (!name.is_empty()).then(|| (u.id().to_base64(), name))
                })
                .collect::<HashMap<String, String>>()
        }
    });

    let rows = Signal::derive(move || {
        let viewer = viewer.get();
        let msgs = messages.get();
        let keys: Vec<(String, i64)> = msgs
            .iter()
            .map(|m| (m.user().map(|r| r.id().to_base64()).unwrap_or_default(), m.timestamp().unwrap_or(0)))
            .collect();
        crate::grouping::group_flags(&keys)
            .into_iter()
            .zip(msgs)
            .map(|(flags, message)| (flags, message, viewer.clone()))
            .collect::<Vec<_>>()
    });

    view! {
        <Show
            when=move || !messages.get().is_empty()
            fallback=move || {
                let name = partner_name.get();
                view! {
                    <div class="emptyState">
                        <div class="emptyStateArt" aria-hidden="true">
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
                                stroke-linecap="round" stroke-linejoin="round">
                                <path d="M4 6h16v11H8l-4 4z" />
                                <path d="M9 11h6" />
                            </svg>
                        </div>
                        <div class="emptyStateTitle">{format!("No messages with {name} yet")}</div>
                        <div class="emptyStateHint">"Only the two of you can read this conversation."</div>
                    </div>
                }
            }
        >
            <For
                each=move || rows.get()
                key=|(flags, message, viewer): &(crate::grouping::GroupFlags, DmMessageView, Option<String>)| {
                    // Grouping context is part of the key so a row re-renders
                    // when a neighbour changes its group shape — and so is the
                    // reader, since whose message a row is decides its shape.
                    format!(
                        "{}|{}{}{}|{}",
                        message.id().to_base64(),
                        flags.first_in_group as u8,
                        flags.last_in_group as u8,
                        flags.day_label.is_some() as u8,
                        viewer.as_deref().unwrap_or("")
                    )
                }
                children={
                    let users = users.clone();
                    move |(flags, message, viewer)| {
                        view! {
                            <DmMessageRow
                                message=message
                                users=users.clone()
                                current_user_id=viewer
                                first_in_group=flags.first_in_group
                                day_label=flags.day_label
                                last_in_group=flags.last_in_group
                                mention_names=mention_names
                            />
                        }
                    }
                }
            />
        </Show>
    }
}

#[component]
fn DmMessageRow(
    message: DmMessageView,
    #[prop(into)] users: Live<LiveQuery<UserView>>,
    current_user_id: Option<String>,
    first_in_group: bool,
    last_in_group: bool,
    day_label: Option<String>,
    mention_names: Memo<HashMap<String, String>>,
) -> impl IntoView {
    let author_user_id = message.user().map(|r| r.id().to_base64()).unwrap_or_default();
    let is_own_message = current_user_id.as_deref() == Some(author_user_id.as_str());

    let author_name = {
        let users = users.clone();
        let author_user_id = author_user_id.clone();
        move || {
            users
                .current()
                .get()
                .iter()
                .find(|u| u.id().to_base64() == author_user_id)
                .map(|u| u.display_name().unwrap_or_default())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| "Unknown".to_string())
        }
    };
    let author_name_for_initials = author_name.clone();

    // Reactive: a remote tombstone flips the row live, exactly like a room
    // message. A DM tombstone can come from the sender, or from whatever a
    // deployment runs over its private traffic.
    let message_for_deleted = message.clone();
    let is_deleted = move || message_for_deleted.deleted().unwrap_or(false);
    let is_deleted_for_class = is_deleted.clone();

    let message_for_text = message.clone();
    let message_for_edited = message.clone();
    let ts = message.timestamp().unwrap_or(0);
    let time_str = fmt::clock_time(ts);
    let stamp = fmt::full_stamp(ts);
    let message_id = message.id().to_base64();
    // What a host's inspector finds this bubble's entity by. See the crate
    // docs; nothing in here reads them.
    let entity_id = message_id.clone();
    let collection = DmMessageView::collection().to_string();

    let row_class = {
        let mut c = String::from("messageRow");
        if is_own_message {
            c.push_str(" own");
        }
        if first_in_group {
            c.push_str(" groupFirst");
        }
        if last_in_group {
            c.push_str(" groupLast");
        }
        c
    };
    let avatar_hue = fmt::hue_class(&author_user_id);

    view! {
        {day_label.map(|label| {
            view! {
                <div class="dayDivider" aria-hidden="true">
                    <span class="dayDividerLabel">{label}</span>
                </div>
            }
        })}
        <div class=row_class>
            {(!is_own_message)
                .then(|| {
                    view! {
                        <div class="messageGutter">
                            {first_in_group
                                .then(|| {
                                    view! {
                                        <div class=format!("avatar {}", avatar_hue) aria-hidden="true">
                                            {move || fmt::initials(&author_name_for_initials())}
                                        </div>
                                    }
                                })}
                        </div>
                    }
                })}
            <div class="messageMain">
                {first_in_group
                    .then(|| {
                        if is_own_message {
                            view! {
                                <div class="messageMeta ownMeta">
                                    <span class="messageTime">{time_str.clone()}</span>
                                </div>
                            }
                                .into_any()
                        } else {
                            view! {
                                <div class="messageMeta">
                                    <span class="messageAuthor">{author_name.clone()}</span>
                                    <span class="messageTime">{time_str.clone()}</span>
                                </div>
                            }
                                .into_any()
                        }
                    })}
                <div
                    class=move || {
                        let mut classes = vec!["messageBubble"];
                        if is_own_message {
                            classes.push("ownMessage");
                        }
                        if is_deleted_for_class() {
                            classes.push("tombstone");
                        }
                        classes.join(" ")
                    }
                    data-msg-id=message_id.clone()
                    data-entity-id=entity_id
                    data-collection=collection
                    title=stamp
                >
                    <Show
                        when={
                            let is_deleted = is_deleted.clone();
                            move || is_deleted()
                        }
                        fallback={
                            let message_for_text = message_for_text.clone();
                            let message_for_edited = message_for_edited.clone();
                            move || {
                                let message_for_text = message_for_text.clone();
                                let message_for_edited = message_for_edited.clone();
                                view! {
                                    <div class="messageText">
                                        {move || {
                                            mention_names.with(|names| {
                                                crate::markdown::render_message(
                                                    &message_for_text.text().unwrap_or_default(),
                                                    names,
                                                )
                                            })
                                        }}
                                        {move || {
                                            message_for_edited
                                                .edited_at()
                                                .ok()
                                                .flatten()
                                                .map(|ts| {
                                                    view! {
                                                        <span
                                                            class="messageEdited"
                                                            title=format!("Edited {}", fmt::full_stamp(ts))
                                                        >
                                                            "(edited)"
                                                        </span>
                                                    }
                                                })
                                        }}
                                    </div>
                                }
                            }
                        }
                    >
                        // No attribution heuristic here, unlike room tombstones:
                        // a DM tombstone is either the sender's own removal or
                        // the rate limiter's, and the public ModAction the
                        // limiter writes is not readable per-message.
                        <div class="messageText tombstoneNotice">"Message removed"</div>
                    </Show>
                </div>
            </div>
        </div>
    }
}
