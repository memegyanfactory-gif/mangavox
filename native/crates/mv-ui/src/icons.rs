//! Phosphor Icons (MIT, @phosphor-icons/core 2.1.1) as typed, compile-checked handles.
//!
//! One list drives everything: it generates the `ph::NAME` consts, embeds every SVG in all four weights
//! with `include_bytes!` (a missing file fails the build) and is what `native/scripts/vendor-phosphor.ps1`
//! reads when it vendors the files. To add an icon: add a line to the list, run the script, rebuild.
//!
//! GPUI keeps only the alpha channel of an SVG, so every icon is a one-colour mask tinted by `text_color`.
//! The weight is chosen by which file you load:
//! - Regular: default chrome at 16px and above.
//! - Bold: 14px and below (chips, badges, status rows), where regular goes faint.
//! - Fill: any on/saved/selected/current state, and solid transport glyphs.
//! - Duotone: soft two-tone hero and empty-state glyphs (its 20% layer becomes a tint of the same colour).
//!
//! `icon(ph::STAR.on(fav), colour, 14.)` works with every helper that takes a `&'static str` path.
use gpui_kit::{Rgba, Svg};

use crate::components::icon;

/// Icon weight: which of the four vendored SVG files to load.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Weight {
    Regular,
    Bold,
    Fill,
    Duotone,
}

/// One Phosphor icon: the asset path of each weight. Use the consts in [`ph`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ph {
    regular: &'static str,
    bold: &'static str,
    fill: &'static str,
    duotone: &'static str,
}

impl Ph {
    /// Asset path at an explicit weight.
    pub const fn path(self, w: Weight) -> &'static str {
        match w {
            Weight::Regular => self.regular,
            Weight::Bold => self.bold,
            Weight::Fill => self.fill,
            Weight::Duotone => self.duotone,
        }
    }

    /// Regular when off, fill when on: the favourite / saved / selected / current convention.
    pub const fn on(self, active: bool) -> &'static str {
        if active {
            self.fill
        } else {
            self.regular
        }
    }

    pub const fn regular(self) -> &'static str {
        self.regular
    }

    pub const fn bold(self) -> &'static str {
        self.bold
    }

    pub const fn fill(self) -> &'static str {
        self.fill
    }

    pub const fn duotone(self) -> &'static str {
        self.duotone
    }
}

/// Phosphor icon at an explicit weight, e.g. `ph_icon(ph::STAR, Weight::Fill, Tokens::warning(), 14.)`.
pub fn ph_icon(i: Ph, w: Weight, color: Rgba, px_size: f32) -> Svg {
    icon(i.path(w), color, px_size)
}

/// Phosphor icon with the weight picked by size: bold at 14px and below, regular above.
pub fn icon_ph(i: Ph, color: Rgba, px_size: f32) -> Svg {
    ph_icon(i, if px_size <= 14. { Weight::Bold } else { Weight::Regular }, color, px_size)
}

macro_rules! phosphor {
    ($($id:ident = $f:literal),* $(,)?) => {
        /// One const per vendored icon, named after the Phosphor icon in SCREAMING_SNAKE
        /// (`caret-down` is `ph::CARET_DOWN`). A typo fails to compile.
        pub mod ph {
            use super::Ph;

            $(pub const $id: Ph = Ph {
                regular: concat!("ph/regular/", $f, ".svg"),
                bold: concat!("ph/bold/", $f, ".svg"),
                fill: concat!("ph/fill/", $f, ".svg"),
                duotone: concat!("ph/duotone/", $f, ".svg"),
            };)*

            /// Every vendored icon.
            pub const ALL: &[Ph] = &[$($id),*];
        }

        /// Every vendored SVG as `(asset path, bytes)`, four weights per icon.
        pub(crate) const EMBED: &[(&str, &[u8])] = &[$(
            (concat!("ph/regular/", $f, ".svg"), include_bytes!(concat!("../assets/ph/regular/", $f, ".svg"))),
            (concat!("ph/bold/", $f, ".svg"), include_bytes!(concat!("../assets/ph/bold/", $f, ".svg"))),
            (concat!("ph/fill/", $f, ".svg"), include_bytes!(concat!("../assets/ph/fill/", $f, ".svg"))),
            (concat!("ph/duotone/", $f, ".svg"), include_bytes!(concat!("../assets/ph/duotone/", $f, ".svg"))),
        )*];
    };
}

