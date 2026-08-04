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

- `ankurah-chat-model` — the chat collections (Message, Room, User, …) and
  the mention/URL scanner. **Interop constraint:** these must stay
  collection- and wire-compatible with the community server they connect to;
  the scanner's caps (64 KiB window, 20 mentions, 8 URLs) are a shared
  client/server contract — change in lockstep with the server or not at all.
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

Scaffold. The components arrive as a cleaned copy of community's (x-ray
wiring replaced by the generic registry hook); the first consumer is the
danielnorman.net portfolio embed. community.ankurah.org itself switches from
its in-tree copies to this crate after ankurah grows native introspection
(the retirement map is ankurah/community#53). Consumer requirements are
pinned on ankurah/community#46.
