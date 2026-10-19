//! Shared UI foundation for every MangaVox page crate:
//! design tokens, small components, navigation and a background runtime.
//! See docs/RUST_REWRITE_PLAN.md §3 for the design rules.
pub mod assets;
pub mod bg;
pub mod components;
pub mod mode;
pub mod motion;
pub mod nav;
pub mod theme;
pub mod update;

pub use assets::Assets;
pub use components::*;
pub use motion::{dot_trail, progress_bar, pulse_dot, scan_overlay, spinner};
pub use nav::{back_target, navigate, Route};
pub use theme::{size, Tokens};