phosphor! {
    // vendor-phosphor:begin
    // Arrows and carets
    CARET_DOWN = "caret-down", CARET_UP = "caret-up", CARET_LEFT = "caret-left", CARET_RIGHT = "caret-right",
    CARET_UP_DOWN = "caret-up-down", CARET_DOUBLE_LEFT = "caret-double-left",
    CARET_DOUBLE_RIGHT = "caret-double-right", ARROW_LEFT = "arrow-left", ARROW_RIGHT = "arrow-right",
    ARROW_UP = "arrow-up", ARROW_DOWN = "arrow-down", ARROW_UP_RIGHT = "arrow-up-right",
    ARROW_DOWN_RIGHT = "arrow-down-right", ARROW_LINE_LEFT = "arrow-line-left", ARROW_LINE_RIGHT = "arrow-line-right",
    ARROW_LINE_DOWN = "arrow-line-down", ARROWS_LEFT_RIGHT = "arrows-left-right", ARROWS_DOWN_UP = "arrows-down-up",
    ARROW_CIRCLE_UP = "arrow-circle-up", ARROW_CIRCLE_DOWN = "arrow-circle-down",
    ARROW_SQUARE_OUT = "arrow-square-out", ARROW_U_UP_LEFT = "arrow-u-up-left", ARROW_U_UP_RIGHT = "arrow-u-up-right",
    ARROW_CLOCKWISE = "arrow-clockwise", ARROW_COUNTER_CLOCKWISE = "arrow-counter-clockwise",
    ARROWS_CLOCKWISE = "arrows-clockwise", ARROWS_OUT = "arrows-out", ARROWS_IN = "arrows-in",
    ARROWS_OUT_SIMPLE = "arrows-out-simple", ARROWS_IN_SIMPLE = "arrows-in-simple", CORNERS_OUT = "corners-out",
    CORNERS_IN = "corners-in", FRAME_CORNERS = "frame-corners", SORT_ASCENDING = "sort-ascending",
    SORT_DESCENDING = "sort-descending", SWAP = "swap", EXPORT = "export", SIGN_OUT = "sign-out", SIGN_IN = "sign-in",
    // Status and actions
    CHECK = "check", CHECKS = "checks", CHECK_CIRCLE = "check-circle", CHECK_SQUARE = "check-square",
    CHECK_FAT = "check-fat", X = "x", X_CIRCLE = "x-circle", X_SQUARE = "x-square", PLUS = "plus",
    PLUS_CIRCLE = "plus-circle", PLUS_SQUARE = "plus-square", MINUS = "minus", MINUS_CIRCLE = "minus-circle",
    MINUS_SQUARE = "minus-square", PROHIBIT = "prohibit", WARNING = "warning", WARNING_CIRCLE = "warning-circle",
    WARNING_OCTAGON = "warning-octagon", WARNING_DIAMOND = "warning-diamond", INFO = "info", QUESTION = "question",
    SEAL_CHECK = "seal-check", SPINNER = "spinner", CIRCLE_NOTCH = "circle-notch", CIRCLE_DASHED = "circle-dashed",
    RADIO_BUTTON = "radio-button", DOT = "dot", DOTS_THREE = "dots-three",
    DOTS_THREE_VERTICAL = "dots-three-vertical", DOTS_SIX_VERTICAL = "dots-six-vertical",
    // Media and audio
    PLAY = "play", PAUSE = "pause", STOP = "stop", PLAY_CIRCLE = "play-circle", PAUSE_CIRCLE = "pause-circle",
    STOP_CIRCLE = "stop-circle", SKIP_FORWARD = "skip-forward", SKIP_BACK = "skip-back",
    FAST_FORWARD = "fast-forward", REWIND = "rewind", SHUFFLE = "shuffle", REPEAT = "repeat",
    REPEAT_ONCE = "repeat-once", PLAY_PAUSE = "play-pause", PLAYLIST = "playlist", QUEUE = "queue",
    SPEAKER_HIGH = "speaker-high", SPEAKER_LOW = "speaker-low", SPEAKER_X = "speaker-x",
    SPEAKER_SLASH = "speaker-slash", WAVEFORM = "waveform", MICROPHONE = "microphone",
    MICROPHONE_SLASH = "microphone-slash", MICROPHONE_STAGE = "microphone-stage", HEADPHONES = "headphones",
    MUSIC_NOTES = "music-notes", MUSIC_NOTE = "music-note", EQUALIZER = "equalizer",
    CLOSED_CAPTIONING = "closed-captioning", SUBTITLES = "subtitles", SUBTITLES_SLASH = "subtitles-slash",
    HIGH_DEFINITION = "high-definition", FILM_SLATE = "film-slate", FILM_STRIP = "film-strip",
    VIDEO_CAMERA = "video-camera", MONITOR_PLAY = "monitor-play", TELEVISION = "television",
    TELEVISION_SIMPLE = "television-simple", PICTURE_IN_PICTURE = "picture-in-picture", TIMER = "timer",
    // Reading and layout
    BOOK_OPEN = "book-open", BOOK_OPEN_TEXT = "book-open-text", BOOKS = "books", BOOK_BOOKMARK = "book-bookmark",
    BOOKMARK_SIMPLE = "bookmark-simple", BOOKMARKS_SIMPLE = "bookmarks-simple", NEWSPAPER = "newspaper",
    NOTE_PENCIL = "note-pencil", CARDS = "cards", STACK = "stack", ROWS = "rows", COLUMNS = "columns",
    SQUARES_FOUR = "squares-four", GRID_FOUR = "grid-four", LAYOUT = "layout", SIDEBAR = "sidebar",
    SIDEBAR_SIMPLE = "sidebar-simple", SQUARE_HALF = "square-half", SQUARE_HALF_BOTTOM = "square-half-bottom",
    LIST = "list", LIST_BULLETS = "list-bullets", LIST_CHECKS = "list-checks", LIST_NUMBERS = "list-numbers",
    TEXT_AA = "text-aa", TEXT_T = "text-t",
    // People, emotion and genres
    USER = "user", USER_CIRCLE = "user-circle", USER_PLUS = "user-plus", USER_GEAR = "user-gear",
    USER_FOCUS = "user-focus", USERS = "users", SMILEY = "smiley", SMILEY_SAD = "smiley-sad",
    SMILEY_WINK = "smiley-wink", SMILEY_MEH = "smiley-meh", SMILEY_NERVOUS = "smiley-nervous",
    SMILEY_ANGRY = "smiley-angry", SMILEY_BLANK = "smiley-blank", SCAN_SMILEY = "scan-smiley",
    MASK_HAPPY = "mask-happy", MASK_SAD = "mask-sad", GHOST = "ghost", SKULL = "skull", DETECTIVE = "detective",
    EAR = "ear", EYE = "eye", EYE_SLASH = "eye-slash", EYE_CLOSED = "eye-closed", HEART = "heart",
    HEART_STRAIGHT = "heart-straight", HEART_BREAK = "heart-break", HEARTBEAT = "heartbeat", STAR = "star",
    STAR_FOUR = "star-four", STAR_HALF = "star-half", FIRE = "fire", LIGHTNING = "lightning", SPARKLE = "sparkle",
    MAGIC_WAND = "magic-wand", CROWN_SIMPLE = "crown-simple", TROPHY = "trophy", MEDAL = "medal", FLAG = "flag",
    SWORD = "sword", SHIELD = "shield", MOUNTAINS = "mountains", ROCKET_LAUNCH = "rocket-launch",
    THUMBS_UP = "thumbs-up", THUMBS_DOWN = "thumbs-down",
    // AI and system
    ROBOT = "robot", BRAIN = "brain", HEAD_CIRCUIT = "head-circuit", GEAR_SIX = "gear-six", GEAR = "gear",
    SLIDERS = "sliders", SLIDERS_HORIZONTAL = "sliders-horizontal", CPU = "cpu", MEMORY = "memory",
    HARD_DRIVE = "hard-drive", HARD_DRIVES = "hard-drives", DATABASE = "database", GAUGE = "gauge",
    SPEEDOMETER = "speedometer", PULSE = "pulse", PLUG = "plug", PLUGS = "plugs", PLUGS_CONNECTED = "plugs-connected",
    POWER = "power", WIFI_HIGH = "wifi-high", WIFI_SLASH = "wifi-slash", CLOUD = "cloud",
    CLOUD_ARROW_DOWN = "cloud-arrow-down", CLOUD_ARROW_UP = "cloud-arrow-up", WRENCH = "wrench", BUG = "bug",
    CODE = "code", TERMINAL = "terminal", TERMINAL_WINDOW = "terminal-window", COMMAND = "command",
    KEYBOARD = "keyboard", KEY = "key", LOCK = "lock", LOCK_OPEN = "lock-open", LOCK_SIMPLE = "lock-simple",
    LOCK_SIMPLE_OPEN = "lock-simple-open", SHIELD_CHECK = "shield-check", SHIELD_WARNING = "shield-warning",
    SHIELD_SLASH = "shield-slash", FINGERPRINT = "fingerprint", BELL = "bell", ENVELOPE = "envelope",
    PAPER_PLANE_TILT = "paper-plane-tilt", CLIPBOARD_TEXT = "clipboard-text", CHART_BAR = "chart-bar",
    CHART_LINE_UP = "chart-line-up",
    // Files, tools and misc
    LINK_SIMPLE = "link-simple", LINK = "link", LINK_BREAK = "link-break", GLOBE = "globe",
    GLOBE_SIMPLE = "globe-simple", TRANSLATE = "translate", PALETTE = "palette", PAINT_BRUSH = "paint-brush",
    SWATCHES = "swatches", SUN = "sun", MOON = "moon", CAMERA = "camera", IMAGE = "image", IMAGES = "images",
    IMAGE_BROKEN = "image-broken", CROP = "crop", SCISSORS = "scissors", ERASER = "eraser", BROOM = "broom",
    LIGHTBULB = "lightbulb", GITHUB_LOGO = "github-logo", PUZZLE_PIECE = "puzzle-piece", PACKAGE = "package",
    CUBE = "cube", DESKTOP = "desktop", MONITOR = "monitor", DEVICE_MOBILE = "device-mobile",
    APP_WINDOW = "app-window", TRAY = "tray", FOLDER = "folder", FOLDER_SIMPLE = "folder-simple",
    FOLDER_OPEN = "folder-open", FOLDER_SIMPLE_PLUS = "folder-simple-plus", FILE = "file", FILE_TEXT = "file-text",
    FILE_VIDEO = "file-video", FILE_AUDIO = "file-audio", FILE_IMAGE = "file-image",
    DOWNLOAD_SIMPLE = "download-simple", UPLOAD_SIMPLE = "upload-simple", DOWNLOAD = "download", TRASH = "trash",
    TRASH_SIMPLE = "trash-simple", COPY = "copy", PENCIL_SIMPLE = "pencil-simple", FUNNEL = "funnel",
    FUNNEL_X = "funnel-x", MAGNIFYING_GLASS = "magnifying-glass", MAGNIFYING_GLASS_MINUS = "magnifying-glass-minus",
    TOGGLE_LEFT = "toggle-left", TOGGLE_RIGHT = "toggle-right", PAPERCLIP = "paperclip", LIFEBUOY = "lifebuoy",
    COMPASS = "compass", HOUSE_SIMPLE = "house-simple", CLOCK = "clock",
    CLOCK_COUNTER_CLOCKWISE = "clock-counter-clockwise", CLOCK_COUNTDOWN = "clock-countdown",
    CLOCK_AFTERNOON = "clock-afternoon", HOURGLASS = "hourglass", HOURGLASS_MEDIUM = "hourglass-medium",
    CALENDAR_BLANK = "calendar-blank", CALENDAR_CHECK = "calendar-check", CALENDAR_DOTS = "calendar-dots",
    TAG = "tag", TAG_SIMPLE = "tag-simple", TRAFFIC_CONE = "traffic-cone", BINOCULARS = "binoculars",
    FEATHER = "feather", SHARE_NETWORK = "share-network", CHAT_CIRCLE = "chat-circle",
    CHAT_CIRCLE_DOTS = "chat-circle-dots", CHATS_CIRCLE = "chats-circle", CHAT_TEARDROP = "chat-teardrop",
    SCAN = "scan", GIT_BRANCH = "git-branch", QR_CODE = "qr-code",

    // vendor-phosphor:end
}

