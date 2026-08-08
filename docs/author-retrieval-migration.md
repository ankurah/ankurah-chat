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
- Signed-out readers now also see real **mention-chip names** and
  **reply-preview author names/snippets** (previously "@unknown"/"Unknown"),
  under the same `retrieve`-on-`user` grant — see the by-ref extension below.
- **Snapshot semantics for guests**: a ref follow leaves no standing
  subscription, so a display-name change does not live-update a signed-out
  reader's labels — author names, mention chips and reply-preview names
  alike, all now resolved by ref. The member-only surfaces that still read
  the roster (composer autocomplete, DM names) keep their liveness.
  Live named reads for the by-ref surfaces are a planned follow-up gated on a
  jwt-auth change.
- Dependency floor is unchanged (`ankurah ^0.9.0`); a fresh resolve lands on
  0.9.2, which is also what the retrieve-aware server side ships with.

## By-ref extension: mention chips and reply previews

The message list's single by-ref resolver now feeds two more surfaces that
used to read the roster, so a signed-out reader sees their names too:

- **Mention chips** in message text: every mentioned id in the window (the
  canonical `<@id>` scanner shared with the server) is resolved through the
  same map; the id → display-name map the rows consume is built from those
  resolutions instead of from `members()`.
- **Reply previews**: the replied-to message's author name and the mention
  tokens inside its one-line snippet, reached by first following the reply's
  `re` ref to that message, then resolving its author (and its mentions).

`MessageRow`'s props are unchanged — the `mention_names` map it takes has the
same type; only its source moved. An id that will not resolve keeps the
existing "@unknown"/"Unknown" fallback.

## What stays roster-backed, deliberately

- Composer mention autocomplete candidates (a member-only listing surface).
- The DM sidebar, DM conversation names, and the DM message list: a private
  thread is only ever visible to its two members, so the roster is always
  populated there and there is no guest to strand.
- Other member-only panels (e.g. the members list).
