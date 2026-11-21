//! Local layout constants (design 4.4), taken from Studio and the Thumbnails generator.
//! Colours, type sizes, radii and the spacing scale come from `mv_ui::theme` only; these are the
//! few widths and heights that belong to this tab's frame. See UI-GUIDE.md for how to use them.


/// Left column of a step (Studio LIBRARY_W).
pub const LIST_W: f32 = 288.;
/// The left column below [`NARROW`], so the centre keeps a readable width.
pub const LIST_W_NARROW: f32 = 236.;
/// A row of a scene or shot list: title and one meta line, never squeezed.
pub const LIST_ROW_H: f32 = 52.;
/// Bulk actions that queue more than this many jobs ask first, with the count and the time.
pub const BULK_ASK_ABOVE: usize = 3;
/// Right column: the Generation panel or an inspector (thumb LEFT_W 272 + p MD).
pub const PANEL_W: f32 = 304.;
/// Below this window width the step bar shows icons only, the left column narrows and the right
/// panel becomes a drawer over the centre, opened on request. Chosen so that above it, with the
/// app's sidebar, the list and the panel side by side, the centre still has about 440 px.
pub const NARROW: f32 = 1320.;
/// Gallery tile; image 248 x 139.5 (16:9).
pub const TILE_W: f32 = 248.;
/// Reference tile square in the Generation panel.
pub const REF_TILE: f32 = 58.;
/// The step bar under the header.
pub const STEPBAR_H: f32 = 48.;
/// The project header.
pub const HEADER_H: f32 = 56.;
/// The queue strip folded to one row.
pub const QUEUE_H: f32 = 40.;
/// The queue strip opened to its list.
pub const QUEUE_OPEN_H: f32 = 220.;
/// Gap between the columns of a step and around them (Studio's 6 px page gutter).
pub const GUTTER: f32 = 6.;
/// Height of a column's header row ("SCENES  12").
pub const COLUMN_HEAD_H: f32 = 40.;
/// Width of the label column of an inspector row ("Lens", "Focus").
pub const ROW_LABEL_W: f32 = 64.;
/// History strip thumbnails (16:9).
pub const HISTORY_THUMB: (f32, f32) = (120., 67.5);
/// Poll interval of the page's one refresh task and of each Generation panel.
pub const POLL_MS: u64 = 300;
