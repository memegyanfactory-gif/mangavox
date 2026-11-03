//! mv-3d: the engine behind the Creative page's 3D tab (no UI).
//!
//! One manga chapter becomes one episode, made scene by scene:
//! READ (mv-recap's page reading and character brain) -> BREAKDOWN (the AI splits the chapter into
//! scenes and lists every location, character and prop they need) -> ASSETS (each one is reused
//! from the [`library`], fetched from a free online library, or modelled in Blender from the
//! manga's own panels) -> STAGE (the AI blocks the scene: placements, shots, actions) -> PREVIZ and
//! RENDER (Blender headless, `blender/assemble.py`) -> the user reviews the scene, approves it or
//! asks for a redo -> when every scene is approved the clips are joined into the episode.
//!
//! The AI is whichever model the user picked in the app, reached through `mv_ai::Registry`.
//! Everything lives under `<data>/studio/3d` (see [`paths`]).

mod ai;
pub mod blender;
mod breakdown;
mod build;
pub mod library;
mod online;
pub mod paths;
pub mod pipeline;
pub mod plan;
mod scripts;
mod stage;
mod util;

pub use library::{Asset, Category, Library, Origin};
pub use pipeline::{approve, episode_video, join_episode, live, load_episode, redo, run, stop, Ctx, Event, Live, RunOptions};
pub use plan::{Episode, SceneStatus};

#[cfg(test)]
mod tests;
