# Migration note: author names resolve by ref, not through the roster

For embedders bumping their pin past this change (e.g. from `12c1cb9`).

## What changed

The room timeline no longer names authors by searching the shared members
roster (`ChatContext::members()`, a listing query over the whole `user`
collection). The message list now resolves each **distinct author id once**,
by following the message's own `user` ref (`ctx.get_cached::<UserView>(id)`,
local cache first), into one shared map; every row receives its resolved
`Option<UserView>` as a prop, and the avatar/author-label markup lives in
leaf components that render purely from that prop.

Why: listing the `user` collection is a member privilege. Under a server
policy that grants unauthenticated sessions `retrieve` on `user` but not
listing (ankurah-jwt-auth 0.9.2, e.g. `user: { retrieve: "view" }`), a
guest's roster query opens and stays empty — so every author rendered
"Unknown" for signed-out readers. A ref follow by id is the read a guest is
allowed.

## What you must change

Nothing, for a typical embedder. `RoomLog`, `Composer`, `RoomSelector`,
`DmConversation`, `DmSidebar`, `ChatContext` and the rest of the public API
keep their signatures; everything that changed (`MessageRow`, the message
list, the new avatar/name leaf components) is crate-private.

What changes observably:

- Signed-out readers see real author names **provided the server grants
  `retrieve` on `user` to unauthenticated sessions**. Against an older
  policy the timeline renders exactly as before ("Unknown"), plus one
  `tracing` warning per unresolvable author id per session.
- **Snapshot semantics for guests**: a ref follow leaves no standing
  subscription, so a display-name change does not live-update a signed-out
  reader's author labels. Member surfaces that still read the roster
  (mention chips, composer autocomplete, DM names) keep their liveness.
  Live named-row reads are a planned follow-up gated on a jwt-auth change.
- Dependency floor is unchanged (`ankurah ^0.9.0`); a fresh resolve lands on
  0.9.2, which is also what the retrieve-aware server side ships with.

## What stays roster-backed, deliberately

- Composer mention autocomplete, and mention-chip rendering in message text.
- Reply-preview author names (they read the same mention-name map).
- DM sidebar/conversation names (`display_name`) and other member-only
  panels.
