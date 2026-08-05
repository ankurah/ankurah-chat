//! The list of rooms, and the affordance for making a new one.
//!
//! Which rooms exist is the host's to decide — it hands in the LiveQuery, so a
//! page can offer every room, three of them, or one. Which room is SELECTED is
//! the host's too: this component writes the signal on a click and reads it to
//! draw the highlight, and takes no view on what should be selected first or
//! on whether that belongs in the page's URL.
//!
//! Rendered as two sibling elements — a header and a scrolling list — rather
//! than one wrapper, so a host can put them inside whatever rail it already
//! has, beside a [`crate::DmSidebar`] or alone.

use leptos::prelude::*;
use web_sys::KeyboardEvent;

use ankurah::model::Mutable;
use ankurah::EntityId;
use ankurah_chat_model::{Room, RoomView};
use ankurah_signals::Get as AnkurahGet;

use crate::context::chat;

#[component]
pub fn RoomSelector(
    /// Which room is open, by id.
    selected_room: RwSignal<Option<EntityId>>,
    /// Whether the rooms surface is the one the reader is looking at. A host
    /// that also mounts a DM panel passes "no conversation is open" here, so
    /// only one rail row can look selected at a time — a sidebar must not
    /// claim the reader is in two places at once.
    #[prop(optional, into)]
    active: Option<Signal<bool>>,
    /// Called with the room just selected. A host with other surfaces uses
    /// this to close them.
    #[prop(optional)]
    on_select: Option<Callback<EntityId>>,
) -> impl IntoView {
    let is_creating = RwSignal::new(false);
    // Making a room is a write, so the affordance is for a reader who is
    // signed in. Read TRACKED, so signing in mid-visit makes it appear.
    let chat = chat();
    let can_create = {
        let chat = chat.clone();
        move || chat.viewer().is_some()
    };

    view! {
        <div class="ankurah-chat sidebarHeader">
            <span class="sidebarTitle">"Rooms"</span>
            <Show when=can_create>
                <button class="createRoomButton" on:click=move |_| is_creating.set(true) title="Create new room">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"
                        stroke-linecap="round" aria-hidden="true">
                        <path d="M12 5v14" />
                        <path d="M5 12h14" />
                    </svg>
                </button>
            </Show>
        </div>

        <div class="ankurah-chat roomList">
            <Show when=move || is_creating.get()>
                <NewRoomInput selected_room=selected_room on_cancel=move || is_creating.set(false) />
            </Show>

            <Show when={
                let chat = chat.clone();
                move || chat.rooms().map(|q| q.get().is_empty()).unwrap_or(true)
            }>
                <div class="emptyRooms">"No rooms yet — press + to plant one."</div>
            </Show>

            <RoomListUl selected_room active on_select />
        </div>
    }
}

#[component]
fn RoomListUl(
    selected_room: RwSignal<Option<EntityId>>,
    active: Option<Signal<bool>>,
    on_select: Option<Callback<EntityId>>,
) -> impl IntoView {
    let chat = chat();
    view! {
        <For
            each=move || chat.rooms().map(|q| q.get()).unwrap_or_default()
            key=|room: &RoomView| room.id()
            children=move |room: RoomView| {
                view! { <RoomItem room=room selected_room=selected_room active=active on_select=on_select /> }
            }
        />
    }
}

#[component]
fn RoomItem(
    room: RoomView,
    selected_room: RwSignal<Option<EntityId>>,
    active: Option<Signal<bool>>,
    on_select: Option<Callback<EntityId>>,
) -> impl IntoView {
    let chat = chat();
    let room_id = room.id();
    let room_id_badge = room_id.to_base64();
    let name = room.name().unwrap_or_default();

    let is_selected =
        move || active.map(|a| a.get()).unwrap_or(true) && selected_room.get() == Some(room_id);

    view! {
        <div
            class=move || if is_selected() { "roomItem selected" } else { "roomItem" }
            on:click=move |_| {
                selected_room.set(Some(room_id));
                if let Some(on_select) = on_select {
                    on_select.run(room_id);
                }
            }
        >
            <span class="roomHash" aria-hidden="true">"#"</span>
            <span class="roomLabel">{name}</span>
            {
                move || {
                    // Reactive read: re-renders as messages arrive, or as the
                    // reader's persistent cursor advances on any of their
                    // devices.
                    let unread_count = chat.room_cursors().map(|c| c.unread_count(&room_id_badge)).unwrap_or(0);
                    (unread_count > 0).then(|| {
                        let badge_text = if unread_count >= 10 { "10+".to_string() } else { unread_count.to_string() };
                        view! { <span class="unreadBadge">{badge_text}</span> }
                    })
                }
            }
        </div>
    }
}

#[component]
fn NewRoomInput(selected_room: RwSignal<Option<EntityId>>, on_cancel: impl Fn() + Clone + 'static) -> impl IntoView {
    let room_name = RwSignal::new(String::new());
    let chat = chat();

    let handle_key = {
        let on_cancel = on_cancel.clone();
        move |ev: KeyboardEvent| match ev.key().as_str() {
            "Enter" => {
                ev.prevent_default();
                let name = room_name.get().trim().to_string();
                if name.is_empty() {
                    return;
                }
                // Resolved before the future defers, like every other write.
                let Some(session) = chat.write_session() else { return };
                let on_cancel = on_cancel.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match (|| async {
                        let transaction = session.context.begin();
                        // `created_by` is the caller: a typical room write
                        // scope rejects anything else.
                        let room = transaction
                            .create(&Room { name, created_by: Some(session.viewer.into()), topic: None })
                            .await?
                            .read();
                        transaction.commit().await?;
                        Ok::<_, Box<dyn std::error::Error>>(room)
                    })()
                    .await
                    {
                        Ok(room) => {
                            selected_room.set(Some(room.id()));
                            on_cancel();
                        }
                        Err(e) => {
                            tracing::error!("Failed to create room: {}", e);
                        }
                    }
                });
            }
            "Escape" => {
                // Consumed, so a host's own window-level Escape handling does
                // not also fire: this press cancels the room-name input and
                // nothing else.
                ev.prevent_default();
                on_cancel();
            }
            _ => {}
        }
    };

    view! {
        <div class="createRoomInput">
            <input
                type="text"
                placeholder="Room name..."
                prop:value=move || room_name.get()
                on:input=move |ev| room_name.set(event_target_value(&ev))
                on:keydown=handle_key
                on:blur={
                    let on_cancel = on_cancel.clone();
                    move |_| {
                        // try_get: the signal may already be disposed.
                        if let Some(name) = room_name.try_get() {
                            if name.trim().is_empty() {
                                on_cancel();
                            }
                        }
                    }
                }

                autofocus
            />
        </div>
    }
}
