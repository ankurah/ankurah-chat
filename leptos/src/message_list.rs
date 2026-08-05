use std::collections::{HashMap, HashSet};

use leptos::prelude::*;

use ankurah::LiveQuery;
use ankurah_chat_model::{MessageView, ReactionView, UserView};
use ankurah_signals::Get as AnkurahGet;
use send_wrapper::SendWrapper;

use crate::context::chat;
use crate::message_row::MessageRow;
use crate::query_registry::{self, QueryRegistration};
use crate::reactions::{picker_index, ReactionChip};

/// One renderable row: a message plus its computed grouping context. The
/// grouping RULE is shared with the DM thread list (see `crate::grouping`) so
/// the two timelines lay out identically.
#[derive(Clone)]
struct RowCtx {
    message: MessageView,
    first_in_group: bool,
    last_in_group: bool,
    /// Day-separator label rendered above this row when the calendar day changes.
    day_label: Option<String>,
}

fn group_rows(msgs: &[MessageView]) -> Vec<RowCtx> {
    let keys: Vec<(String, i64)> = msgs
        .iter()
        .map(|m| (m.user().map(|r| r.id().to_base64()).unwrap_or_default(), m.timestamp().unwrap_or(0)))
        .collect();
    crate::grouping::group_flags(&keys)
        .into_iter()
        .zip(msgs.iter().cloned())
        .map(|(flags, message)| RowCtx {
            message,
            first_in_group: flags.first_in_group,
            last_in_group: flags.last_in_group,
            day_label: flags.day_label,
        })
        .collect()
}

/// Message list component that displays messages grouped by author and day.
#[component]
pub fn MessageList(
    #[prop(into)] messages: Signal<Vec<MessageView>>,
    users: LiveQuery<UserView>,
    current_user_id: Option<String>,
    editing_message: RwSignal<Option<MessageView>>,
    /// The composer's reply state, armed from the rows' context menus.
    replying_to: RwSignal<Option<MessageView>>,
) -> impl IntoView {
    let rows = Signal::derive(move || group_rows(&messages.get()));

    // Mention rendering: one id → display-name map shared by every row,
    // rebuilt when the users list (or any display name — View field reads are
    // tracked) changes. Rows' text closures read it through `.with`, so a
    // rename re-renders mentions live without per-row user lookups.
    let mention_names = Memo::new({
        let users = users.clone();
        move |_| {
            users
                .get()
                .iter()
                .filter_map(|u| {
                    let name = u.display_name().unwrap_or_default();
                    (!name.is_empty()).then(|| (u.id().to_base64(), name))
                })
                .collect::<HashMap<String, String>>()
        }
    });

    // Reactions: one standing LiveQuery over active reactions, grouped into
    // render-ready chips per message id. `Reaction` has no room ref, so a
    // room-scoped predicate is inexpressible; per-row queries would churn
    // subscriptions with every virtual-scroll mount and unmount. See
    // reactions.rs.
    //
    // Built in an effect rather than in the body, and held in a signal, so a
    // host that swaps the session — a reader signing in mid-visit — gets the
    // query rebuilt against the new context without this list remounting. The
    // registration guard rides in the same signal: dropping it is what tells
    // an attached observer the old query is gone.
    let reactions = RwSignal::new(None::<SendWrapper<LiveQuery<ReactionView>>>);
    let reactions_registration = StoredValue::new(None::<QueryRegistration>);
    Effect::new(move |_| match chat().context().query::<ReactionView>("active = true") {
        Ok(query) => {
            reactions_registration.set_value(Some(query_registry::register("reactions (message list)", &query)));
            reactions.set(Some(SendWrapper::new(query)));
        }
        Err(e) => {
            tracing::error!("Failed to create the reactions LiveQuery: {:?}", e);
            reactions_registration.set_value(None);
            reactions.set(None);
        }
    });
    let viewer_id = current_user_id.clone();
    let reaction_chips = Memo::new(move |_| {
        // Distinct users per (message, emoji): duplicate rows (possible under
        // concurrent first-toggles) count once.
        let mut sets: HashMap<String, HashMap<String, HashSet<String>>> = HashMap::new();
        let rows = reactions.get().map(|q| q.get()).unwrap_or_default();
        for row in rows.iter() {
            if !row.active().unwrap_or(false) {
                continue;
            }
            let (Ok(message), Ok(user), Ok(emoji)) = (row.message(), row.user(), row.emoji()) else {
                continue;
            };
            sets.entry(message.id().to_base64())
                .or_default()
                .entry(emoji)
                .or_default()
                .insert(user.id().to_base64());
        }
        sets.into_iter()
            .map(|(message_id, by_emoji)| {
                let mut chips: Vec<ReactionChip> = by_emoji
                    .into_iter()
                    .map(|(emoji, users)| ReactionChip {
                        mine: viewer_id.as_deref().map(|id| users.contains(id)).unwrap_or(false),
                        count: users.len(),
                        emoji,
                    })
                    .collect();
                chips.sort_by(|a, b| (picker_index(&a.emoji), &a.emoji).cmp(&(picker_index(&b.emoji), &b.emoji)));
                (message_id, chips)
            })
            .collect::<HashMap<String, Vec<ReactionChip>>>()
    });

    view! {
        <Show
            when=move || !messages.get().is_empty()
            fallback=|| {
                view! {
                    <div class="emptyState">
                        <div class="emptyStateArt" aria-hidden="true">
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
                                stroke-linecap="round" stroke-linejoin="round">
                                <path d="M12 22v-9" />
                                <path d="M9.5 9.4c1.1.8 1.8 2.2 2.3 3.7-2 .4-3.5.4-4.8-.3-1.2-.6-2.3-1.9-3-4.2 2.8-.5 4.4 0 5.5.8z" />
                                <path d="M14.1 6a7 7 0 0 0-1.1 4c1.9-.1 3.3-.6 4.3-1.4 1-1 1.6-2.3 1.7-4.6-2.7.1-4 1-4.9 2z" />
                            </svg>
                        </div>
                        <div class="emptyStateTitle">"No messages yet"</div>
                        <div class="emptyStateHint">"Be the first to say hello — plant the seed."</div>
                    </div>
                }
            }
        >
            <For
                each=move || rows.get()
                key=|row: &RowCtx| {
                    // Grouping context is part of the key so a row re-renders when
                    // a neighbor changes its group shape (e.g. a follow-up arrives).
                    format!(
                        "{}|{}{}{}",
                        row.message.id().to_base64(),
                        row.first_in_group as u8,
                        row.last_in_group as u8,
                        row.day_label.is_some() as u8
                    )
                }
                children={
                    let users = users.clone();
                    let current_user_id = current_user_id.clone();
                    move |row: RowCtx| {
                        view! {
                            <MessageRow
                                message=row.message
                                users=users.clone()
                                current_user_id=current_user_id.clone()
                                editing_message=editing_message
                                replying_to=replying_to
                                first_in_group=row.first_in_group
                                last_in_group=row.last_in_group
                                day_label=row.day_label
                                reaction_chips=reaction_chips
                                mention_names=mention_names
                            />
                        }
                    }
                }
            />
        </Show>
    }
}
