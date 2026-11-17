use std::collections::BTreeMap;

use crate::theme;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    Archival,
    Stabilized,
    /// EDIT_MASTER*.mkv (non-VD) — out of Kdenlive, ready for VDecimate or Viewer Encode
    EditMaster,
    /// EDIT_MASTER*_VD.mkv — VDecimate already applied, ready for Viewer Encode or Anime Upscale
    EditMasterVD,
    Viewer,
}

impl FileKind {
    /// The section and badge name.
    pub fn label(&self) -> &'static str {
        match self {
            FileKind::Viewer => "Viewer",
            FileKind::EditMasterVD => "Edit Master (VD)",
            FileKind::EditMaster => "Edit Master",
            FileKind::Stabilized => "Stabilized",
            FileKind::Archival => "Archival",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LibraryEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: FileKind,
}

pub struct Library {
    pub entries: Vec<LibraryEntry>,
    /// Index into `entries` of the currently selected file (if any).
    pub selected: Option<usize>,
    /// Sections the user has opened or closed by hand, by label. Anything not listed follows the
    /// default: open if it holds the selection (or if nothing is selected yet).
    section_overrides: BTreeMap<String, bool>,
}

/// What the user did in the list this frame.
#[derive(Default)]
pub struct LibraryEvent {
    /// A row was clicked: open it in the player.
    pub open: Option<LibraryEntry>,
    /// A section header was clicked (the caller saves the layout).
    pub sections_changed: bool,
}

/// Prefixes the section already says, dropped from a row's name.
const KNOWN_PREFIXES: [&str; 3] = ["VHS_ARCHIVAL_", "EDIT_MASTER-", "EDIT_MASTER_"];

/// The file name without the prefix its section already states.
pub fn strip_known_prefix(name: &str) -> &str {
    KNOWN_PREFIXES
        .iter()
        .find_map(|p| name.strip_prefix(p))
        .unwrap_or(name)
}

/// `text` shortened with a `…` in the middle until `measure` says it fits in `max_width`, keeping
/// the start (a date) and the end (the processing suffix and extension), which are what tell files
/// apart: `2026-11-15_18-06-23_20-37-48_STABLE_QTGMC.mkv` becomes `2026-11-15…_STABLE_QTGMC.mkv`.
pub fn elide_middle(text: &str, max_width: f32, measure: &dyn Fn(&str) -> f32) -> String {
    if measure(text) <= max_width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    for kept in (2..chars.len()).rev() {
        let head = (((kept as f32) * 0.34).ceil() as usize).clamp(1, kept - 1);
        let tail = kept - head;
        let candidate: String = chars[..head]
            .iter()
            .chain(std::iter::once(&'…'))
            .chain(chars[chars.len() - tail..].iter())
            .collect();
        if measure(&candidate) <= max_width {
            return candidate;
        }
    }
    "…".to_owned()
}

impl Library {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            section_overrides: BTreeMap::new(),
        }
    }

    pub fn refresh(&mut self, cfg: &crate::config::Config) {
        self.entries.clear();
        self.selected = None; // stale index is invalid after rescan
        self.scan_dir(&cfg.viewer_dir(), FileKind::Viewer);
        self.scan_stabilized_dir(&cfg.stabilized_dir());
        self.scan_dir(&cfg.archival_dir(), FileKind::Archival);
    }

    /// Scan `captures/stabilized/` and partition into three sections:
    ///
    /// 1. EditMasterVD — `EDIT_MASTER*_VD.mkv`  (VDecimate done)
    /// 2. EditMaster   — `EDIT_MASTER*.mkv`      (out of Kdenlive, not yet VDecimated)
    /// 3. Stabilized   — everything else          (denoised/QTGMC intermediates)
    ///
    /// Appended in that order so the library list reads Viewer → VD → EditMaster → Stabilized → Archival.
    fn scan_stabilized_dir(&mut self, dir: &std::path::Path) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut vd_entries: Vec<_> = Vec::new();
        let mut em_entries: Vec<_> = Vec::new();
        let mut st_entries: Vec<_> = Vec::new();

        let mut all: Vec<_> = rd
            .filter_map(|e| e.ok())
            .filter(|e| {
                let p = e.path();
                matches!(p.extension().and_then(|s| s.to_str()), Some("mkv" | "mp4"))
            })
            .collect();
        all.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));

        for e in all {
            let path = e.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_owned();
            if name.starts_with("EDIT_MASTER") && name.ends_with("_VD.mkv") {
                vd_entries.push(LibraryEntry {
                    path,
                    name,
                    kind: FileKind::EditMasterVD,
                });
            } else if name.starts_with("EDIT_MASTER") {
                em_entries.push(LibraryEntry {
                    path,
                    name,
                    kind: FileKind::EditMaster,
                });
            } else {
                st_entries.push(LibraryEntry {
                    path,
                    name,
                    kind: FileKind::Stabilized,
                });
            }
        }

        self.entries.extend(vd_entries);
        self.entries.extend(em_entries);
        self.entries.extend(st_entries);
    }

    fn scan_dir(&mut self, dir: &std::path::Path, kind: FileKind) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd
            .filter_map(|e| e.ok())
            .filter(|e| {
                let p = e.path();
                matches!(p.extension().and_then(|s| s.to_str()), Some("mkv" | "mp4"))
            })
            .collect();
        // Newest first
        entries.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
        for e in entries {
            let path = e.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_owned();
            self.entries.push(LibraryEntry {
                path,
                name,
                kind: kind.clone(),
            });
        }
    }

    /// Returns the currently selected entry, if any.
    pub fn selected_entry(&self) -> Option<&LibraryEntry> {
        self.selected.and_then(|i| self.entries.get(i))
    }

    /// Whether a section shows its files: what the user chose, else open when it holds the
    /// selection, or when nothing is selected yet.
    pub fn section_open(&self, kind: &FileKind) -> bool {
        if let Some(&open) = self.section_overrides.get(kind.label()) {
            return open;
        }
        match self.selected_entry() {
            Some(e) => &e.kind == kind,
            None => true,
        }
    }

    /// The hand-set sections, for the settings file.
    pub fn section_overrides(&self) -> &BTreeMap<String, bool> {
        &self.section_overrides
    }

    pub fn set_section_overrides(&mut self, overrides: BTreeMap<String, bool>) {
        self.section_overrides = overrides;
    }

    /// Render the file list in the redesign's shape: a header row per section (caret, name, count)
    /// that folds it, and 24 px file rows with the prefix the section already says dropped and the
    /// middle of a long name elided (the full name is the hover text and shows in the toolbar).
    pub fn show(&mut self, ui: &mut egui::Ui) -> LibraryEvent {
        let mut event = LibraryEvent::default();
        let font = egui::FontId::monospace(11.0);
        let mut last_kind: Option<FileKind> = None;
        let mut toggle: Option<FileKind> = None;
        for (i, entry) in self.entries.iter().enumerate() {
            if last_kind.as_ref() != Some(&entry.kind) {
                last_kind = Some(entry.kind.clone());
                let count = self.entries.iter().filter(|e| e.kind == entry.kind).count();
                let open = self.section_open(&entry.kind);
                if section_header(ui, entry.kind.label(), count, open).clicked() {
                    toggle = Some(entry.kind.clone());
                }
            }
            if !self.section_open(&entry.kind) {
                continue;
            }
            let selected = self.selected == Some(i);
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), theme::size::LIST_ROW_HEIGHT),
                egui::Sense::click(),
            );
            let fill = if selected {
                theme::color::SELECTION
            } else if resp.hovered() {
                theme::color::WIDGET_HOVERED
            } else {
                theme::color::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 0.0, fill);
            let fg = if selected {
                theme::color::TEXT_STRONG
            } else {
                theme::color::TEXT_NORMAL
            };
            let indent = 23.0;
            let max_w = (rect.width() - indent - 8.0).max(20.0);
            let painter = ui.painter().clone();
            let measure = |s: &str| {
                painter
                    .layout_no_wrap(s.to_owned(), font.clone(), fg)
                    .size()
                    .x
            };
            let text = elide_middle(strip_known_prefix(&entry.name), max_w, &measure);
            ui.painter().text(
                egui::pos2(rect.left() + indent, rect.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                font.clone(),
                fg,
            );
            if resp.on_hover_text(&entry.name).clicked() {
                self.selected = Some(i);
                event.open = Some(entry.clone());
            }
        }
        if let Some(kind) = toggle {
            let open = self.section_open(&kind);
            self.section_overrides
                .insert(kind.label().to_owned(), !open);
            event.sections_changed = true;
        }
        event
    }
}