/// One glyph per meaning, so a destination or action looks the same on every page and in every
/// section (the sidebar, a page title and an empty state all say "History" with one clock).
/// Use these before reaching for a raw `ph::` const. Apple HIG SF Symbols: use one symbol for one
/// meaning: <https://developer.apple.com/design/human-interface-guidelines/sf-symbols>
pub mod glyphs {
    use super::{ph, Ph};

    // Destinations.
    pub const HOME: Ph = ph::HOUSE_SIMPLE;
    pub const BROWSE: Ph = ph::COMPASS;
    /// Same meaning as [`BROWSE`]: explore what is out there.
    pub const DISCOVER: Ph = ph::COMPASS;
    pub const LIBRARY: Ph = ph::BOOKS;
    pub const SEARCH: Ph = ph::MAGNIFYING_GLASS;
    /// Favourites are stars everywhere (filled gold when on: `favorite_star`).
    pub const FAVORITES: Ph = ph::STAR;
    pub const HISTORY: Ph = ph::CLOCK_COUNTER_CLOCKWISE;
    pub const DOWNLOADS: Ph = ph::DOWNLOAD_SIMPLE;
    pub const MANGA: Ph = ph::BOOK_OPEN;
    pub const ANIME: Ph = ph::TELEVISION_SIMPLE;
    pub const MOVIES: Ph = ph::FILM_SLATE;
    pub const STUDIO: Ph = ph::MAGIC_WAND;
    pub const ASSISTANT: Ph = ph::SPARKLE;
    pub const SOURCES: Ph = ph::PUZZLE_PIECE;
    pub const SETTINGS: Ph = ph::GEAR_SIX;

