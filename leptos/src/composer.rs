use leptos::html::Textarea;
use leptos::prelude::*;
use std::collections::HashMap;
use web_sys::KeyboardEvent;

use ankurah_chat_model::mention_display::MemberDirectory;
use ankurah::EntityId;
use ankurah_chat_model::{Message, MessageView, UserView};
use ankurah_signals::{Get as AnkurahGet, Peek as AnkurahPeek};
use crate::context::chat;
use crate::fmt;

/// Where a composed message goes. Everything ABOVE the send — autosize, the
/// mention popup and its `@Name` re-encoding, `:emoji:` completion, the IME
/// guards, Enter/Shift+Enter — is identical for a room and a DM thread, so the
/// composer is one component and only the create branch differs.
///
/// Edit and reply are room affordances: they are driven by
/// `editing_message`/`replying_to`, which are `MessageView`-typed and which the
/// DM thread view never arms. A DM composer therefore renders no chips, and
/// Cmd/Ctrl+Up navigates nothing (the DM view passes an empty message list).
#[derive(Clone, Copy)]
pub enum ComposerTarget {
    /// A room, by id.
    Room(EntityId),
    /// A private conversation, by the OTHER participant's id. A conversation
    /// is keyed on its pair, not on the row that happens to represent it — see
    /// [`crate::dm`] on why a pair can end up with two rows — so the composer
    /// resolves the row to write into at send time.
    Dm { partner: EntityId },
}

/// Cap on the auto-grown composer height — roughly eight lines of text;
/// beyond it the textarea scrolls internally instead of eating the timeline.
const MAX_COMPOSER_HEIGHT: i32 = 192;

/// Fit the composer textarea to its content, up to [`MAX_COMPOSER_HEIGHT`].
/// Collapse-to-auto first so shrinking works (scrollHeight never shrinks
/// below the styled height on its own).
fn autosize(el: &web_sys::HtmlTextAreaElement) {
    // Fully qualified: leptos' `ElementExt::style` extension shadows the
    // web_sys inherent getter in this scope.
    let style = web_sys::HtmlElement::style(el);
    let _ = style.set_property("height", "auto");
    // scrollHeight covers content + padding; the element is border-box, so
    // add the border (offset − client) to avoid a 2px internal scroll.
    let border = el.offset_height() - el.client_height();
    let content = el.scroll_height() + border;
    let clamped = content.min(MAX_COMPOSER_HEIGHT);
    let _ = style.set_property("height", &format!("{clamped}px"));
    let _ = style.set_property("overflow-y", if content > MAX_COMPOSER_HEIGHT { "auto" } else { "hidden" });
}

/// At most this many candidates in the mention popup.
const MENTION_POPUP_MAX: usize = 8;

/// How far back from the caret we scan for the `@` of a mention draft.
const MENTION_SCAN_MAX: usize = 48;

/// An in-progress `@mention` being typed: the utf16 index of the `@`
/// and the query text between it and the caret.
#[derive(Clone, PartialEq)]
struct MentionDraft {
    start_utf16: usize,
    query: String,
}

/// Find the mention being typed at the caret, if any: an `@` at a word start
/// (start-of-text or after whitespace) with no whitespace between it and the
/// caret. All indices are utf16 code units — the DOM's currency — so emoji
/// and other astral text before the `@` cannot skew the math; conversion to
/// Rust strings happens per-slice via `from_utf16_lossy`.
fn current_mention_draft(el: &web_sys::HtmlTextAreaElement) -> Option<MentionDraft> {
    let caret = el.selection_start().ok().flatten()? as usize;
    let units: Vec<u16> = el.value().encode_utf16().collect();
    let caret = caret.min(units.len());
    let mut i = caret;
    while i > 0 && caret - i < MENTION_SCAN_MAX {
        let unit = units[i - 1];
        // Lone surrogate halves (pieces of emoji) are ordinary non-whitespace.
        if let Some(c) = char::from_u32(unit as u32) {
            if c.is_whitespace() {
                return None;
            }
            if c == '@' {
                let at_word_start = i == 1 || char::from_u32(units[i - 2] as u32).map(|p| p.is_whitespace()).unwrap_or(false);
                if !at_word_start {
                    return None; // e.g. the @ of an email address
                }
                return Some(MentionDraft { start_utf16: i - 1, query: String::from_utf16_lossy(&units[i..caret]) });
            }
        }
        i -= 1;
    }
    None
}

/// At most this many candidates in the emoji popup.
const EMOJI_POPUP_MAX: usize = 8;

/// How far back from the caret we scan for the `:` of a shortcode draft —
/// generous headroom over the longest table name.
const EMOJI_SCAN_MAX: usize = 32;

/// An in-progress `:shortcode` being typed: the utf16 index of the
/// opening `:` and the query text between it and the caret.
#[derive(Clone, PartialEq)]
struct EmojiDraft {
    start_utf16: usize,
    query: String,
}