/// A 22 px section header row: caret, name in the section style, and the file count on the right.
fn section_header(ui: &mut egui::Ui, label: &str, count: usize, open: bool) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, theme::color::WIDGET_HOVERED.gamma_multiply(0.5));
    }
    let caret = if open {
        crate::icons::SECTION_OPEN
    } else {
        crate::icons::SECTION_CLOSED
    };
    let p = ui.painter();
    p.text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        caret,
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
        theme::color::TEXT_WEAK,
    );
    p.text(
        egui::pos2(rect.left() + 26.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label.to_uppercase(),
        egui::FontId::new(10.5, egui::FontFamily::Name(theme::FAMILY_BOLD.into())),
        theme::color::TEXT_WEAK,
    );
    p.text(
        egui::pos2(rect.right() - 10.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::monospace(11.0),
        theme::color::TEXT_WEAK,
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 7 px per character: a stand-in for the monospace measure.
    fn mono7(s: &str) -> f32 {
        s.chars().count() as f32 * 7.0
    }

    fn lib_with(kinds: &[FileKind], selected: Option<usize>) -> Library {
        let mut l = Library::new();
        for (i, k) in kinds.iter().enumerate() {
            l.entries.push(LibraryEntry {
                path: PathBuf::from(format!("/x/{i}.mkv")),
                name: format!("{i}.mkv"),
                kind: k.clone(),
            });
        }
        l.selected = selected;
        l
    }

    #[test]
    fn the_section_prefix_is_dropped() {
        assert_eq!(
            strip_known_prefix("VHS_ARCHIVAL_2026-11-15_18-06-23.mkv"),
            "2026-11-15_18-06-23.mkv"
        );
        assert_eq!(
            strip_known_prefix("EDIT_MASTER-WILLOW_VD.mkv"),
            "WILLOW_VD.mkv"
        );
        assert_eq!(strip_known_prefix("EDIT_MASTER_x.mkv"), "x.mkv");
        assert_eq!(strip_known_prefix("tape.mkv"), "tape.mkv");
    }

    #[test]
    fn a_name_that_fits_is_left_alone() {
        assert_eq!(elide_middle("tape.mkv", 100.0, &mono7), "tape.mkv");
    }

    #[test]
    fn a_long_name_keeps_its_date_and_its_suffix() {
        let name = strip_known_prefix("VHS_ARCHIVAL_2026-11-15_18-06-23_20-37-48_STABLE_QTGMC.mkv");
        let out = elide_middle(name, 28.0 * 7.0, &mono7);
        assert!(mono7(&out) <= 28.0 * 7.0, "{out}");
        assert!(out.starts_with("2026-11-15"), "{out}");
        assert!(out.ends_with("QTGMC.mkv"), "{out}");
        assert_eq!(out.matches('…').count(), 1);
        // The two files that differ only in the suffix stay distinguishable.
        let other = elide_middle(
            "2026-11-15_18-06-23_20-37-48_STABLE.mkv",
            28.0 * 7.0,
            &mono7,
        );
        assert_ne!(out, other);
    }

    #[test]
    fn a_tiny_width_still_gives_something() {
        assert_eq!(elide_middle("abcdef.mkv", 1.0, &mono7), "…");
        assert!(mono7(&elide_middle("abcdef.mkv", 5.0 * 7.0, &mono7)) <= 35.0);
    }

    #[test]
    fn with_nothing_selected_every_section_is_open() {
        let l = lib_with(&[FileKind::Viewer, FileKind::Archival], None);
        assert!(l.section_open(&FileKind::Viewer) && l.section_open(&FileKind::Archival));
    }

    #[test]
    fn with_a_selection_only_its_section_is_open_until_the_user_decides() {
        let mut l = lib_with(&[FileKind::Viewer, FileKind::Archival], Some(1));
        assert!(!l.section_open(&FileKind::Viewer));
        assert!(l.section_open(&FileKind::Archival));
        l.set_section_overrides(BTreeMap::from([
            ("Viewer".to_owned(), true),
            ("Archival".to_owned(), false),
        ]));
        assert!(
            l.section_open(&FileKind::Viewer),
            "a hand-opened section stays open"
        );
        assert!(
            !l.section_open(&FileKind::Archival),
            "and a hand-closed one stays closed"
        );
        assert_eq!(l.section_overrides().len(), 2);
    }
}
