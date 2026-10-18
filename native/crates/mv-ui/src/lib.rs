//! Shared UI foundation for every MangaVox page crate:
//! design tokens, small components, navigation and a background runtime.
//! See docs/RUST_REWRITE_PLAN.md §3 for the design rules.
pub mod bg;
pub mod components;
pub mod nav;
pub mod theme;

pub use components::*;
pub use nav::{navigate, Route};
pub use theme::{size, Tokens};
