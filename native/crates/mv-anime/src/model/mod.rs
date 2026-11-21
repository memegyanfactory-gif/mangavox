//! The engine's data model (design 5.3): plain serde structs, one file per area, all
//! re-exported here. Every top-level entity carries `schema` and an `extra` map so fields
//! written by a newer app survive a round-trip through an older one (design 5.2).

mod bible;
mod card;
mod enums;
mod series;
mod shot;
mod story;

pub use bible::*;
pub use card::*;
pub use enums::*;
pub use series::*;
pub use shot::*;
pub use story::*;

/// Version of every top-level entity file written by this build. A file with a higher number
/// opens read-only ("Made with a newer MangaVox; update to edit").
pub const SCHEMA: u32 = 1;
