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
  ReadState, and the DM trio DmThread/DmMessage/DmReadState), the mention/URL
  scanner, and the `mention_display` codec that lets a composer show
  `@DisplayName` over the stored token. This is the ONE definition each of
  them: a chat server and its clients both link this crate, so neither can
  drift from the other.
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

## Consuming this crate from a wasm app

Not on crates.io yet, so depend on a git rev — an exact one, never a branch:
these structs are the format of live rows, so the tree you compile against is
something to choose rather than follow.

```toml
ankurah-chat-model  = { git = "https://github.com/ankurah/ankurah-chat", rev = "<full sha>" }
ankurah-chat-leptos = { git = "https://github.com/ankurah/ankurah-chat", rev = "<full sha>" }
```

Mounting a surface is three steps — provide the handshake, install the styles,
mount what you want:

```rust
use ankurah_chat_leptos::{ChatContext, RoomLog, install_styles};

ChatContext::new(context)              // your ankurah::Context
    .viewer(Some(my_user_id))          // or leave it out: read-only
    .on_auth_demand(|| start_sign_in())
    .provide();
install_styles();

view! { <RoomLog room=selected_room current_user=me users=users read_state=cursors /> }
```

Then, on your side:

- **Adopt the pin family above.** ankurah-signals 0.9.0 holds js-sys/web-sys at
  =0.3.82, and leptos 0.8.15+ demands ^0.3.85 through server_fn → wasm-streams.
  The two cannot both be satisfied; raise both ends together or neither.
- **Name a getrandom backend.** ankurah reaches getrandom transitively for
  entity ids, and it refuses `wasm32-unknown-unknown` until something names a
  backend. This crate declares the `wasm_js` feature for 0.3 and `js` for 0.2
  on wasm targets, which is what the resolved versions need — but a
  `.cargo/config.toml` does **not** travel with a dependency, so the
  `getrandom_backend="wasm_js"` rustflag this repo sets for its own wasm checks
  is not something you inherit. If your resolve lands on an early 0.3.x that
  wants the cfg, set it in your own workspace:

  ```toml
  # <your workspace>/.cargo/config.toml
  [target.wasm32-unknown-unknown]
  rustflags = ["--cfg", "getrandom_backend=\"wasm_js\""]
  ```

  (community does exactly this, in `leptos-app/.cargo/config.toml`.)
- **Do not pass a `wasm` feature** — there isn't one. `ankurah/wasm` is enabled
  by target, because no wasm build would want it off and forgetting it produces
  a confusing failure deep inside ankurah-core.

## Status & trajectory

The model has landed and community.ankurah.org consumes it — the collections
and the scanner moved OUT of community's own model crate rather than being
copied, so no second copy of the scanner caps exists to drift. community's
model crate keeps what is community's alone (moderation records, the
notification inbox, the link-preview cache) and re-exports what moved.

The components followed on the same terms: extracted, not copied, with
community consuming them as the reference embedder and the danielnorman.net
portfolio embed as the second. The inspector wiring community's components
carried is gone from them — every message bubble now carries `data-entity-id`
and `data-collection`, and community's x-ray installs its own handlers over
those, which is the end state
[ankurah/community#53](https://github.com/ankurah/community/issues/53)
describes. Consumer requirements are pinned on
[ankurah/community#46](https://github.com/ankurah/community/issues/46).

Not published to crates.io yet: consumers pin a git rev while the API is
still moving under dogfooding.
