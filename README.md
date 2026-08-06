# ankurah-chat

Embeddable Ankurah chat: the data model and Leptos components behind
[community.ankurah.org](https://community.ankurah.org), packaged for hosts
that want to put live chat surfaces in their own pages.

**This is a library, not an app.** The host application:

- stands up its own ankurah node (typically an in-browser ephemeral node
  connected over websocket to a durable chat server) and hands these
  components a **signal** of its session — that `ankurah::Context` and who is
  reading through it. The host owns that signal and is the only thing that
  writes it;
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

// Your signal, holding your ankurah::Context and who is reading through it
// (`None` for read-only). You own it; the components only read it.
let session = RwSignal::new((context, Some(my_user_id)));

ChatContext::new(session)
    .on_auth_demand(|| start_sign_in())
    .provide();
install_styles();

view! { <RoomLog room=room_id /> }
```

**The surfaces take identifiers, not objects.** A room id, a correspondent's
id — nothing else. The queries behind them, the members list, the read cursors:
all of that belongs to the handshake, which builds it once per session and
rebuilds it when the session moves. There is no query to construct, no manager
to keep, and nothing to hand back in.

The surfaces are `RoomSelector` and `RoomLog` (both keyed on a room id),
`Composer` (a `ComposerTarget` naming a room or a correspondent), and
`DmSidebar` / `DmConversation` (keyed on a correspondent's id). Mount any of
them, in any combination.

Which rooms exist is the one predicate a host owns, and it is declared as data:
`ChatContext::new(session).rooms_where("name = 'general'")`. It scopes the
selector and the unread windows together.

### On your side

- **Adopt the pin family above.** ankurah-signals 0.9.0 holds js-sys/web-sys at
  =0.3.82, and leptos 0.8.15+ demands ^0.3.85 through server_fn → wasm-streams.
  The two cannot both be satisfied; raise both ends together or neither.
- **Name a getrandom backend.** ankurah reaches getrandom transitively for
  entity ids, and it refuses `wasm32-unknown-unknown` until something names a
  backend. These crates declare the `wasm_js` feature for 0.3 and `js` for 0.2
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

### Theming

Every colour and metric is an `--akchat-*` custom property. Re-declare the ones
you want **on `.ankurah-chat`**, which is the class each component root carries:

```css
.ankurah-chat { --akchat-bg: #101014; --akchat-text: #e7e7ea; }
```

**Not on `:root`.** The crate declares its own defaults on the component root
itself, and a declaration on an element always beats a value inherited from an
ancestor — specificity only orders declarations competing for the same element.
A `:root` mapping would be inherited down and then overwritten by the defaults
it was meant to replace. On `.ankurah-chat` it is one class against the crate's
zero (its defaults use `:where`), so your value wins.

### Signing in mid-visit

Set your session signal:

```rust
session.set((new_context, Some(user_id)));
```

That is the whole of it, and it is the only write path — the components never
write that signal. Every read inside them takes the pair as one value, so
nothing in there can see the new context beside the previous reader. **Set it
as one value too.** A signal you derive from a context signal and a viewer
signal, or an in-place `update` that moves the context now and the reader
after, tears the pair before it ever reaches the components — and the session
in between is precisely the mismatch that one value exists to rule out.

Nothing unmounts, so the draft, the armed reply, the selected room, the open
conversation and the message being edited all stay exactly as they were;
everything scoped to the session — members, rooms, DM threads, both read-cursor
managers — is rebuilt inside the handshake. You hold no query and no manager,
so there is nothing to swap alongside it and no window where half the surfaces
read through one session and half through another.

Three moments follow your `.set()`, and only the last two wait for a tick.
Everything session-scoped is keyed to a VERSION of your signal that recomputes
on the first read after the set, so nothing can be handed out that pairs your
new context with the departed session's queries — not even within that tick.
The DISCARD, which disposes what the departed session built, is driven by an
effect and lands a tick later (sooner, if a surface asks for something first).
The REBUILD is later still: it happens when the components re-run on that
version's notification and ask again.

A write that had ALREADY passed its last check when the discard lands finishes
— as itself, by the author who started it, against that session's own rows and
through that session's context, however long its commit takes. That is the old
session completing its own bookkeeping. No new work begins after the discard,
and nothing keeps a manager or a flush alive for as long as some background
task happens to hold it.

One thing does not survive: the timeline's loaded window. A `ScrollManager`
takes its context at construction and ankurah-virtual-scroll 0.9.0 cannot
re-point one, so the pane rebuilds and the reader lands at the live tail rather
than wherever they had paged back to.

The reader may CHANGE, not merely appear — signing in as somebody else is a
legitimate swap. What the previous reader had selected stays in your signals,
because those are yours; every write revalidates its author against the session
before committing, and a conversation whose two ends have become the same
person is refused rather than written.

Handles the accessors give you (`chat.members()`, `chat.rooms()`, the cursor
managers) are BORROWS of the session's, not things to park. Keep one past a
swap and it goes on reading through a context the reader has left. Ask again;
it is a cache lookup.

If you would RATHER discard everything on sign-in — draft included — key the
subtree on `chat.generation()` and Leptos will remount it for you. Both models
work; neither is privileged.

Unmounting the subtree the handshake was provided in ENDS it, terminally:
teardown disposes everything the session built, and from that moment every
query and cursor accessor answers `None`, writes are refused without raising
the auth demand, and `chat.generation()` answers its final number, frozen.
The raw session reads (`chat.context()`, `chat.viewer()` and their untracked
forms) are the one exception — they have no `None` to answer, and they read
YOUR signal, whose lifetime is yours. A handle you kept past teardown is a
handle to an ended handshake, not a way to keep using one — mount a new
handshake instead.

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
