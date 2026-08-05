# ankurah-chat

Embeddable Ankurah chat: the data model and Leptos components behind
[community.ankurah.org](https://community.ankurah.org), packaged for hosts
that want to put live chat surfaces in their own pages.

**This is a library, not an app.** The host application:

- stands up its own ankurah node (typically an in-browser ephemeral node
  connected over websocket to a durable chat server) and hands these
  components an `ankurah::Context`;
- owns sign-in entirely — components render read-only on an unauthenticated
  context, and the *send affordance* invokes a **host-provided callback**
  when auth is needed. Upgrading anonymous → authenticated must not remount
  or lose state;
- mounts surfaces independently: room selector, room log, composer, and the
  DM thread view are separately embeddable (a host can show a single room,
  or just a DM panel, without the rest).

## Crates

- `ankurah-chat-model` — the chat collections (Message, Room, User, Reaction,
  ReadState, and the DM trio DmThread/DmMessage/DmReadState) and the
  mention/URL scanner. This is the ONE definition each of them: a chat server
  and its clients both link this crate, so neither can drift from the other.
  **Interop constraint:** ankurah derives a collection's identifier from the
  struct name and a property's from the field name, so those names are wire
  and are pinned by live data — a rename is a migration, never a refactor.
  The scanner's caps (64 KiB window, 20 mentions, 8 URLs) are a shared
  client/server contract on the same terms: change them for every consumer in
  lockstep, or not at all. Ankurah + serde only; builds for a native server
  and for `wasm32-unknown-unknown` alike.
- `ankurah-chat-leptos` — the components. Themable via CSS custom-property
  tokens with neutral defaults; carries its own scoped reset and
  reduced-motion handling; designed to live small (usable down to ~320 px
  wide). Query observability goes through a generic registry hook the host
  may attach an observer to (e.g. the `ankurah-xray` inspector) — the crate
  itself is inspector-agnostic.

## Version pins

The workspace resolves inside the ankurah 0.9.0 pin family (wasm-bindgen
=0.2.105, web-sys/js-sys =0.3.82, wasm-bindgen-futures =0.4.55, leptos
0.8.12–0.8.14 with leptos_macro <0.8.15). The ceilings are requirements, not
preferences — see the workspace `Cargo.toml` comments.

## Status & trajectory

The model has landed and community.ankurah.org consumes it — the collections
and the scanner moved OUT of community's own model crate rather than being
copied, so no second copy of the scanner caps exists to drift. community's
model crate keeps what is community's alone (moderation records, the
notification inbox, the link-preview cache) and re-exports what moved.

Next: the components extract to `ankurah-chat-leptos` on the same terms (a
cleaned copy of community's, x-ray wiring replaced by the generic registry
hook), with community as the reference consumer and the danielnorman.net
portfolio embed as the second. Consumer requirements are pinned on
ankurah/community#46; the wider reconvergence map is ankurah/community#53.

Not published to crates.io yet: consumers pin a git rev while the API is
still moving under dogfooding.
