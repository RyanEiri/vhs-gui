use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub enum FileKind {
    Archival,
    Stabilized,
    /// EDIT_MASTER*.mkv (non-VD) — out of Kdenlive, ready for VDecimate or Viewer Encode
    EditMaster,
    /// EDIT_MASTER*_VD.mkv — VDecimate already applied, ready for Viewer Encode or Anime Upscale
    EditMasterVD,
    Viewer,
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
}

impl Library {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
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

    /// Render the file list.  Returns `Some(entry)` when the user clicks a row
    /// (meaning: open it in the player).  Selection highlight is tracked internally.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<LibraryEntry> {
        let mut to_open = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut last_kind: Option<FileKind> = None;
            for (i, entry) in self.entries.iter().enumerate() {
                if last_kind.as_ref() != Some(&entry.kind) {
                    last_kind = Some(entry.kind.clone());
                    let label = match entry.kind {
                        FileKind::Viewer => "Viewer",
                        FileKind::EditMasterVD => "Edit Master (VD)",
                        FileKind::EditMaster => "Edit Master",
                        FileKind::Stabilized => "Stabilized",
                        FileKind::Archival => "Archival",
                    };
                    ui.separator();
                    ui.label(egui::RichText::new(label).small().weak());
                }
                let is_selected = self.selected == Some(i);
                if ui.selectable_label(is_selected, &entry.name).clicked() {
                    self.selected = Some(i);
                    to_open = Some(entry.clone());
                }
            }
        });
        to_open
    }
}
