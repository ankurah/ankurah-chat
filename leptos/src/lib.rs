// A browser crate through and through: every component reaches for a window, a
// scroll container, a textarea's caret. On any other target it compiles to
// nothing, so a workspace that also holds a native server can run
// `cargo test --workspace` without this crate's dependency graph appearing in
// it. Build it for `wasm32-unknown-unknown` to see anything at all.
#![cfg(target_arch = "wasm32")]
//! Embeddable Leptos chat components for Ankurah.
//!
//! Surfaces, each mountable on its own:
//!
//! - [`RoomSelector`] — the list of rooms, with unread badges and a
//!   create-room affordance;
//! - [`RoomLog`] — one room's message timeline, paginated and pinned to the
//!   live tail, with its composer;
//! - [`Composer`] — the message box on its own, with mention and `:emoji:`
//!   completion;
//! - [`DmThread`] and [`DmSidebar`] — direct messages: the conversation, and
//!   the list of conversations.
//!
//! A page may mount any of them without the others: a single room log with no
//! selector, or a DM panel with no rooms at all.
//!
//! # What a host provides
//!
//! An [`ankurah::Context`], through [`ChatContext`]. This crate creates no
//! node, opens no socket, and knows nothing about how anyone signs in — a host
//! stands up its own node (in a browser, typically an ephemeral node talking
//! to a durable chat server over a websocket) and hands the context over. See
//! the [`context`] module for the whole handshake, including how a reader can
//! sign in mid-session without the components remounting.
//!
//! Rows come from
//! [`ankurah-chat-model`](https://github.com/ankurah/ankurah-chat), which the
//! host's chat server links too — that is what makes an embedded panel and the
//! service it talks to agree on what a message is.
//!
//! # Styling
//!
//! Call [`install_styles`] once, or mount [`ChatStyles`]. Everything the
//! components need travels with them: no stylesheet to copy, no build step.
//! Every rule is confined to the `.ankurah-chat` class the component roots
//! carry, so the surrounding page is not restyled, and every colour and metric
//! is an `--akchat-*` custom property a host may re-declare on
//! `.ankurah-chat`. The defaults are neutral, the scoped reset covers what an
//! app-wide reset normally would, and `prefers-reduced-motion` is honoured.
//!
//! # Small pieces a host can reuse
//!
//! [`fmt`] holds the presentation rules a host's own chrome has to agree with:
//! a member's avatar hue and initials are computed there, so a member list
//! beside these components colours the same person the same way. [`queries`]
//! builds parameterized AnkQL, and [`emoji`] is the `:shortcode:` table the
//! composer completes against.
//!
//! # Watching the queries the components hold
//!
//! Components register each long-lived query with the [`query_registry`], and
//! a host may attach any observer it likes — a debugging panel, a metrics
//! collector, a test harness. Attach before mounting; see that module for why.
//!
//! # Inspecting entities from outside
//!
//! Every message bubble carries `data-entity-id` (the base64 entity id) and
//! `data-collection`. Nothing in this crate reads them: they exist so a host
//! can install its own handlers — a hover treatment, click-to-inspect, an
//! outline on entities with concurrent heads — over a component tree that
//! knows nothing about that host's inspector. Bubbles also carry `data-msg-id`,
//! which is what the scroll pane finds visible rows by.

pub mod context;
pub mod emoji;
pub mod fmt;
mod grouping;
mod mentions;
pub mod queries;
pub mod query_registry;
mod reactions;
mod styles;

pub use context::{ChatContext, ChatContextBuilder, ChatHooks, MessageSlot, ModeratorDelete, Session};
pub use reactions::REACTION_EMOJIS;
pub use styles::{install_styles, ChatStyles, STYLES};