/// Characters a shortcode run may contain. Uppercase is tolerated while
/// typing (matching lowercases); `+`/`-` serve `:+1:`/`:-1:`.
fn is_shortcode_char(c: char) -> bool { c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-') }

/// Find the emoji shortcode being typed at the caret, if any: a `:` at a
/// word start (start-of-text or after whitespace — so clock times like
/// "12:30" and pasted URLs stay quiet) with 2+ shortcode chars between it
/// and the caret (so a lone `:` or a `:)` smiley stays quiet too). Indices
/// are utf16 code units, like the mention draft.
fn current_emoji_draft(el: &web_sys::HtmlTextAreaElement) -> Option<EmojiDraft> {
    let caret = el.selection_start().ok().flatten()? as usize;
    let units: Vec<u16> = el.value().encode_utf16().collect();
    let caret = caret.min(units.len());
    let mut i = caret;
    while i > 0 && caret - i < EMOJI_SCAN_MAX {
        // Lone surrogate halves (pieces of emoji) fail the shortcode-char
        // test below, exactly as they should.
        let c = char::from_u32(units[i - 1] as u32)?;
        if c == ':' {
            let at_word_start = i == 1 || char::from_u32(units[i - 2] as u32).map(|p| p.is_whitespace()).unwrap_or(false);
            if !at_word_start {
                return None;
            }
            let query = String::from_utf16_lossy(&units[i..caret]);
            return (query.len() >= 2).then_some(EmojiDraft { start_utf16: i - 1, query });
        }
        if !is_shortcode_char(c) {
            return None;
        }
        i -= 1;
    }
    None
}

/// A completed `:name:` run ending exactly at `caret`: returns the utf16
/// index of the opening `:` and the name between the colons. Same word-start
/// rule as the draft scanner; an empty name (`::`) never matches.
fn completed_shortcode(units: &[u16], caret: usize) -> Option<(usize, String)> {
    if caret < 3 || caret > units.len() || units[caret - 1] != u16::from(b':') {
        return None;
    }
    let mut i = caret - 1;
    while i > 0 && caret - i < EMOJI_SCAN_MAX {
        let c = char::from_u32(units[i - 1] as u32)?;
        if c == ':' {
            let at_word_start = i == 1 || char::from_u32(units[i - 2] as u32).map(|p| p.is_whitespace()).unwrap_or(false);
            if i == caret - 1 || !at_word_start {
                return None;
            }
            return Some((i - 1, String::from_utf16_lossy(&units[i..caret - 1])));
        }
        if !is_shortcode_char(c) {
            return None;
        }
        i -= 1;
    }
    None
}

/// Whether a keypress would have CHANGED the draft or sent it.
///
/// What it is for: the anonymous reader whose caret is already in the box —
/// the one case the composer's other gates never see, because the session
/// dropped to anonymous under a focus that had already landed. `readonly` makes
/// their keys do nothing, and this is the test for which of those silences is
/// worth the host's sign-in ceremony.
///
/// Deliberately not "every key". An arrow, Home/End, Tab, Escape or a bare
/// modifier moves a caret or dismisses a chip and changes no text, and a reader
/// pressing one of those has not reached for the draft. `key()` carries the
/// character itself for a printable key and a word ("ArrowLeft", "Shift") for
/// the rest, so "exactly one scalar" is the printable test; a Ctrl or Meta
/// chord over that is a command rather than a character, and only the two that
/// move text — paste and cut — count. Alt is not excluded, because on macOS it
/// composes real characters.
fn would_write_draft(e: &KeyboardEvent) -> bool {
    let key = e.key();
    match key.as_str() {
        "Enter" | "Backspace" | "Delete" => true,
        // Paste and cut, by chord. Undo is not here: with the box readonly
        // there is no edit of the reader's for it to walk back.
        "v" | "V" | "x" | "X" if e.ctrl_key() || e.meta_key() => true,
        "Insert" if e.shift_key() => true,
        _ => key.chars().count() == 1 && !e.ctrl_key() && !e.meta_key(),
    }
}

/// Rank users for the mention popup: display-name prefix matches first, then
/// substring matches, alphabetically within each tier; at most
/// [`MENTION_POPUP_MAX`]. An empty query (bare `@`) lists everyone.
fn mention_candidates(users: &[UserView], query: &str) -> Vec<UserView> {
    let q = query.to_lowercase();
    let mut ranked: Vec<(bool, String, UserView)> = users
        .iter()
        .filter_map(|u| {
            let name = u.display_name().unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            let lower = name.to_lowercase();
            if lower.starts_with(&q) {
                Some((false, lower, u.clone()))
            } else if lower.contains(&q) {
                Some((true, lower, u.clone()))
            } else {
                None
            }
        })
        .collect();
    ranked.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    ranked.truncate(MENTION_POPUP_MAX);
    ranked.into_iter().map(|(_, _, u)| u).collect()
}

/// The message box: writing, sending, and editing.
///
/// A multiline textarea. Enter sends, Shift+Enter inserts a newline, Escape
/// cancels an edit (or an armed reply), and Cmd/Ctrl+Up/Down walks back
/// through the reader's own messages to edit one. Typing `@` opens the mention
/// autocomplete and `:` the emoji one. While a reply is armed, a
/// "Replying to …" chip sits above the input and the send attaches the
/// referenced message as `re`.
///
/// A READER WITH NO VIEWER GETS NO CARET AND WRITES NO DRAFT. A pointer press
/// on the box, a programmatic focus, or text dragged onto it raises the host's
/// auth-demand callback — once per gesture — and the focus is dropped rather
/// than kept; Tab skips the box entirely; and the box is `readonly`, so
/// nothing a keystroke, a paste, a drop or an IME composition would have
/// written reaches the draft. Every write demands as well, through
/// `write_session`. A signed-in reader meets none of it, and a host that
/// installed no callback keeps the older behaviour exactly: the box takes
/// focus, and the send is refused with a warning in the log.
///
/// THE DOWN TRANSITION. A session that drops to anonymous while the box is
/// focused keeps its caret and whatever was already typed — nothing here
/// revokes a focus that already landed — but the box stops accepting text at
/// once, and the next keystroke that would have changed or sent the draft
/// raises the ceremony rather than doing nothing.
///
/// The draft holds plain `@DisplayName` text, never raw tokens: the
/// autocomplete inserts the name, send re-encodes matching `@Name` runs to the
/// canonical `<@id>` token (see
/// [`ankurah_chat_model::mention_display::MemberDirectory`]), and the edit
/// mirror decodes stored tokens back for the textarea. What reaches the wire
/// is the token either way, which is what a server's mention scanner reads.
///
/// Mountable on its own — a page can show a composer with no timeline above it
/// — as long as it can say which room or thread the message goes to.
#[component]
pub(crate) fn WiredComposer(
    /// The room or conversation this composer posts into.
    target: ComposerTarget,
    /// The message being edited, shared with the timeline that armed it.
    editing_message: RwSignal<Option<MessageView>>,
    /// The message the next send replies to, armed by the actions menu's
    /// Reply. Independent of the draft text: arming, cancelling, or sending a
    /// reply never rewrites what the reader has typed.
    replying_to: RwSignal<Option<MessageView>>,
    /// Current visible messages (oldest-first), for Cmd/Ctrl+Up/Down navigation.
    #[prop(into)]
    messages: Signal<Vec<MessageView>>,
) -> impl IntoView {
    let message_input = RwSignal::new(String::new());
    let textarea_ref = NodeRef::<Textarea>::new();

    // Taken once, in the body, and cloned into everything below. Not because a
    // handler could not resolve it — tachys re-enters the owner it captured at
    // attach, so a click handler can — but because the send path DEFERS, and a
    // future's first poll is a microtask with no owner at all. Hoisting makes
    // every closure here owner-independent by construction instead of by
    // auditing which of them happen to run where, and spares a context walk per
    // call.
    let chat = chat();

    // Whether the host says the transport is up. A host with nothing to report
    // says nothing and the composer stays enabled.
    let is_connected = {
        let chat = chat.clone();
        move || chat.online()
    };
    let can_send = {
        let is_connected = is_connected.clone();
        move || !message_input.get().trim().is_empty() && is_connected()
    };

    // WHETHER REACHING FOR THIS BOX RAISES THE HOST'S SIGN-IN CEREMONY INSTEAD
    // OF OPENING A CARET.
    //
    // A reader with no viewer is given the ceremony the moment they reach for
    // the box, and no text of theirs ever reaches the draft — not by caret, not
    // by drag-and-drop, not by paste. They learn what is needed before
    // composing rather than after.
    //
    // Both halves of the test are load-bearing. The viewer is read through the
    // TRACKED accessor, and it is read from an ATTRIBUTE closure below as well
    // as from the listeners, so a reader who signs in mid-visit — the host sets
    // its session signal and nothing remounts — gets the tab stop back and a
    // live message box with the same DOM node, the same node_ref and the same
    // listeners. And a host that installed no auth-demand callback keeps
    // exactly the behaviour it had before this existed: taking the caret away
    // with no ceremony to put in its place is a dead end, so the whole gate
    // stands down and the send path refuses as it always did.
    //
    // THE DOWN TRANSITION KEEPS ITS CARET BUT NOT ITS KEYBOARD. A session that
    // drops to anonymous while the box is focused does not have that focus
    // revoked — nothing here reaches back and blurs a caret that already
    // landed. What it does lose immediately is mutability: `readonly` goes on
    // in the same tick the attribute recomputes, so the next keystroke, paste,
    // drop or composition update changes nothing, and the keydown gate below
    // turns that into the ceremony rather than into silence. Whatever was
    // already typed stays in the draft, and the send refuses through
    // `write_session` as it always has.
    let demand_instead_of_caret = {
        let chat = chat.clone();
        move || !chat.is_authenticated() && chat.can_demand_auth()
    };

    // ONE DEMAND PER GESTURE, AND NO TEXT BY ANY ROUTE.
    //
    // Two attributes and six listeners, because a textarea can take focus and
    // take text by more than one route, and the demand must go up exactly once
    // per gesture whichever route the reader used:
    //
    //   readonly     — THE MUTABILITY BOUNDARY, and the only part of this that
    //                  is not an event. Nothing writes into a readonly
    //                  textarea: not a keystroke, not a paste, not a drop, not
    //                  an IME composition, not the browser's own autofill. It
    //                  is here because `beforeinput` is NOT a boundary — Input
    //                  Events Level 2 makes `insertCompositionText`
    //                  non-cancelable, so a member mid-composition whose
    //                  session drops to anonymous would otherwise have the next
    //                  composition update written and filed into the draft.
    //   tabindex     — `-1` while anonymous, so sequential focus navigation
    //                  SKIPS the box entirely. Tab flows past it to the next
    //                  control instead of landing on something that would
    //                  immediately blur — which in Chrome and Firefox drops to
    //                  the document body and restarts forward Tab at the top.
    //                  Absent (not `0`) for a signed-in reader, so their markup
    //                  is untouched; `readonly` is likewise absent rather than
    //                  false, a bool attribute rendering bare or not at all.
    //   pointerdown  — COUNTS this pointer gesture, and nothing else. It is the
    //                  first event of any tap or click.
    //   mousedown    — prevent_default for EVERY button, so the caret never
    //                  lands (the same trick the autocomplete popups below use
    //                  to KEEP focus, pointed the other way), then demands for
    //                  the primary button only. A right-press gets no ceremony
    //                  alongside its context menu.
    //   focus        — the belt: a programmatic focus, and any engine where the
    //                  prevent_default above did not stop focus. It blurs FIRST
    //                  and demands after, so the host's popup opens with
    //                  nothing focused and there is no message box to restore
    //                  focus to when it closes.
    //   keydown      — for the one case where a caret is already in the box and
    //                  nothing above ever fires: a session that DROPS to
    //                  anonymous under a focused composer. `readonly` makes the
    //                  keys do nothing, and this turns that silence into the
    //                  ceremony. Only for keys that would have changed or sent
    //                  the draft — see `would_write_draft`; moving a caret or
    //                  pressing Escape is not a reach for the box.
    //   beforeinput  — prevent_default, as the belt for every CANCELABLE
    //                  insertion route (paste, drop, a plain typed character)
    //                  should `readonly` ever be lifted or unsupported. Not the
    //                  boundary, per the readonly entry above.
    //   drop         — prevent_default as well, for engines whose drop
    //                  insertion raises no cancelable beforeinput, and it
    //                  demands: a reader who drags text at the box has reached
    //                  for it as plainly as one who clicks. beforeinput does
    //                  NOT demand — it can arrive in bursts during composition,
    //                  and every route to it is answered elsewhere here.
    //
    // WHY THE LISTENERS FIRE AT ALL, in both worlds. tachys delegates a
    // listener only when the event bubbles AND `cfg!(feature = "delegation")`
    // (tachys/src/html/event.rs:214). That feature is not on by default and is
    // not enabled anywhere in this workspace — tachys resolves here as
    // `default,oco,reactive_graph,reactive_stores,testing` — so every listener
    // below attaches DIRECTLY to the textarea. A host that turns
    // `leptos/delegation` on instead routes EVERY bubbling one of them
    // (pointerdown, mousedown, keydown, beforeinput, drop) through a single
    // window-level bubble-phase listener that walks up from the target and
    // invokes each node's handler with the SAME native event
    // (tachys/src/renderer/dom.rs:367). prevent_default holds in both worlds,
    // because a default action runs after the whole dispatch rather than after
    // the target phase. What differs is REACH: in the delegated world an
    // ancestor of the composer that calls stopPropagation on any of those five
    // keeps the event from ever arriving at the window, and that handler does
    // not run. Two things survive that. `focus` is non-bubbling in tachys'
    // table, so it attaches directly either way. And `readonly` is an
    // ATTRIBUTE, not an event — no ancestor can stop it — which is the second
    // reason the mutability boundary is not made of listeners.
    //
    // THE LATCH RULE, exactly. Three per-mount counters, and no clock:
    //
    //   `gesture_seq`    — how many pointer gestures this box has seen. Each
    //                      pointerdown bumps it.
    //   `last_press_seq` — the `gesture_seq` recorded by the most recent
    //                      primary press, or None before the first one.
    //   `demanded_in`    — the `gesture_seq` as of the moment the outstanding
    //                      demand was raised, or None while none is
    //                      outstanding. `demand_once` does nothing when it is
    //                      Some; otherwise it records the current seq and calls
    //                      the host.
    //
    // The latch is cleared by exactly one thing: a PRIMARY, `detail() <= 1`
    // mousedown, and only when the outstanding demand does not already belong
    // to the press being handled. In order, that press —
    //
    //   1. returns immediately if `detail() > 1`, BEFORE any bookkeeping. The
    //      second and third press of a multi-click continue the gesture the
    //      first one started, so one double-click gets one ceremony. The click
    //      count is read here and not from the pointerdown, because a pointer
    //      event's `detail` is 0 and the test there would be dead code.
    //   2. SELF-BUMPS `gesture_seq` when it equals `last_press_seq` — no
    //      pointerdown has been seen since the previous press. Every real
    //      gesture has exactly one press, so a press with no fresh pointerdown
    //      IS a new gesture. This is the delegation-enabled world where a host
    //      ancestor stops pointerdown alone: without the self-bump the counter
    //      would freeze and every later click would be swallowed.
    //   3. clears the latch UNLESS no completed press has answered the
    //      outstanding demand yet — `last_press_seq` is None, or it is older
    //      than the seq the demand was raised in. That is the focus-before-
    //      press order, in both its shapes: a tap whose focus arrives before
    //      the compatibility mousedown, and a host ancestor whose own
    //      pointerdown handler calls `textarea.focus()` mid-propagation —
    //      including the delegated world, where that focus demands BEFORE our
    //      pointerdown handler has run at the window, so the demand is recorded
    //      one seq early. Testing against the last PRESS rather than against
    //      the current seq is what makes that early recording harmless.
    //   4. records itself in `last_press_seq`, and demands.
    //
    // TRACED, all of it. SINGLE CLICK: pointerdown bumps to 1; the press has no
    // demand outstanding, so it clears nothing and demands at 1. DOUBLE AND
    // TRIPLE CLICK: presses two and three return at step 1, so still one
    // ceremony — and the next single click demands again, because by then a
    // completed press stands behind the outstanding demand. TAP OR PEN whose
    // focus precedes the compatibility mousedown: focus demands at 1, the press
    // finds `last_press_seq` None and suppresses — one ceremony. ANCESTOR
    // FOCUSES DURING POINTERDOWN, listeners direct: same as the tap. The same,
    // DELEGATED: the focus demands at seq 0 because our pointerdown handler has
    // not run yet; the press still finds no completed press behind that demand
    // and suppresses — one ceremony. ANCESTOR STOPS POINTERDOWN ONLY: the press
    // self-bumps, and every click demands. CEREMONY REFOCUS, after any length
    // of time: a focus with no press at all, so `demand_once` finds the latch
    // up and raises ZERO — which is why the rule counts gestures instead of
    // measuring time, and why no coarse or frozen clock can affect it. A FOCUS
    // WITH NO PRESS EVER BEHIND IT (a host focusing the box from its own code)
    // demands once and then latches; a second one is silent until the reader
    // presses, and the host callback's documented idempotence is what makes the
    // first one safe to repeat when a press does arrive.
    //
    // Per-mount state, so a host that keys its subtree on `chat.generation()`
    // gets a fresh latch along with the fresh composer.
    let gesture_seq = StoredValue::new(0u64);
    let last_press_seq = StoredValue::new(None::<u64>);
    let demanded_in = StoredValue::new(None::<u64>);
    let demand_once = {
        let chat = chat.clone();
        move || {
            if demanded_in.get_value().is_some() {
                return;
            }
            demanded_in.set_value(Some(gesture_seq.get_value()));
            chat.demand_auth();
        }
    };
    // The crate's own deliberate gestures re-arm the latch before they demand:
    // a drop, and the reply-arming focus further down. Each is a reader acting
    // on purpose, and neither can be the second route of some other gesture —
    // unlike the focus a closing ceremony hands back, which clears nothing and
    // therefore stays quiet.
    let rearm = move || demanded_in.set_value(None);
    let anonymous_readonly = demand_instead_of_caret.clone();
    let anonymous_tabindex = {
        let demand_instead_of_caret = demand_instead_of_caret.clone();
        move || demand_instead_of_caret().then_some("-1")
    };
    let mark_gesture = {
        let demand_instead_of_caret = demand_instead_of_caret.clone();
        move |_: leptos::ev::PointerEvent| {
            if demand_instead_of_caret() {
                gesture_seq.update_value(|seq| *seq += 1);
            }
        }
    };
    let refuse_caret = {
        let demand_instead_of_caret = demand_instead_of_caret.clone();
        let demand_once = demand_once.clone();
        move |e: leptos::ev::MouseEvent| {
            if !demand_instead_of_caret() {
                return;
            }
            // Every button: a right-press must not land a caret either. Denying
            // the caret BEFORE demanding is the deliberate order — a host that
            // signs the reader in synchronously from its callback still gets no
            // caret out of THIS click, and the next click types. Refusing and
            // being overtaken costs one click; the other order lands a caret on
            // a reader who has none.
            e.prevent_default();
            if e.button() != 0 {
                return;
            }
            // (1) A multi-click continues the gesture its first press started.
            if e.detail() > 1 {
                return;
            }
            // (2) No pointerdown since the last press: this press IS the
            // gesture.
            if last_press_seq.get_value() == Some(gesture_seq.get_value()) {
                gesture_seq.update_value(|seq| *seq += 1);
            }
            // (3) Has a completed press already answered the demand in hand?
            let answered_by_this_press = match demanded_in.get_value() {
                None => false,
                Some(raised_in) => last_press_seq.get_value().is_none_or(|pressed| pressed < raised_in),
            };
            // (4)
            last_press_seq.set_value(Some(gesture_seq.get_value()));
            if !answered_by_this_press {
                rearm();
            }
            demand_once();
        }
    };
    let demand_on_focus = {
        let demand_instead_of_caret = demand_instead_of_caret.clone();
        let demand_once = demand_once.clone();
        move |_: leptos::ev::FocusEvent| {
            if !demand_instead_of_caret() {
                return;
            }
            if let Some(el) = textarea_ref.get_untracked() {
                let _ = el.blur();
            }
            // WHAT STAYS SWALLOWED HERE, deliberately: a focus that raises
            // nothing because a demand is already outstanding and nothing
            // re-armed the latch for it. The crate re-arms for its OWN
            // reply-arming focus, because it knows that one came from a
            // reader's click on Reply. It cannot know that of a screen reader's
            // browse-mode activation, or of a host focusing the box from its
            // own code, so a second one of those after a dismissed ceremony is
            // silent until the reader presses on the box.
            demand_once();
        }
    };
    let refuse_insertion = {
        let demand_instead_of_caret = demand_instead_of_caret.clone();
        move |e: leptos::ev::InputEvent| {
            if demand_instead_of_caret() {
                e.prevent_default();
            }
        }
    };
    let refuse_drop = {
        let demand_instead_of_caret = demand_instead_of_caret.clone();
        let demand_once = demand_once.clone();
        move |e: leptos::ev::DragEvent| {
            if !demand_instead_of_caret() {
                return;
            }
            e.prevent_default();
            // A drop is its own gesture — the press that began it landed on the
            // drag source, never here — so it can never be the second route of
            // a gesture this latch is already holding. Re-arm, or a second drop
            // after a dismissed ceremony would be swallowed.
            rearm();
            demand_once();
        }
    };

    // Mention autocomplete draws on the handshake's members query — the same
    // rows the timeline names authors from, so a rename shows up in both at
    // once, and one subscription rather than one per composer.
    let members_now = {
        let chat = chat.clone();
        move || {
            chat.members()
                .map(|q| q.peek().iter().map(|u| (u.id().to_base64(), u.display_name().unwrap_or_default())).collect::<Vec<_>>())
                .unwrap_or_default()
        }
    };
    let mention_draft = RwSignal::new(None::<MentionDraft>);
    let mention_selected = RwSignal::new(0usize);
    let mention_matches = Signal::derive({
        let chat = chat.clone();
        move || match mention_draft.get() {
            Some(draft) => match chat.members() {
                Some(members) => mention_candidates(&members.get(), &draft.query),
                None => Vec::new(),
            },
            None => Vec::new(),
        }
    });

    // Emoji autocomplete: the same draft/selection/matches trio as
    // mentions, over the static shortcode table.
    let emoji_draft = RwSignal::new(None::<EmojiDraft>);
    let emoji_selected = RwSignal::new(0usize);
    let emoji_matches = Signal::derive(move || match emoji_draft.get() {
        Some(draft) => crate::emoji::candidates(&draft.query, EMOJI_POPUP_MAX),
        None => Vec::new(),
    });

    // Which member an autocompleted name meant: name → id, recorded at
    // pick time so an ambiguous display name re-encodes to the member the
    // user actually chose. Session-scoped; a name never picked falls back to
    // the directory's deterministic choice.
    let mention_picks = StoredValue::new(HashMap::<String, String>::new());

    // The member list as a coding directory. Untracked on purpose: a send or
    // an edit-mirror wants the list as of NOW, and must not re-run when
    // someone joins or renames.
    let directory = {
        let members_now = members_now.clone();
        move || MemberDirectory::new(members_now())
    };

    // id → display-name map for the reply chip: the author line, and token
    // resolution inside the snippet. Rebuilt live — renames included — from
    // the same users query the mention popup holds.
    let member_names = Memo::new({
        let chat = chat.clone();
        move |_| {
            chat.members()
                .map(|q| {
                    q.get()
                        .iter()
                        .filter_map(|u| {
                            let name = u.display_name().unwrap_or_default();
                            (!name.is_empty()).then(|| (u.id().to_base64(), name))
                        })
                        .collect::<HashMap<String, String>>()
                })
                .unwrap_or_default()
        }
    });

    // Re-derive both drafts from the caret. Cheap; called on input and on
    // caret-moving keys/clicks so an anchor can never go stale silently.
    // The two can never be Some at once: a mention draft tolerates no
    // whitespace back to its `@`, an emoji draft no non-shortcode char back
    // to its `:`, and each trigger disqualifies the other's scan.
    let refresh_drafts = move || {
        let Some(el) = textarea_ref.get_untracked() else { return };
        let next = current_mention_draft(&el);
        if next != mention_draft.get_untracked() {
            mention_draft.set(next);
            mention_selected.set(0);
        }
        let next = current_emoji_draft(&el);
        if next != emoji_draft.get_untracked() {
            emoji_draft.set(next);
            emoji_selected.set(0);
        }
    };

    // Splice `replacement` over utf16 units [start, end) of the textarea and
    // pin the caret after it. Write the DOM first (value + caret), then the
    // signal: the render effect re-assigns .value with the identical string,
    // and some engines reset the caret to the end on any assignment, so it
    // is re-pinned a frame later for mid-text insertions.
    let splice_units = move |el: &web_sys::HtmlTextAreaElement, start: usize, end: usize, replacement: &str| {
        let units: Vec<u16> = el.value().encode_utf16().collect();
        let end = end.min(units.len());
        let rep: Vec<u16> = replacement.encode_utf16().collect();
        let mut next: Vec<u16> = Vec::with_capacity(units.len() - (end - start) + rep.len());
        next.extend_from_slice(&units[..start]);
        next.extend_from_slice(&rep);
        next.extend_from_slice(&units[end..]);
        let new_caret = (start + rep.len()) as u32;
        let new_value = String::from_utf16_lossy(&next);
        el.set_value(&new_value);
        let _ = el.set_selection_range(new_caret, new_caret);
        message_input.set(new_value);
        autosize(el);
        // The other programmatic focus in this component, and the one that does
        // NOT re-arm the anonymous latch: getting here needs a mention or emoji
        // draft, which needs typed text, which a reader with no viewer cannot
        // produce.
        let _ = el.focus();
        request_animation_frame(move || {
            if let Some(el) = textarea_ref.get_untracked() {
                let _ = el.set_selection_range(new_caret, new_caret);
            }
        });
    };

    // Replace the draft (`@que`) with plain `@DisplayName ` — the draft
    // shows names, never tokens; send re-encodes to the canonical
    // `<@BASE64_ID>` wire format. The pick is recorded so a shared display
    // name still re-encodes to the member chosen here.
    let insert_mention = move |user: &UserView| {
        let Some(el) = textarea_ref.get_untracked() else { return };
        let Some(draft) = mention_draft.get_untracked() else { return };
        let units: Vec<u16> = el.value().encode_utf16().collect();
        let caret = el.selection_start().ok().flatten().map(|c| c as usize).unwrap_or(units.len()).min(units.len());
        let start = draft.start_utf16;
        if start >= caret || units.get(start) != Some(&u16::from(b'@')) {
            mention_draft.set(None); // stale anchor: the text changed under us
            return;
        }
        let name = user.display_name().unwrap_or_default();
        if name.is_empty() {
            return; // candidates are name-filtered; belt for a race
        }
        mention_picks.update_value(|picks| {
            picks.insert(name.clone(), user.id().to_base64());
        });
        splice_units(&el, start, caret, &format!("@{name} "));
        mention_draft.set(None);
    };

    // Replace the draft (`:que`) with the chosen unicode glyph — input-time
    // replacement only; what is stored is the plain emoji.
    let insert_emoji = move |glyph: &str| {
        let Some(el) = textarea_ref.get_untracked() else { return };
        let Some(draft) = emoji_draft.get_untracked() else { return };
        let units: Vec<u16> = el.value().encode_utf16().collect();
        let caret = el.selection_start().ok().flatten().map(|c| c as usize).unwrap_or(units.len()).min(units.len());
        let start = draft.start_utf16;
        if start >= caret || units.get(start) != Some(&u16::from(b':')) {
            emoji_draft.set(None); // stale anchor: the text changed under us
            return;
        }
        splice_units(&el, start, caret, glyph);
        emoji_draft.set(None);
    };

    // Inline completion: a fully typed `:name:` becomes its glyph the
    // moment the closing colon lands — no popup interaction required. IME
    // composition updates are exempt (composing text must never be spliced
    // mid-flight); names outside the table stay as typed, per the contract.
    let complete_typed_shortcode = move |ev: &leptos::ev::Event| {
        use wasm_bindgen::JsCast;
        if ev.dyn_ref::<web_sys::InputEvent>().map(|e| e.is_composing()).unwrap_or(false) {
            return;
        }
        let Some(el) = textarea_ref.get_untracked() else { return };
        let Some(caret) = el.selection_start().ok().flatten().map(|c| c as usize) else { return };
        let units: Vec<u16> = el.value().encode_utf16().collect();
        let Some((start, name)) = completed_shortcode(&units, caret.min(units.len())) else { return };
        let Some(glyph) = crate::emoji::lookup(&name) else { return };
        splice_units(&el, start, caret, glyph);
        emoji_draft.set(None);
    };

    // Edit-session snapshot: the decoded editor text and the member set
    // the decode ran against, captured when an edit enters the composer. The
    // save's no-op check and re-encode use THIS snapshot — decode's lossless
    // guard (encode(decode(x)) == x) only holds within one directory, so a
    // member joining, leaving, or renaming while the editor is open could
    // otherwise retarget or destroy a mention on a save the user never
    // touched.
    let edit_snapshot = StoredValue::new(None::<(String, Vec<(String, String)>)>);

    // Mirror the edit target into the composer. `prev` carries the previous
    // run's editing id, so only actual transitions rewrite the draft (signal
    // re-notifications must never clobber typing). Entering an edit also
    // disarms a pending reply: send() would EDIT, not create, so a
    // lingering chip would promise a `re` that never attaches. The reverse
    // (Reply canceling an edit) is handled at the Reply action itself.
    Effect::new({
        let members_now = members_now.clone();
        move |prev: Option<Option<String>>| {
        let editing = editing_message.get();
        let editing_id = editing.as_ref().map(|m| m.id().to_base64());
        if prev.map(|p| p != editing_id).unwrap_or(true) {
            // Programmatic fill invalidates any draft anchor.
            mention_draft.set(None);
            emoji_draft.set(None);
            match &editing {
                Some(edit_msg) => {
                    replying_to.set(None);
                    // Stored tokens decode to `@names` for the textarea;
                    // tokens that cannot decode safely stay raw (see
                    // `MemberDirectory::decode`). Snapshot the result and the
                    // members it was computed against for the save side (see
                    // `edit_snapshot`).
                    let members = members_now();
                    let dir = MemberDirectory::new(members.iter().cloned());
                    let decoded = dir.decode(&edit_msg.text().unwrap_or_default());
                    message_input.set(decoded.clone());
                    edit_snapshot.set_value(Some((decoded, members)));
                }
                None => {
                    edit_snapshot.set_value(None);
                    message_input.set(String::new());
                }
            }
        }
        editing_id
        }
    });

    // Arming a reply focuses the composer — the chip itself is chrome,
    // and the user's next act is typing.
    //
    // For an anonymous reader that focus is refused and turned into the
    // ceremony, so the latch is RE-ARMED first: the crate knows this focus came
    // from a reader clicking Reply, which is a fresh deliberate gesture, and
    // without the re-arm a second Reply after a dismissed ceremony would be
    // swallowed by the demand the first one left outstanding. Re-arming
    // unconditionally is right — for a signed-in reader nothing reads the latch
    // at all, and the message row offers Reply to every reader.
    Effect::new(move |_| {
        if replying_to.get().is_some() {
            rearm();
            if let Some(el) = textarea_ref.get_untracked() {
                let _ = el.focus();
            }
        }
    });

    // Refit the textarea after every programmatic content change (edit-mirror
    // fill, clear-after-send). Effects run after the render effect has pushed
    // the new value into the DOM, so scrollHeight is current. Typing is
    // covered separately by the on:input handler for zero-lag growth.
    Effect::new(move |_| {
        let _ = message_input.get();
        if let Some(el) = textarea_ref.get_untracked() {
            autosize(&el);
        }
    });

    let send = {
        let target = target.clone();
        let chat = chat.clone();
        move || {
            mention_draft.set(None);
            emoji_draft.set(None);
            let input_text = message_input.get();
            if input_text.trim().is_empty() {
                return;
            }
            // Author and context, resolved together and before anything is
            // deferred. Nobody signed in means the host has been asked to fix
            // that, and the draft is left exactly where it is — it is still
            // there afterwards.
            let Some(session) = chat.write_session() else { return };

            if let Some(edit_msg) = editing_message.get() {
                // Edit existing message via a CRDT text replace. The editor
                // held display text; the wire gets the re-encoded form,
                // built against the EDIT-ENTRY snapshot — the same directory
                // that produced the editor text (see `edit_snapshot`), so a
                // membership change mid-edit can't shift what a save means.
                let input_text = input_text.trim().to_string();
                let (entry_decoded, members) = edit_snapshot.get_value().unwrap_or_else(|| {
                    // Defensive only: the snapshot is written by the same
                    // effect that filled the editor. Absent, the current
                    // members are the best approximation of what it shows.
                    let members = members_now();
                    let dir = MemberDirectory::new(members.iter().cloned());
                    (dir.decode(&edit_msg.text().unwrap_or_default()), members)
                });
                // The armed edit may predate a session swap: a reader who
                // signed in as somebody else must not save through the new
                // session into the old reader's message. Community never swaps
                // a session, so this is the crate's contract holding rather
                // than a case that arises there.
                let author = edit_msg.user().ok().map(|r| r.id());
                let open_to_anyone = edit_msg.collaborative().ok().flatten().unwrap_or(false);
                if !open_to_anyone && author != Some(session.viewer) {
                    tracing::warn!("abandoning an edit armed by a different reader");
                    editing_message.set(None);
                    return;
                }
                let picks = mention_picks.get_value();
                wasm_bindgen_futures::spawn_local(async move {
                    let result = async {
                        let stored = edit_msg.text().unwrap_or_default();
                        // No-op edits commit nothing (and earn no "(edited)"
                        // marker). "Unchanged" means the editor still shows
                        // exactly what the edit-entry decode produced —
                        // comparing wire forms would stamp a phantom edit
                        // whenever decode had fallen back to raw tokens, and
                        // a fresh decode would judge against a directory the
                        // user never saw.
                        if entry_decoded == input_text {
                            return Ok(());
                        }
                        let dir = MemberDirectory::new(members.into_iter());
                        let wire_text = dir.encode(&input_text, &picks);
                        if wire_text == stored {
                            return Ok(()); // byte-identical outcome (e.g. a re-typed mention)
                        }
                        let trx = session.context.begin();
                        let mutable = edit_msg.edit(&trx)?;
                        mutable.text().replace(&wire_text)?;
                        // Stamp the edit for the "(edited)" indicator.
                        mutable.edited_at().set(&Some(js_sys::Date::now() as i64))?;
                        trx.commit().await?;
                        Ok::<_, Box<dyn std::error::Error>>(())
                    }
                    .await;
                    match result {
                        Ok(_) => {
                            editing_message.set(None);
                            message_input.set(String::new());
                        }
                        Err(e) => tracing::error!("Failed to update message: {}", e),
                    }
                });
            } else {
                // Create a new message. ankurah stores user/room as typed
                // Refs; an armed reply rides along as `re`. The wire text is
                // the display draft with `@Name` runs re-encoded to canonical
                // tokens — the same bytes for a room and for a DM. What a
                // server does with a mention in each is the server's affair.
                let target = target.clone();
                let input_text = input_text.trim().to_string();
                let wire_text = directory().encode(&input_text, &mention_picks.get_value());
                let reply_to = replying_to.get_untracked();
                // Clear synchronously: clearing only in the async completion
                // left a window where a second Enter re-sent the same text.
                // The reply chip clears with it — this send owns it now.
                message_input.set(String::new());
                replying_to.set(None);
                wasm_bindgen_futures::spawn_local(async move {
                    let result = async {
                        match &target {
                            ComposerTarget::Room(room_id) => {
                                let trx = session.context.begin();
                                trx.create(&Message {
                                    user: session.viewer.into(),
                                    room: (*room_id).into(),
                                    text: wire_text,
                                    timestamp: js_sys::Date::now() as i64,
                                    deleted: false,
                                    edited_at: None,
                                    collaborative: None,
                                    re: reply_to.as_ref().map(ankurah::Ref::from),
                                })
                                .await?;
                                trx.commit().await?;
                            }
                            ComposerTarget::Dm { partner } => crate::dm::send_dm(&session, *partner, wire_text).await?,
                        }
                        Ok::<_, Box<dyn std::error::Error>>(())
                    }
                    .await;
                    if let Err(e) = result {
                        // Including a refusal — a conversation that became the
                        // reader's own by a session swap, say. The draft goes
                        // back either way: the words are the reader's, and a
                        // refused send is not a reason to take them.
                        tracing::error!("Failed to send message: {}", e);
                        // Put the failed text back — above anything typed since,
                        // never over it — and re-arm the reply unless a new one
                        // was chosen meanwhile, or an edit began (the chip and
                        // an edit are mutually exclusive; resurrecting it under
                        // an open editor would promise a `re` on the NEXT new
                        // message instead).
                        message_input.update(|current| {
                            if current.trim().is_empty() {
                                *current = input_text;
                            } else {
                                *current = format!("{input_text}\n{current}");
                            }
                        });
                        if replying_to.get_untracked().is_none() && editing_message.get_untracked().is_none() {
                            replying_to.set(reply_to);
                        }
                    }
                });
            }
        }
    };

    // Select the previous/next message the reader wrote, for editing.
    let navigate_own = {
        let chat = chat.clone();
        move |backward: bool| {
            let Some(user_id) = chat.viewer_untracked().map(|id| id.to_base64()) else { return };
            let msgs = messages.get_untracked();
            if msgs.is_empty() {
                return;
            }
            // Tombstones are not editable — skip them while navigating.
            let is_own = |m: &MessageView| {
                m.user().ok().map(|r| r.id().to_base64()).as_deref() == Some(user_id.as_str())
                    && !m.deleted().unwrap_or(false)
            };

            let current_idx = editing_message
                .get()
                .and_then(|em| {
                    let id = em.id().to_base64();
                    msgs.iter().position(|m| m.id().to_base64() == id)
                });

            if backward {
                // Cmd/Ctrl+Up: search toward older messages (lower indices).
                let start = current_idx.unwrap_or(msgs.len());
                for i in (0..start).rev() {
                    if is_own(&msgs[i]) {
                        editing_message.set(Some(msgs[i].clone()));
                        return;
                    }
                }
            } else if let Some(start) = current_idx {
                // Cmd/Ctrl+Down: only meaningful while editing; search toward newer messages.
                for i in (start + 1)..msgs.len() {
                    if is_own(&msgs[i]) {
                        editing_message.set(Some(msgs[i].clone()));
                        return;
                    }
                }
                // Past the newest own message: exit edit mode.
                editing_message.set(None);
                message_input.set(String::new());
            }
        }
    };

    let handle_key_down = {
        let send = send.clone();
        let demand_instead_of_caret = demand_instead_of_caret.clone();
        let demand_once = demand_once.clone();
        move |e: KeyboardEvent| {
            // A caret already in the box, and no viewer any more: the session
            // dropped to anonymous under a focus that had already landed, which
            // is the one arrival none of the composer's other gates can see.
            // `readonly` has already made the key do nothing; this is what
            // stops it being SILENT. Only for keys that would have changed or
            // sent the draft — a caret move is not a reach for the box — and
            // latched like every other route, so holding a key down asks once.
            if demand_instead_of_caret() && would_write_draft(&e) {
                demand_once();
                return;
            }
            // While the mention popup is open it captures its keys —
            // but never modifier'd ones, so Cmd/Ctrl+Up edit-nav still works.
            let matches = mention_matches.get_untracked();
            if !matches.is_empty() && !e.meta_key() && !e.ctrl_key() && !e.alt_key() {
                match e.key().as_str() {
                    "ArrowDown" => {
                        e.prevent_default();
                        mention_selected.update(|i| *i = (*i + 1) % matches.len());
                        return;
                    }
                    "ArrowUp" => {
                        e.prevent_default();
                        mention_selected.update(|i| *i = (*i + matches.len() - 1) % matches.len());
                        return;
                    }
                    "Enter" | "Tab" if !e.shift_key() => {
                        // keyCode 229: WebKit fires the composition-commit
                        // keydown AFTER compositionend with isComposing=false
                        // — without this check Safari IME users select a
                        // candidate and we treat it as popup confirmation.
                        if !e.is_composing() && e.key_code() != 229 {
                            e.prevent_default();
                            let idx = mention_selected.get_untracked().min(matches.len() - 1);
                            insert_mention(&matches[idx]);
                        }
                        return;
                    }
                    "Escape" => {
                        // Closes only the popup; a second Escape cancels the edit.
                        e.prevent_default();
                        mention_draft.set(None);
                        return;
                    }
                    _ => {}
                }
            }
            // The emoji popup captures the same keys while open, with
            // the same IME guards. Never open at the same time as the mention
            // popup (the drafts are mutually exclusive by construction).
            let ematches = emoji_matches.get_untracked();
            if !ematches.is_empty() && !e.meta_key() && !e.ctrl_key() && !e.alt_key() {
                match e.key().as_str() {
                    "ArrowDown" => {
                        e.prevent_default();
                        emoji_selected.update(|i| *i = (*i + 1) % ematches.len());
                        return;
                    }
                    "ArrowUp" => {
                        e.prevent_default();
                        emoji_selected.update(|i| *i = (*i + ematches.len() - 1) % ematches.len());
                        return;
                    }
                    "Enter" | "Tab" if !e.shift_key() => {
                        // Same WebKit guard as the mention popup: the
                        // composition-commit keydown (keyCode 229) must not
                        // read as popup confirmation.
                        if !e.is_composing() && e.key_code() != 229 {
                            e.prevent_default();
                            let idx = emoji_selected.get_untracked().min(ematches.len() - 1);
                            insert_emoji(ematches[idx].1);
                        }
                        return;
                    }
                    "Escape" => {
                        // Consumed: the window-level Escape (panel manager)
                        // skips defaultPrevented events, so only the popup
                        // closes.
                        e.prevent_default();
                        emoji_draft.set(None);
                        return;
                    }
                    _ => {}
                }
            }
            // Enter sends; Shift+Enter falls through to the textarea's native
            // newline. An Enter that confirms an IME composition must
            // not send: isComposing covers Chrome/Firefox, and keyCode 229
            // covers WebKit, which fires the commit keydown after
            // compositionend with isComposing already false. repeat() drops
            // key-autorepeat so holding Enter sends once, not once per repeat.
            if e.key() == "Enter" && !e.shift_key() && !e.is_composing() && e.key_code() != 229 && !e.repeat() {
                e.prevent_default();
                send();
            } else if e.key() == "Escape" && editing_message.get().is_some() {
                e.prevent_default();
                editing_message.set(None);
                message_input.set(String::new());
            } else if e.key() == "Escape" && replying_to.get().is_some() {
                // Cancel the armed reply; the draft text is untouched.
                // preventDefault keeps the window-level Escape (panel manager)
                // from also acting on this press.
                e.prevent_default();
                replying_to.set(None);
            } else if e.key() == "ArrowUp" && (e.meta_key() || e.ctrl_key()) {
                e.prevent_default();
                navigate_own(true);
            } else if e.key() == "ArrowDown" && (e.meta_key() || e.ctrl_key()) && editing_message.get().is_some() {
                e.prevent_default();
                navigate_own(false);
            }
        }
    };

    let send_click = send.clone();
    view! {
        <div class="ankurah-chat inputContainer">
            // Mention autocomplete popup: floats above the composer.
            // mousedown is prevented throughout so the textarea keeps focus
            // (its blur handler would otherwise close the popup pre-click).
            <Show when=move || !mention_matches.get().is_empty()>
                <div
                    class="mentionPopup"
                    role="listbox"
                    aria-label="Mention a member"
                    on:mousedown=|e: leptos::ev::MouseEvent| e.prevent_default()
                >
                    {move || {
                        mention_matches
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(i, user)| {
                                let name = user
                                    .display_name()
                                    .unwrap_or_default();
                                let initials = fmt::initials(&name);
                                let hue = fmt::hue_class(&user.id().to_base64());
                                view! {
                                    <button
                                        type="button"
                                        class=move || {
                                            if mention_selected.get() == i {
                                                "mentionItem active"
                                            } else {
                                                "mentionItem"
                                            }
                                        }
                                        role="option"
                                        aria-selected=move || {
                                            if mention_selected.get() == i { "true" } else { "false" }
                                        }
                                        on:mouseenter=move |_| mention_selected.set(i)
                                        on:click=move |_| insert_mention(&user)
                                    >
                                        <span class=format!("mentionAvatar {hue}") aria-hidden="true">
                                            {initials}
                                        </span>
                                        <span class="mentionName">{name.clone()}</span>
                                    </button>
                                }
                            })
                            .collect_view()
                    }}
                </div>
            </Show>
            // Emoji autocomplete popup: same shell and interaction
            // contract as the mention popup, over the shortcode table.
            <Show when=move || !emoji_matches.get().is_empty()>
                <div
                    class="mentionPopup"
                    role="listbox"
                    aria-label="Insert an emoji"
                    on:mousedown=|e: leptos::ev::MouseEvent| e.prevent_default()
                >
                    {move || {
                        emoji_matches
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(i, (name, glyph))| {
                                view! {
                                    <button
                                        type="button"
                                        class=move || {
                                            if emoji_selected.get() == i {
                                                "mentionItem active"
                                            } else {
                                                "mentionItem"
                                            }
                                        }
                                        role="option"
                                        aria-selected=move || {
                                            if emoji_selected.get() == i { "true" } else { "false" }
                                        }
                                        on:mouseenter=move |_| emoji_selected.set(i)
                                        on:click=move |_| insert_emoji(glyph)
                                    >
                                        <span class="emojiGlyph" aria-hidden="true">{glyph}</span>
                                        <span class="mentionName">{format!(":{name}:")}</span>
                                    </button>
                                }
                            })
                            .collect_view()
                    }}
                </div>
            </Show>
            // Reply chip: compact "Replying to …" state above the input.
            // Live reads: a rename, edit, or delete of the original while the
            // chip is up re-renders it (a deleted original still sends — `re`
            // points at the tombstone, which the preview renders honestly).
            <Show when=move || replying_to.get().is_some()>
                {move || {
                    replying_to
                        .get()
                        .map(|orig| {
                            let author_id = orig.user().map(|r| r.id().to_base64()).unwrap_or_default();
                            let author = member_names
                                .with(|names| names.get(&author_id).cloned())
                                .filter(|n| !n.is_empty())
                                .unwrap_or_else(|| "Unknown".to_string());
                            let snippet = if orig.deleted().unwrap_or(false) {
                                "Removed message".to_string()
                            } else {
                                member_names
                                    .with(|names| crate::mentions::reply_snippet(&orig.text().unwrap_or_default(), names))
                            };
                            view! {
                                <div class="replyingNotice">
                                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                                        stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                                        <polyline points="9 14 4 9 9 4" />
                                        <path d="M20 20v-7a4 4 0 0 0-4-4H4" />
                                    </svg>
                                    <span class="replyingNoticeLabel">"Replying to " {author}</span>
                                    <span class="replyingNoticeSnippet">{snippet}</span>
                                    <button
                                        class="replyingNoticeCancel"
                                        aria-label="Cancel reply"
                                        title="Cancel reply"
                                        on:click=move |_| replying_to.set(None)
                                    >
                                        "×"
                                    </button>
                                </div>
                            }
                        })
                }}
            </Show>
            <Show when=move || editing_message.get().is_some()>
                <div class="editingNotice">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                        stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                        <path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z" />
                    </svg>
                    <span>"Editing message"</span>
                    <span class="editingNoticeHint">
                        <kbd>"Esc"</kbd>
                        " to cancel"
                    </span>
                </div>
            </Show>
            <div class="inputRow">
                // Multiline composer. Keeps class="input" + the same
                // placeholder: e2e locates it by `.input[placeholder=...]`.
                <textarea
                    node_ref=textarea_ref
                    class="input"
                    placeholder="Type a message..."
                    rows="1"
                    aria-label="Message"
                    prop:value=move || message_input.get()
                    on:input=move |ev| {
                        message_input.set(event_target_value(&ev));
                        if let Some(el) = textarea_ref.get_untracked() {
                            autosize(&el);
                        }
                        // Order matters: a just-completed `:name:` splices
                        // first; the drafts re-derive from the result.
                        complete_typed_shortcode(&ev);
                        refresh_drafts();
                    }
                    on:keydown=handle_key_down
                    // Caret moves without input events: arrows/Home/End
                    // keyup and mouse clicks re-derive the drafts.
                    on:keyup=move |e: KeyboardEvent| {
                        if matches!(
                            e.key().as_str(),
                            "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" | "Home" | "End"
                        ) {
                            refresh_drafts();
                        }
                    }
                    on:click=move |_| refresh_drafts()
                    on:blur=move |_| {
                        mention_draft.set(None);
                        emoji_draft.set(None);
                    }
                    // An anonymous reader gets the host's sign-in ceremony
                    // rather than a caret, and no text of theirs reaches the
                    // draft by any route. Every one of these is inert for a
                    // signed-in reader and for a host that installed no
                    // callback — `readonly` and `tabindex` are absent rather
                    // than false and "0", so their markup is untouched. The
                    // keydown demand lives inside `handle_key_down` above. See
                    // the latch rule for all of it.
                    readonly=anonymous_readonly
                    tabindex=anonymous_tabindex
                    on:pointerdown=mark_gesture
                    on:mousedown=refuse_caret
                    on:focus=demand_on_focus
                    on:beforeinput=refuse_insertion
                    on:drop=refuse_drop
                    prop:disabled=move || !is_connected()
                ></textarea>
                <Show when=move || editing_message.get().is_some()>
                    <button
                        class="button buttonGhost"
                        on:click=move |_| {
                            editing_message.set(None);
                            message_input.set(String::new());
                        }
                    >
                        "Cancel"
                    </button>
                </Show>
                <button class="button sendButton" on:click=move |_| send_click() prop:disabled=move || !can_send()>
                    {move || if editing_message.get().is_some() { "Update" } else { "Send" }}
                </button>
            </div>
        </div>
    }
}

