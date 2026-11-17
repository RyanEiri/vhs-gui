//! vhs-gui redesign: icon names. Phosphor Icons, Regular weight (MIT).
//!
//! `egui-phosphor = "0.12"` is the release that builds against egui 0.34. Each constant is a
//! `&str` holding one glyph; use it in a label or button like text: `format!("{} Reattach", PLUG)`.
//! Compile-checked against egui-phosphor 0.12.

use egui_phosphor::regular as ph;

// Rail
pub const CAPTURE: &str = ph::RECORD; // was ⏺ Monitor
pub const LIBRARY: &str = ph::FILM_STRIP; // new view (was part of ⬆ Upscale)
pub const UPSCALE: &str = ph::ARROWS_OUT; // was ⬆
pub const TOOLS: &str = ph::WRENCH; // was 🔧
pub const FOLDERS: &str = ph::FOLDER; // was 📁
pub const ABOUT: &str = ph::INFO; // was ℹ
pub const SETTINGS: &str = ph::SLIDERS_HORIZONTAL; // was ⚙, now a toolbar toggle in Upscale

// Transport and jobs
pub const PLAY: &str = ph::PLAY; // was ▶
pub const PAUSE: &str = ph::PAUSE; // was ⏸
pub const STOP: &str = ph::STOP;
pub const CANCEL: &str = ph::X; // was ✗
pub const FAILED: &str = ph::X_CIRCLE;
pub const OK: &str = ph::CHECK; // was ✓
pub const WARNING: &str = ph::WARNING; // was ⚠
pub const WORKING: &str = ph::CIRCLE_NOTCH; // or egui::Spinner
pub const TIME: &str = ph::CLOCK;
pub const REATTACH: &str = ph::PLUGS_CONNECTED;
pub const OPEN_LOG: &str = ph::FILE_TEXT;

// Files and panels
pub const TRASH: &str = ph::TRASH; // was 🗑
pub const REFRESH: &str = ph::ARROWS_CLOCKWISE; // was ⟳
pub const RESET: &str = ph::ARROW_COUNTER_CLOCKWISE; // was ↺
pub const SEARCH: &str = ph::MAGNIFYING_GLASS; // was 🔍
pub const LETTERBOX: &str = ph::CROP;
pub const VIEWER: &str = ph::EYE;
pub const SECTION_OPEN: &str = ph::CARET_DOWN;
pub const SECTION_CLOSED: &str = ph::CARET_RIGHT;
pub const BACK: &str = ph::CARET_LEFT; // was ◀
