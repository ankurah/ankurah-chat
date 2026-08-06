//! The reactive fact the session version stands on.
//!
//! `ChatContext`'s generation is a memo over the host's session signal, and the
//! whole of why the cache cannot serve a mixed object is that A MEMO MARKED
//! DIRTY BY A `.set()` RECOMPUTES ON THE NEXT READ — tracked or untracked, in
//! the same tick, with no effect in between. Every accessor in
//! `context.rs` compares its cached `built_for` against that read. If the
//! reactive layer ever stopped doing this, the accessors would go on compiling
//! and quietly serve the departed session's queries paired with the arriving
//! session's context, which is the defect the memo replaced a counter to close.
//!
//! So the fact is pinned here rather than described. The components themselves
//! cannot be: the crate is `cfg(target_arch = "wasm32")`, its surfaces need a
//! browser and a live ankurah node, and this workspace has no wasm harness.
//! What CAN run on the host is the reactive layer underneath — the same
//! `reactive_graph` the components' leptos resolves to, since the version
//! requirement here unifies with leptos's own.
//!
//! `Session` itself is not reachable from a native build (it holds an
//! `ankurah::Context`), so the payload below stands in for it: clonable,
//! carrying no equality of any kind, exactly like the pair the host sets.

#![cfg(not(target_arch = "wasm32"))]

use reactive_graph::computed::ArcMemo;
use reactive_graph::owner::Owner;
use reactive_graph::prelude::*;
use reactive_graph::signal::RwSignal;
use reactive_graph::wrappers::read::Signal;

/// A stand-in for `ankurah::Context`: cloneable, and comparable by nothing.
#[derive(Clone)]
struct Ctx(#[allow(dead_code)] u32);

/// The shape of `ankurah_chat_leptos::Session`.
type Session = (Ctx, Option<u64>);

/// `ChatContextBuilder::build`'s generation, verbatim — A COPY, because the
/// crate's lib is empty on the host, so nothing mechanical ties the two.
/// `build()` carries the matching pointer; whoever changes either closure
/// changes both, and a `prev`-shape that could ever repeat a value breaks the
/// swap effect over there (it re-runs only when the memo's value CHANGED).
fn version(session: Signal<Session>) -> ArcMemo<u64> {
    ArcMemo::new(move |prev: Option<&u64>| {
        session.track();
        prev.map_or(0, |p| p + 1)
    })
}

#[test]
fn a_set_is_visible_to_the_very_next_read() {
    let owner = Owner::new();
    owner.set();

    let host = RwSignal::new((Ctx(1), None));
    let generation = version(host.into());

    assert_eq!(generation.get_untracked(), 0);
    assert_eq!(generation.get_untracked(), 0, "a read with no set between does not move it");

    // No tick boundary here: this is what an accessor called from a click
    // handler after the host signed someone in sees.
    host.set((Ctx(2), Some(7)));
    assert_eq!(generation.get_untracked(), 1, "an untracked read of a dirty memo recomputes");
}

#[test]
fn sets_in_one_tick_coalesce_to_the_last_of_them() {
    let owner = Owner::new();
    owner.set();

    let host = RwSignal::new((Ctx(1), None));
    let generation = version(host.into());
    assert_eq!(generation.get_untracked(), 0);

    host.set((Ctx(2), Some(7)));
    host.set((Ctx(3), Some(8)));
    assert_eq!(generation.get_untracked(), 1, "two sets nobody read between are one observed change");
}

#[test]
fn a_set_before_anything_has_read_is_absorbed_into_the_first_generation() {
    let owner = Owner::new();
    owner.set();

    let host = RwSignal::new((Ctx(1), None));
    let generation = version(host.into());

    // The mount tick: the host resolves a stored token and sets the session
    // before any accessor or effect has run. Nothing was built against the
    // session that departed, so nothing is stale, and the number stays 0.
    host.set((Ctx(2), Some(7)));
    assert_eq!(generation.get_untracked(), 0);
    host.set((Ctx(3), Some(8)));
    assert_eq!(generation.get_untracked(), 1, "and it moves from there like any other");
}

#[test]
fn a_host_that_moves_one_half_moves_the_version_twice() {
    let owner = Owner::new();
    owner.set();

    // The arrangement `ChatContext::new` asks hosts NOT to build: the context
    // and the reader in separate signals, assembled on read. The pair tears at
    // the source, and this is what the components then see — two sessions, the
    // first of them a new context beside the previous reader.
    let ctx = RwSignal::new(Ctx(1));
    let viewer = RwSignal::new(None::<u64>);
    let generation = version(Signal::derive(move || (ctx.get(), viewer.get())));

    assert_eq!(generation.get_untracked(), 0);
    ctx.set(Ctx(2));
    assert_eq!(generation.get_untracked(), 1);
    viewer.set(Some(7));
    assert_eq!(generation.get_untracked(), 2);
}

#[test]
fn a_fixed_session_never_moves() {
    let owner = Owner::new();
    owner.set();

    // A host whose session never changes hands over the bare pair, which the
    // blanket `From<T>` wraps in a signal with nothing behind it to track.
    let generation = version(Signal::from((Ctx(1), Some(7u64))));

    assert_eq!(generation.get_untracked(), 0);
    assert_eq!(generation.get_untracked(), 0);
}
