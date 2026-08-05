//! Emoji reactions: the chip bar under message bubbles, the toggle write
//! path, and the fixed set the pickers offer.
//!
//! Query shape: ONE `active = true` LiveQuery for the whole message list
//! (built in `message_list.rs`), grouped client-side into per-message chips.
//! `Reaction` carries no room ref, so a room-scoped predicate is
//! inexpressible; the alternative — a LiveQuery per row — would churn
//! subscriptions as the virtual scroller mounts and unmounts rows. One
//! standing subscription with an O(rows) regroup on change is the cheaper
//! steady state at the scale a chat room reaches. Revisit if `Reaction` ever
//! gains a room ref.

use leptos::prelude::*;

use ankurah_chat_model::{MessageView, Reaction, ReactionView};

use crate::context::ChatContext;

/// The fixed reaction set: a small picker, deliberately, not a full emoji
/// keyboard. The composer's `:shortcode:` completion is the wide door.
pub const REACTION_EMOJIS: [&str; 6] = ["\u{1F44D}", "\u{2764}\u{FE0F}", "\u{1F602}", "\u{1F389}", "\u{1F615}", "\u{1F440}"];

/// Stable chip ordering: picker order first, then anything else (from older
/// clients or future pickers) lexicographically.
pub fn picker_index(emoji: &str) -> usize {
    REACTION_EMOJIS.iter().position(|e| *e == emoji).unwrap_or(REACTION_EMOJIS.len())
}

/// One rendered chip: an emoji, how many distinct users reacted with it, and
/// whether the viewer is among them.
#[derive(Clone, PartialEq)]
pub struct ReactionChip {
    pub emoji: String,
    pub count: usize,
    pub mine: bool,
}

/// Toggle the viewer's reaction. Reaction rows are never deleted (ankurah
/// 0.9.0 has no entity deletion): the first toggle creates {active: true},
/// later toggles flip `active`. A one-shot fetch finds prior rows — including
/// inactive ones the live `active = true` query no longer carries. Duplicate
/// rows (concurrent first-toggles) are tolerated: all matching rows flip to
/// the opposite of "any active", and the chip grouping counts distinct users.
///
/// Reacting is a write, so an anonymous reader is sent to the host's sign-in
/// instead: there is no row to create until we know whose reaction it is. The
/// handshake comes in as an argument rather than being looked up here, because
/// this is called from click handlers and resolves nothing once the future
/// below has been deferred.
pub fn toggle_reaction(chat: &ChatContext, message: &MessageView, emoji: &str) {
    let Some(session) = chat.write_session() else { return };
    let me = session.viewer;
    let ctx = session.context;
    let message = message.clone();
    let emoji = emoji.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            let selection = crate::queries::selection(
                "message = ? AND user = ? AND emoji = ?",
                [(&message.id()).into(), (&me).into(), emoji.as_str().into()],
            )?;
            let existing = ctx.fetch::<ReactionView>(selection).await?;

            let trx = ctx.begin();
            if existing.is_empty() {
                trx.create(&Reaction {
                    message: ankurah::Ref::from(&message),
                    user: me.into(),
                    emoji: emoji.clone(),
                    active: true,
                })
                .await?;
            } else {
                let any_active = existing.iter().any(|r| r.active().unwrap_or(false));
                for row in &existing {
                    row.edit(&trx)?.active().set(&!any_active)?;
                }
            }
            trx.commit().await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        }
        .await;
        if let Err(e) = result {
            tracing::error!("Failed to toggle reaction: {}", e);
        }
    });
}

/// The chip row under a bubble. Renders nothing of its own accord when
/// `chips` is empty — the caller already gates on that, but keep it safe.
#[component]
pub fn ReactionBar(message: MessageView, #[prop(into)] chips: Signal<Vec<ReactionChip>>) -> impl IntoView {
    // Taken here, where a reactive owner exists, and cloned into every chip's
    // click handler — which has none.
    let chat = crate::context::chat();
    view! {
        <div class="reactionBar">
            {move || {
                let message = message.clone();
                let chat = chat.clone();
                chips
                    .get()
                    .into_iter()
                    .map(move |chip| {
                        let emoji = chip.emoji.clone();
                        let message = message.clone();
                        let chat = chat.clone();
                        let noun = if chip.count == 1 { "reaction" } else { "reactions" };
                        let hint = if chip.mine { "Click to remove yours" } else { "Click to react" };
                        let label = format!("{} {} {}. {}.", chip.count, chip.emoji, noun, hint);
                        view! {
                            <button
                                type="button"
                                class=if chip.mine { "reactionChip mine" } else { "reactionChip" }
                                aria-pressed=if chip.mine { "true" } else { "false" }
                                aria-label=label
                                on:click=move |_| toggle_reaction(&chat, &message, &emoji)
                            >
                                <span class="reactionEmoji" aria-hidden="true">{chip.emoji.clone()}</span>
                                <span class="reactionCount">{chip.count}</span>
                            </button>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}
