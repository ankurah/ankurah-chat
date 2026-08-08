use std::collections::{HashMap, HashSet};

use leptos::prelude::*;

use ankurah::EntityId;
use ankurah_chat_model::{parse_mentions, MessageView, UserView};
use ankurah_signals::Get as AnkurahGet;

use crate::context::chat;
use crate::message_row::MessageRow;
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
    /// Who the reader was when this row was built. Part of the `For` key,
    /// because whether a message is the reader's own decides the row's shape.
    viewer: Option<String>,
}

fn group_rows(msgs: &[MessageView], viewer: Option<String>) -> Vec<RowCtx> {
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
            viewer: viewer.clone(),
        })
        .collect()
}

/// The room timeline's rows, grouped by author and day.
///
/// Internal: it is what [`crate::RoomLog`] renders inside its scroll pane, and
/// it takes a window of already-paged messages rather than deciding anything
/// about which. Mounting it alone would mean owning the pane too.
#[component]
pub(crate) fn MessageList(
    #[prop(into)] messages: Signal<Vec<MessageView>>,
    editing_message: RwSignal<Option<MessageView>>,
    /// The composer's reply state, armed from the rows' context menus.
    replying_to: RwSignal<Option<MessageView>>,
) -> impl IntoView {
    let chat = chat();
    // Who the reader is comes from the session, not from a prop. A host's own
    // `User` row resolves asynchronously, and a timeline that waited for it
    // rendered the reader's own messages as somebody else's — no avatar
    // gutter, no Edit, no Delete — until it arrived. The session knows from the
    // first frame, and reading it TRACKED means a sign-in mid-visit re-keys the
    // rows rather than leaving them wrong.
    let viewer = {
        let chat = chat.clone();
        Signal::derive(move || chat.viewer().map(|id| id.to_base64()))
    };
    let rows = Signal::derive(move || group_rows(&messages.get(), viewer.get()));

    // Every user a row names, resolved by REF — one `get_cached` per distinct
    // id, into one shared map every surface reads. Three kinds of id flow
    // through this single lookup: the AUTHOR who wrote each row, everyone a
    // row MENTIONS (the same `<@id>` tokens the body renders as chips), and
    // the AUTHOR OF THE MESSAGE a row replies to (reached by first following
    // the reply's `re` ref to that message). NOT the roster: `members()` lists
    // the whole user collection, and listing is a member privilege — a
    // signed-out reader's roster opens and stays empty, which rendered every
    // author, mention chip and reply-preview name "Unknown". Following a
    // message's own refs is the one user read a guest session is allowed, and
    // the local cache makes repeat ids and re-pagination free. An id the
    // session cannot resolve — refused, absent, failed — maps to None, and
    // its surfaces keep the "Unknown"/"@unknown" fallback.
    //
    // SNAPSHOT SEMANTICS, ACCEPTED FOR NOW: a ref follow leaves no standing
    // subscription behind, so a rename does not live-update the labels
    // resolved here — author names, mention chips and reply-preview names
    // alike. The roster-lively surfaces that remain (composer autocomplete,
    // the members panel, DM threads — all members-only) keep their liveness.
    // Live named reads here are a planned later change, gated on a jwt-auth
    // follow-up.
    let authors = RwSignal::new(HashMap::<String, Option<UserView>>::new());
    // Ids already resolved or in flight — the dedupe that makes a repeat id
    // (an author who is also mentioned, a mention who also authors) cost
    // nothing. Discarded with the map when the session moves.
    let requested = StoredValue::new(HashSet::<String>::new());
    // Reply-target MESSAGE ids already followed. A replied-to message's author
    // is only knowable AFTER that message resolves, so the user-id dedupe
    // above cannot fire until then; this second set stops each re-run of the
    // effect (a new message, a fresh page) from re-following the same targets.
    // Cleared with `requested` on a session swap.
    let requested_targets = StoredValue::new(HashSet::<String>::new());
    Effect::new({
        let chat = chat.clone();
        move |built_for: Option<u64>| {
            // Tracked: a session swap rebuilds the map through the arriving
            // context, exactly like the handshake's own queries.
            let generation = chat.generation();
            if built_for.is_some_and(|prev| prev != generation) {
                requested.update_value(|r| r.clear());
                requested_targets.update_value(|r| r.clear());
                authors.set(HashMap::new());
            }
            let ctx = chat.context_untracked();

            // Resolve one user id into the shared map, once. The dedupe is on
            // the id, so an id reached three ways — author, mention, reply-
            // target author — costs a single read. The spawned fetch drops its
            // write when the list was disposed mid-loop or the session moved
            // on before it landed.
            let resolve_user = {
                let ctx = ctx.clone();
                let chat = chat.clone();
                move |id: EntityId| {
                    let id_b64 = id.to_base64();
                    // Some(false) = already resolved or in flight; None = the
                    // list was disposed under this very loop.
                    if requested.try_update_value(|r| r.insert(id_b64.clone())) != Some(true) {
                        return;
                    }
                    let ctx = ctx.clone();
                    let chat = chat.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let resolved = match ctx.get_cached::<UserView>(id).await {
                            Ok(user) => Some(user),
                            Err(e) => {
                                tracing::warn!("Failed to resolve user {}: {}", id_b64, e);
                                None
                            }
                        };
                        // A resolution that outlived its session must not land
                        // in the map the arriving session is filling.
                        if chat.generation_untracked() != generation {
                            return;
                        }
                        // try_update: the list may have been unmounted (room
                        // switch) before the fetch resolved.
                        let _ = authors.try_update(|m| {
                            m.insert(id_b64, resolved);
                        });
                    });
                }
            };

            let msgs = messages.get();
            for message in msgs.iter() {
                // The author who wrote this row.
                if let Ok(user) = message.user() {
                    resolve_user(user.id());
                }
                // Everyone this row mentions — the canonical `<@id>` scanner
                // shared with the server, so a chip's name comes from the same
                // map the author's does.
                for id_b64 in parse_mentions(&message.text().unwrap_or_default()) {
                    if let Ok(id) = EntityId::from_base64(&id_b64) {
                        resolve_user(id);
                    }
                }
                // The message this row replies to: naming its author, and the
                // mentions inside its one-line snippet, needs that message
                // first — so this is a two-hop follow through the same map.
                // Deduped on the target id so re-runs do not re-follow it.
                if let Ok(Some(target)) = message.re() {
                    let target_id = target.id();
                    if requested_targets.try_update_value(|r| r.insert(target_id.to_base64())) == Some(true) {
                        let resolve_user = resolve_user.clone();
                        let ctx = ctx.clone();
                        let chat = chat.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            let Ok(original) = ctx.get::<MessageView>(target_id).await else { return };
                            // The target outliving its session is dropped like
                            // any other stale resolution.
                            if chat.generation_untracked() != generation {
                                return;
                            }
                            if let Ok(user) = original.user() {
                                resolve_user(user.id());
                            }
                            for id_b64 in parse_mentions(&original.text().unwrap_or_default()) {
                                if let Ok(id) = EntityId::from_base64(&id_b64) {
                                    resolve_user(id);
                                }
                            }
                        });
                    }
                }
            }
            generation
        }
    });

    // Mention rendering: one id → display-name map shared by every row, built
    // from the SAME by-ref resolutions above rather than the roster, so a chip
    // resolves for a signed-out reader exactly as an author name does. Rows
    // read it through `.with`, so a resolution landing (or the session moving)
    // re-renders the chips it names; an id still resolving, or resolved to
    // None, is simply absent and renders the "@unknown" fallback.
    let mention_names = Memo::new(move |_| {
        authors.with(|m| {
            m.iter()
                .filter_map(|(id, user)| {
                    let name = user.as_ref()?.display_name().unwrap_or_default();
                    (!name.is_empty()).then(|| (id.clone(), name))
                })
                .collect::<HashMap<String, String>>()
        })
    });

    // Render-ready chips per message id, from the handshake's one standing
    // reactions query.
    let reaction_chips = Memo::new({
        let chat = chat.clone();
        move |_| {
        let viewer_id = viewer.get();
        // Distinct users per (message, emoji): duplicate rows (possible under
        // concurrent first-toggles) count once.
        let mut sets: HashMap<String, HashMap<String, HashSet<String>>> = HashMap::new();
        let rows = chat.reactions().map(|q| q.get()).unwrap_or_default();
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
        }
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
                    // So is the reader: whose message a row is decides its shape.
                    format!(
                        "{}|{}{}{}|{}",
                        row.message.id().to_base64(),
                        row.first_in_group as u8,
                        row.last_in_group as u8,
                        row.day_label.is_some() as u8,
                        row.viewer.as_deref().unwrap_or("")
                    )
                }
                children={
                    move |row: RowCtx| {
                        let viewer = row.viewer.clone();
                        // This row's slice of the shared author map, as a
                        // signal: resolution landing (or the session moving)
                        // re-renders exactly the rows it names.
                        let author = {
                            let author_id = row.message.user().map(|r| r.id().to_base64()).unwrap_or_default();
                            Signal::derive(move || authors.with(|m| m.get(&author_id).cloned().flatten()))
                        };
                        view! {
                            <MessageRow
                                message=row.message
                                current_user_id=viewer
                                author=author
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
