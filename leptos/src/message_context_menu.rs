use leptos::ev::MouseEvent as LeptosMouseEvent;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use wasm_bindgen::JsCast;
use web_sys::{KeyboardEvent, MouseEvent, window};

use ankurah_chat_model::MessageView;

use crate::context::chat;

/// The message actions menu: react and reply for everyone, edit for the author
/// (or for anyone, on a message its author has opened up), the author's
/// open-it-up toggle, and delete. Opens on right-click or from the row's "⋯"
/// trigger, on any message that is not already a tombstone.
#[component]
pub fn MessageContextMenu(
    x: i32,
    y: i32,
    message: MessageView,
    editing_message: RwSignal<Option<MessageView>>,
    /// The composer's reply state; Reply arms it with this message.
    replying_to: RwSignal<Option<MessageView>>,
    /// Whether the message belongs to the viewer (gates Edit; Delete also
    /// opens to moderators).
    is_own: bool,
    on_close: impl Fn() + Clone + 'static,
) -> impl IntoView {
    // Removing someone ELSE's message is the host's affair, so the item is
    // offered only where the host said what it means (see
    // `ChatHooks::moderator_delete`). Gating here is presentation: the server's
    // write policy is what actually decides.
    // Taken once, here: the handlers below run with no reactive owner, and
    // two of them defer into a future on top of that.
    let chat = chat();
    let can_moderate_delete = !is_own && chat.can_moderate() && chat.hooks().moderator_delete.is_some();
    let can_delete = is_own || can_moderate_delete;
    // Whether the author has opened this message up for anyone to edit. The
    // menu mounts fresh on every open, so a non-reactive read is correct.
    let is_collaborative = message.collaborative().ok().flatten().unwrap_or(false);
    // A typical message write scope (`user = $jwt.sub OR collaborative = true`)
    // already permits non-author edits of an opened-up message; this only
    // surfaces what the server allows.
    let can_edit = is_own || is_collaborative;
    let menu_ref = NodeRef::<leptos::html::Div>::new();
    let position = RwSignal::new((x, y));

    // Adjust position to prevent menu from going off-screen
    Effect::new({
        let menu_ref = menu_ref.clone();
        move |_| {
            if let Some(menu_el) = menu_ref.get() {
                let rect = menu_el.unchecked_ref::<web_sys::Element>().get_bounding_client_rect();
                let Some(win) = window() else { return };
                let win_width = win.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1024.0) as i32;
                let win_height = win.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(768.0) as i32;

                let mut adjusted_x = x;
                let mut adjusted_y = y;

                // Check right edge
                if x + rect.width() as i32 > win_width {
                    adjusted_x = win_width - rect.width() as i32 - 10;
                }

                // Check bottom edge
                if y + rect.height() as i32 > win_height {
                    adjusted_y = win_height - rect.height() as i32 - 10;
                }

                // Check left edge
                if adjusted_x < 10 {
                    adjusted_x = 10;
                }

                // Check top edge
                if adjusted_y < 10 {
                    adjusted_y = 10;
                }

                position.set((adjusted_x, adjusted_y));
            }
        }
    });

    // Outside-click + Escape dismiss. Registered once at mount and removed on
    // unmount, so repeated menu opens never accumulate document listeners.
    let click_closure = wasm_bindgen::closure::Closure::wrap(Box::new({
        let on_close = on_close.clone();
        let menu_ref = menu_ref.clone();
        move |e: MouseEvent| {
            if let Some(menu_el) = menu_ref.get_untracked() {
                if let Some(target) = e.target() {
                    if let Ok(target_el) = target.dyn_into::<web_sys::Node>() {
                        if !menu_el.contains(Some(&target_el)) {
                            on_close();
                        }
                    }
                }
            }
        }
    }) as Box<dyn FnMut(_)>);
    let key_closure = wasm_bindgen::closure::Closure::wrap(Box::new({
        let on_close = on_close.clone();
        move |e: KeyboardEvent| {
            if e.key() == "Escape" {
                // Consumed: the header's window-level Escape (panel manager)
                // skips defaultPrevented events, so only this menu closes.
                e.prevent_default();
                on_close();
            }
        }
    }) as Box<dyn FnMut(_)>);
    if let Some(doc) = window().and_then(|w| w.document()) {
        let _ = doc.add_event_listener_with_callback("mousedown", click_closure.as_ref().unchecked_ref());
        let _ = doc.add_event_listener_with_callback("keydown", key_closure.as_ref().unchecked_ref());
    }
    let dismiss_closures = SendWrapper::new((click_closure, key_closure));
    on_cleanup(move || {
        let (click_closure, key_closure) = dismiss_closures.take();
        if let Some(doc) = window().and_then(|w| w.document()) {
            let _ = doc.remove_event_listener_with_callback("mousedown", click_closure.as_ref().unchecked_ref());
            let _ = doc.remove_event_listener_with_callback("keydown", key_closure.as_ref().unchecked_ref());
        }
    });

    // Focus the first item when the menu opens, so arrow keys work immediately
    // (mouse users see no ring — the global outline is :focus-visible only).
    let focused_once = StoredValue::new(false);
    Effect::new({
        let menu_ref = menu_ref.clone();
        move |_| {
            if focused_once.get_value() {
                return;
            }
            if let Some(menu_el) = menu_ref.get() {
                if let Ok(Some(node)) = menu_el.query_selector("[role='menuitem']") {
                    if let Ok(el) = node.dyn_into::<web_sys::HtmlElement>() {
                        let _ = el.focus();
                        focused_once.set_value(true);
                    }
                }
            }
        }
    });

    // Menu keyboard contract: arrows cycle items, Home/End jump,
    // Enter/Space activate (native button behavior), Tab closes. Escape is
    // handled by the document-level listener above.
    let handle_menu_keydown = {
        let on_close = on_close.clone();
        let menu_ref = menu_ref.clone();
        move |e: KeyboardEvent| {
            let key = e.key();
            if key == "Tab" {
                e.prevent_default();
                on_close();
                return;
            }
            if !matches!(key.as_str(), "ArrowDown" | "ArrowUp" | "Home" | "End") {
                return;
            }
            e.prevent_default();
            let Some(menu_el) = menu_ref.get_untracked() else { return };
            let Ok(items) = menu_el.query_selector_all("[role='menuitem']") else { return };
            let n = items.length();
            if n == 0 {
                return;
            }
            let active = window().and_then(|w| w.document()).and_then(|d| d.active_element());
            let current = (0..n).find(|i| {
                items
                    .item(*i)
                    .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
                    .as_ref()
                    .map(|el| Some(el) == active.as_ref())
                    .unwrap_or(false)
            });
            let next = match key.as_str() {
                "Home" => 0,
                "End" => n - 1,
                "ArrowDown" => current.map(|c| (c + 1) % n).unwrap_or(0),
                _ => current.map(|c| (c + n - 1) % n).unwrap_or(n - 1),
            };
            if let Some(el) = items.item(next).and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok()) {
                let _ = el.focus();
            }
        }
    };

    let handle_edit = {
        let on_close = on_close.clone();
        let message = message.clone();
        move |_: LeptosMouseEvent| {
            editing_message.set(Some(message.clone()));
            on_close();
        }
    };

    // Reply: arm the composer's reply chip with this message. Cancels
    // any in-progress edit — a reply composes a NEW message — while the
    // draft text itself survives (the chip is state beside the text, not
    // text injected into it).
    let handle_reply = {
        let on_close = on_close.clone();
        let message = message.clone();
        move |_: LeptosMouseEvent| {
            replying_to.set(Some(message.clone()));
            editing_message.set(None);
            on_close();
        }
    };

    // The author's open-it-up toggle: flips `collaborative` between Some(true)
    // and Some(false). Only the author sees it — a typical write scope would
    // deny a non-author flipping it off anyway, since the post-write state
    // must still satisfy `user = $jwt.sub OR collaborative = true`.
    let handle_toggle_collab = {
        let on_close = on_close.clone();
        let message = message.clone();
        let chat = chat.clone();
        move |_: LeptosMouseEvent| {
            // A write like any other: it needs an author and a context, taken
            // together and before the future below defers.
            let Some(session) = chat.write_session() else { return };
            let message = message.clone();
            let on_close = on_close.clone();
            let make_collaborative = !is_collaborative;
            wasm_bindgen_futures::spawn_local(async move {
                match (|| async {
                    let trx = session.context.begin();
                    message.edit(&trx)?.collaborative().set(&Some(make_collaborative))?;
                    trx.commit().await?;
                    Ok::<_, Box<dyn std::error::Error>>(())
                })()
                .await
                {
                    Ok(_) => {}
                    Err(e) => tracing::error!("Failed to toggle collaborative editing: {}", e),
                }
                on_close();
            });
        }
    };

    // Clones for the quick-reaction row in the view below (handle_delete
    // consumes the originals).
    let message_for_react = message.clone();
    let on_close_for_react = on_close.clone();
    let chat_for_react = chat.clone();
    let message_for_menu = message.clone();
    let on_close_for_extras = on_close.clone();

    // Deleting an author's own message: tombstone the row and clear its text.
    // The row stays so the timeline keeps its shape; the words do not, because
    // a reader who deletes a message means the words.
    //
    // Deleting SOMEONE ELSE'S is handed to the host untouched, along with a
    // way to close this menu — a deployment may want a confirmation, a public
    // log row written in the same transaction, or a different check than the
    // one that put the item on screen.
    let handle_delete = {
        let chat = chat.clone();
        move |_: LeptosMouseEvent| {
        let message = message.clone();
        let on_close = on_close.clone();

        if !is_own {
            let close: Box<dyn Fn()> = Box::new({
                let on_close = on_close.clone();
                move || on_close()
            });
            if let Some(delete) = chat.hooks().moderator_delete.as_ref() {
                delete(message, close);
            }
            return;
        }

        let Some(session) = chat.write_session() else { return };
        wasm_bindgen_futures::spawn_local(async move {
            match (|| async {
                let trx = session.context.begin();
                let mutable = message.edit(&trx)?;
                mutable.deleted().set(&true)?;
                mutable.text().replace("")?;
                trx.commit().await?;
                Ok::<_, Box<dyn std::error::Error>>(())
            })()
            .await
            {
                Ok(_) => tracing::info!("Message deleted"),
                Err(e) => tracing::error!("Failed to delete message: {}", e),
            }
            on_close();
        });
        }
    };

    view! {
        <div
            node_ref=menu_ref
            class="contextMenu"
            role="menu"
            aria-label="Message actions"
            style:position="fixed"
            style:left=move || format!("{}px", position.get().0)
            style:top=move || format!("{}px", position.get().1)
            on:keydown=handle_menu_keydown
        >
            // Quick reactions: the fixed set, for every viewer. Always
            // `withItems`: Reply is offered on every non-tombstone row.
            <div class="contextMenuReactions withItems" role="none">
                {crate::reactions::REACTION_EMOJIS
                    .iter()
                    .map(|emoji| {
                        let on_close = on_close_for_react.clone();
                        let message = message_for_react.clone();
                        let chat = chat_for_react.clone();
                        view! {
                            <button
                                class="contextMenuEmoji"
                                role="menuitem"
                                aria-label=format!("React with {}", emoji)
                                on:click=move |_| {
                                    crate::reactions::toggle_reaction(&chat, &message, emoji);
                                    on_close();
                                }
                            >
                                {*emoji}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
            <button class="contextMenuItem" role="menuitem" on:click=handle_reply>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                    stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <polyline points="9 14 4 9 9 4" />
                    <path d="M20 20v-7a4 4 0 0 0-4-4H4" />
                </svg>
                "Reply"
            </button>
            {can_edit
                .then(|| {
                    view! {
                        <button class="contextMenuItem" role="menuitem" on:click=handle_edit>
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                                stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                                <path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z" />
                            </svg>
                            "Edit message"
                        </button>
                    }
                })}
            {is_own
                .then(|| {
                    view! {
                        <button class="contextMenuItem" role="menuitem" on:click=handle_toggle_collab>
                            {if is_collaborative {
                                view! {
                                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                                        stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                                        <rect x="3" y="11" width="18" height="11" rx="2" />
                                        <path d="M7 11V7a5 5 0 0 1 10 0v4" />
                                    </svg>
                                }
                                    .into_any()
                            } else {
                                view! {
                                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                                        stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                                        <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
                                        <circle cx="9" cy="7" r="4" />
                                        <path d="M22 21v-2a4 4 0 0 0-3-3.87" />
                                        <path d="M16 3.13a4 4 0 0 1 0 7.75" />
                                    </svg>
                                }
                                    .into_any()
                            }}
                            {if is_collaborative { "Make private again" } else { "Allow others to edit" }}
                        </button>
                    }
                })}
            {can_delete
                .then(|| {
                    view! {
                        <button class="contextMenuItem contextMenuItemDanger" role="menuitem" on:click=handle_delete>
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                                stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                                <path d="M3 6h18" />
                                <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" />
                                <path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                            </svg>
                            {if is_own { "Delete" } else { "Delete (moderator)" }}
                        </button>
                    }
                })}
            // Whatever the host puts on a message. Last, after the actions the
            // components own, and inside the same menu so the arrow keys reach
            // it — which is the whole point of the slot (see `MenuActions`).
            {
                let message = message_for_menu.clone();
                let on_close = on_close_for_extras.clone();
                chat.hooks().menu_actions.as_ref().map(move |render| {
                    let close: Box<dyn Fn()> = Box::new(move || on_close());
                    render(message, close)
                })
            }
        </div>
    }
}