/// The message box, on its own.
///
/// Everything it needs is the target: a room id, or a correspondent's id.
/// Mount it where there is no timeline above it — a page that only offers
/// "message me" is one composer and nothing else.
///
/// A reader with no viewer gets the host's sign-in ceremony the moment they
/// press on the box rather than a caret, so a standalone composer on a public
/// page is an invitation to sign in and never a draft that cannot be sent. The
/// write path demands as well. Both want
/// [`crate::ChatContextBuilder::on_auth_demand`]; with no callback installed
/// the box takes focus as it always did and the send is refused with a warning
/// in the log.
///
/// GIVE A KEYBOARD-ONLY READER SOMETHING ELSE TO REACH. In a standalone mount
/// this component is the whole surface, and for a reader with no viewer none
/// of it is a tab stop: the box is skipped on purpose, and Send is disabled
/// because the draft is empty. Tab therefore passes the composer by and offers
/// nothing — which is right for the box itself, and leaves a host mounting only
/// this one with a sign-in affordance of its own to put beside it. A mount that
/// sits inside a page with its own sign-in control already has one.
///
/// What a standalone composer does NOT do is edit. Editing and replying are
/// things a reader starts FROM a message, and there is no message on screen to
/// start from, so the state that carries them is owned here, empty, and never
/// armed: the "Replying to …" chip never appears, Cmd/Ctrl+Up walks an empty
/// list, and Escape has nothing to cancel. Mount [`crate::RoomLog`] to get
/// those, which is the same component with the timeline's signals threaded in.
#[component]
pub fn Composer(target: ComposerTarget) -> impl IntoView {
    let editing_message = RwSignal::new(None::<MessageView>);
    let replying_to = RwSignal::new(None::<MessageView>);
    let no_messages = Signal::derive(Vec::<MessageView>::new);
    view! {
        <WiredComposer
            target=target
            editing_message=editing_message
            replying_to=replying_to
            messages=no_messages
        />
    }
}
