//! Shared UI foundation for every MangaVox page crate:
//! design tokens, the component kit, navigation and a background runtime.
//! See docs/RUST_REWRITE_PLAN.md §3 for the design rules and COMPONENTS.md for the kit.
pub mod assets;
pub mod bg;
pub mod components;
pub mod controls;
pub mod feedback;
pub mod icons;
pub mod interact;
pub mod menu;
pub mod mode;
pub mod motion;
pub mod nav;
pub mod theme;
pub mod toast;
pub mod update;

pub use assets::Assets;
pub use components::*;
pub use controls::*;
pub use feedback::*;
pub use icons::{glyphs, icon_ph, ph, ph_icon, Ph, Weight};
pub use interact::{focus_ring_shadow, interactive, kbd, kbd_label, tip, tip_kbd, IconSrc, Interactive};
pub use menu::{menu_button, menu_row, overflow_menu, popup_surface, MenuButton, MenuItem, MenuRow};
pub use motion::{appear, dot_trail, progress_bar, pulse_dot, scan_overlay, shimmer, spinner, step_cards, StepMark};
pub use nav::{back_target, current_route, local_back, navigate, set_local_back, Route};
pub use theme::{size, Hue, RampExt, Tokens};
pub use toast::Toast;
