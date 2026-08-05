//! The components' stylesheet, carried in the binary.
//!
//! What this is for: a host should be able to `cargo add` these components and
//! mount one, with no CSS to copy and no build-tool wiring. The rules live
//! beside the components that need them and are compiled in, so a stylesheet
//! can never be out of step with the markup it styles.
//!
//! Everything is confined to the `.ankurah-chat` class each component root
//! carries — see `styles/theme.css`, which also holds the `--akchat-*` palette
//! and the scoped reset.
//!
//! Install once per document. Later calls do nothing, so a page mounting three
//! separate surfaces may have each of them ask.

use leptos::prelude::*;

/// Every rule the components need, in cascade order: the palette and the
/// scoped reset first, then one block per surface. Public so a host that
/// prefers to ship CSS its own way — a `<link>` to a file it writes at build
/// time, an inline `<style>` in a server-rendered page — can take the text and
/// place it itself, instead of calling [`install_styles`].
pub const STYLES: &str = concat!(
    include_str!("styles/theme.css"),
    include_str!("styles/room_selector.css"),
    include_str!("styles/room_log.css"),
    include_str!("styles/message_row.css"),
    include_str!("styles/markdown.css"),
    include_str!("styles/reactions.css"),
    include_str!("styles/context_menu.css"),
    include_str!("styles/composer.css"),
    include_str!("styles/direct_messages.css"),
    include_str!("styles/debug_header.css"),
);

/// The `id` on the injected element, which is also how a second call knows
/// there is nothing to do.
const STYLE_ELEMENT_ID: &str = "ankurah-chat-styles";

/// Put [`STYLES`] in the document's head, once.
///
/// WHERE IT LANDS IN THE CASCADE, plainly: appended to `<head>` at the moment
/// of the call, which for a host whose own stylesheets are `<link>` elements in
/// the document — a trunk build, say — is always AFTER them, no matter how
/// early this is called. Those links are parsed with the document; this runs
/// once wasm has started.
///
/// So at EQUAL specificity the components win. That is usually right — every
/// selector here carries the scope class, so it is already more specific than
/// an unscoped host rule — but a host overriding a component rule has to mean
/// it: prefix with `.ankurah-chat` to tie, and add one more class or an
/// attribute to win. Community's inspector does exactly that, keying its
/// bubble rules on `[data-entity-id][data-collection]`.
///
/// A host that would rather control the order completely can take [`STYLES`]
/// and place the text itself.
pub fn install_styles() {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    if document.get_element_by_id(STYLE_ELEMENT_ID).is_some() {
        return;
    }
    let Some(head) = document.head() else { return };
    let Ok(element) = document.create_element("style") else { return };
    element.set_id(STYLE_ELEMENT_ID);
    element.set_text_content(Some(STYLES));
    let _ = head.append_child(&element);
}

/// [`install_styles`] as a component, for a host that would rather mount it
/// than call it. Renders nothing.
#[component]
pub fn ChatStyles() -> impl IntoView {
    install_styles();
}