    // Things.
    /// Saved / in library / on the watchlist (fill weight when saved).
    pub const SAVED: Ph = ph::BOOKMARK_SIMPLE;
    pub const VOICE: Ph = ph::MICROPHONE;
    pub const MUSIC: Ph = ph::MUSIC_NOTES;
    pub const MODEL: Ph = ph::CUBE;
    pub const STORAGE: Ph = ph::HARD_DRIVE;
    pub const FOLDER: Ph = ph::FOLDER_SIMPLE;
    pub const IMAGE: Ph = ph::IMAGE;
    pub const SUBTITLES: Ph = ph::SUBTITLES;
    pub const LANGUAGE: Ph = ph::TRANSLATE;

    // Studio phases.
    pub const READ: Ph = ph::SCAN;
    pub const STORY: Ph = ph::NOTE_PENCIL;
    pub const NARRATE: Ph = ph::MICROPHONE;
    pub const VIDEO: Ph = ph::FILM_STRIP;

    // Actions.
    pub const ADD: Ph = ph::PLUS;
    pub const EDIT: Ph = ph::PENCIL_SIMPLE;
    pub const DELETE: Ph = ph::TRASH;
    pub const MORE: Ph = ph::DOTS_THREE;
    pub const CLOSE: Ph = ph::X;
    pub const BACK: Ph = ph::ARROW_LEFT;
    pub const REFRESH: Ph = ph::ARROWS_CLOCKWISE;
    pub const RETRY: Ph = ph::ARROW_CLOCKWISE;
    pub const FILTER: Ph = ph::FUNNEL;
    pub const SORT: Ph = ph::SORT_ASCENDING;
    pub const COPY: Ph = ph::COPY;
    pub const DOWNLOAD: Ph = ph::DOWNLOAD_SIMPLE;
    pub const UPLOAD: Ph = ph::UPLOAD_SIMPLE;
    /// Opens something outside the app (browser, Explorer).
    pub const EXTERNAL: Ph = ph::ARROW_SQUARE_OUT;
    pub const PLAY: Ph = ph::PLAY;
    pub const PAUSE: Ph = ph::PAUSE;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_map_to_distinct_paths() {
        let s = ph::STAR;
        assert_eq!(s.path(Weight::Regular), "ph/regular/star.svg");
        assert_eq!(s.path(Weight::Bold), "ph/bold/star.svg");
        assert_eq!(s.path(Weight::Fill), "ph/fill/star.svg");
        assert_eq!(s.path(Weight::Duotone), "ph/duotone/star.svg");
        assert_eq!(s.regular(), s.path(Weight::Regular));
        assert_eq!(s.bold(), s.path(Weight::Bold));
        assert_eq!(s.fill(), s.path(Weight::Fill));
        assert_eq!(s.duotone(), s.path(Weight::Duotone));
    }

    #[test]
    fn on_switches_between_regular_and_fill() {
        assert_eq!(ph::HEART.on(false), "ph/regular/heart.svg");
        assert_eq!(ph::HEART.on(true), "ph/fill/heart.svg");
    }

    #[test]
    fn multi_word_names_become_screaming_snake() {
        assert_eq!(ph::CARET_DOWN.regular(), "ph/regular/caret-down.svg");
        assert_eq!(ph::BOOKMARK_SIMPLE.fill(), "ph/fill/bookmark-simple.svg");
    }
}
