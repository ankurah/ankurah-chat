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
    include_str!("styles/message_row.css"),
    include_str!("styles/markdown.css"),
    include_str!("styles/reactions.css"),
    include_str!("styles/context_menu.css"),
    include_str!("styles/composer.css"),
    include_str!("styles/direct_messages.css"),
);

/// The `id` on the injected element, which is also how a second call knows
/// there is nothing to do.
const STYLE_ELEMENT_ID: &str = "ankurah-chat-styles";

/// Put [`STYLES`] in the document's head, once.
///
/// A host that wants the rules to sit EARLIER in the cascade than its own
/// stylesheets should call this before those load — the components' selectors
/// each carry the scope class, so they outrank an unscoped host rule wherever
/// they land, and a host that means to override one should say so with a
/// `.ankurah-chat` prefix of its own.
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
