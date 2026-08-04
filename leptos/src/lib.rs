//! Embeddable chat components (scaffold).
//!
//! Surfaces (each independently mountable): room selector, room log,
//! composer, DM thread view. Hosts provide the `ankurah::Context` and the
//! auth-demand callback; components render read-only without one. Empty
//! until the copy-out from community lands.
