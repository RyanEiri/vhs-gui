//! The actions offered for the selected file, grouped by purpose, and the buttons that draw them.
//!
//! What each file category can do is data here ([`groups`]), so a test can check that every action
//! that existed before the redesign still exists. Drawing ([`show_groups`]) returns the chosen
//! [`ActionId`]; running it stays in `UpscalePanel`, which calls the same launch functions as ever.

use crate::library::FileKind;
use crate::theme::{self, color};
use crate::upscale::plan::Variant;

/// One thing the user can start on a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionId {
    Denoise,
    DenoiseQtgmc,
    Qtgmc,
    Ivtc,
    IvtcDecomb,
    FieldAlign,
    FixSync,
    Vdecimate,
    ViewerEncode,
    /// An upscale with this engine variant; the string is the button's original full label, which
    /// is also the start of the job's label ("Upscale Film EDIT_MASTER-X_VD.mkv").
    Upscale(Variant, &'static str),
    DetectLetterbox,
    AudioCleanup,
    Rename,
}

/// How a button looks, by how strongly the app recommends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Secondary,
    /// The one next step: filled with the accent.
    Primary,
    /// One of two equal next steps, when the right one depends on the source: accent outline.
    Recommended,
}

#[derive(Clone, Debug)]
pub struct Action {
    pub id: ActionId,
    /// What the button says.
    pub label: &'static str,
    /// The label it had before the redesign; the hover text, and what a test checks.
    pub full_label: &'static str,
    pub style: Style,
    /// A second line under the label (next steps only); makes the button two lines tall.
    pub caption: Option<&'static str>,
}

/// A titled run of buttons. `rows` break where the buttons would not fit side by side.
#[derive(Clone, Debug)]
pub struct Group {
    pub title: &'static str,
    pub rows: Vec<Vec<Action>>,
}

fn plain(id: ActionId, label: &'static str, full_label: &'static str) -> Action {
    Action {
        id,
        label,
        full_label,
        style: Style::Secondary,
        caption: None,
    }
}

fn step(
    id: ActionId,
    label: &'static str,
    full_label: &'static str,
    style: Style,
    caption: &'static str,
) -> Action {
    Action {
        id,
        label,
        full_label,
        style,
        caption: Some(caption),
    }
}

fn group(title: &'static str, rows: Vec<Vec<Action>>) -> Group {
    Group { title, rows }
}

fn picture_and_sound() -> Group {
    group(
        "Picture and sound",
        vec![
            vec![plain(
                ActionId::DetectLetterbox,
                "Detect Letterbox",
                "Detect Letterbox",
            )],
            vec![
                plain(ActionId::FixSync, "Fix A/V Sync", "Fix A/V Sync"),
                plain(ActionId::AudioCleanup, "Audio Cleanup…", "Audio Cleanup..."),
            ],
        ],
    )
}

/// The groups for a file category, top to bottom. The first holds the next step the app suggests;
/// Stabilized has none, because QTGMC or IVTC depends on how the tape was made.
pub fn groups(kind: &FileKind) -> Vec<Group> {
    use ActionId::*;
    match kind {
        FileKind::Archival => vec![
            group(
                "Next step",
                vec![vec![step(
                    DenoiseQtgmc,
                    "Denoise + QTGMC",
                    "Denoise+QTGMC",
                    Style::Primary,
                    "result goes to Stabilized",
                )]],
            ),
            group(
                "Or audio only",
                vec![vec![plain(Denoise, "Denoise", "Denoise")]],
            ),
        ],
        FileKind::Stabilized => vec![
            group(
                "Deinterlace · shot on video",
                vec![vec![plain(Qtgmc, "QTGMC", "QTGMC")]],
            ),
            group(
                "Inverse telecine · shot on film",
                vec![vec![
                    plain(Ivtc, "IVTC", "IVTC"),
                    plain(IvtcDecomb, "IVTC+Decomb", "IVTC+Decomb"),
                ]],
            ),
            group(
                "Repair",
                vec![vec![
                    plain(FieldAlign, "Field Align", "Field Align"),
                    plain(FixSync, "Fix A/V Sync", "Fix A/V Sync"),
                ]],
            ),
        ],
        FileKind::EditMaster => vec![
            group(
                "Next step · pick by source",
                vec![vec![
                    step(
                        ViewerEncode,
                        "Viewer Encode",
                        "Viewer Encode",
                        Style::Recommended,
                        "shot on video",
                    ),
                    step(
                        Vdecimate,
                        "VDecimate",
                        "VDecimate",
                        Style::Recommended,
                        "film, animation",
                    ),
                ]],
            ),
            group(
                "Upscale · video source",
                vec![vec![
                    plain(
                        Upscale(Variant::Standard, "Upscale Video"),
                        "Video",
                        "Upscale Video",
                    ),
                    plain(
                        Upscale(Variant::Bw, "Upscale Video B&W"),
                        "Video B&W",
                        "Upscale Video B&W",
                    ),
                ]],
            ),
            picture_and_sound(),
        ],
        FileKind::EditMasterVD => vec![
            group(
                "Next step",
                vec![vec![step(
                    ViewerEncode,
                    "Viewer Encode",
                    "Viewer Encode",
                    Style::Primary,
                    "result goes to Viewer",
                )]],
            ),
            group(
                "Upscale · 24 fps source",
                vec![vec![
                    plain(
                        Upscale(Variant::Standard, "Upscale Film"),
                        "Film",
                        "Upscale Film",
                    ),
                    plain(
                        Upscale(Variant::Bw, "Upscale Film B&W"),
                        "Film B&W",
                        "Upscale Film B&W",
                    ),
                    plain(
                        Upscale(Variant::Anime, "Upscale Anime"),
                        "Anime",
                        "Upscale Anime",
                    ),
                ]],
            ),
            picture_and_sound(),
        ],
        FileKind::Viewer => vec![
            group(
                "Next step",
                vec![vec![step(
                    Rename,
                    "Rename…",
                    "Rename…",
                    Style::Primary,
                    "suggests a readable name",
                )]],
            ),
            group(
                "Upscale",
                vec![vec![
                    plain(Upscale(Variant::Standard, "Upscale"), "Upscale", "Upscale"),
                    plain(Upscale(Variant::Bw, "Upscale B&W"), "B&W", "Upscale B&W"),
                    plain(
                        Upscale(Variant::Anime, "Upscale Anime"),
                        "Anime",
                        "Upscale Anime",
                    ),
                ]],
            ),
            group(
                "Sound",
                vec![vec![
                    plain(FixSync, "Fix A/V Sync", "Fix A/V Sync"),
                    plain(AudioCleanup, "Audio Cleanup…", "Audio Cleanup..."),
                ]],
            ),
        ],
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// The header row of an inline panel: a back button and the title. True when Back is pressed.
pub fn inline_header(ui: &mut egui::Ui, title: &str, title_color: egui::Color32) -> bool {
    let mut back = false;
    ui.horizontal(|ui| {
        let b = egui::Button::new(crate::icons::BACK)
            .fill(color::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
            .min_size(egui::vec2(22.0, 22.0));
        back = ui.add(b).on_hover_text("Back").clicked();
        ui.label(
            egui::RichText::new(title)
                .family(egui::FontFamily::Name(theme::FAMILY_BOLD.into()))
                .size(12.5)
                .color(title_color),
        );
    });
    back
}

/// Draw the groups; returns the action whose button was clicked this frame. Everything is drawn
/// in whatever enabled state `ui` is in (the caller disables the lot while a job runs).
pub fn show_groups(ui: &mut egui::Ui, groups: &[Group]) -> Option<ActionId> {
    let mut picked = None;
    for g in groups {
        ui.label(theme::section_label(g.title));
        for row in &g.rows {
            ui.columns(row.len(), |cols| {
                for (col, action) in cols.iter_mut().zip(row) {
                    let resp = if action.caption.is_some() {
                        next_step_button(col, action)
                    } else {
                        col.spacing_mut().button_padding.x = 4.0;
                        let label = egui::RichText::new(action.label).size(11.0);
                        col.add_sized(
                            [col.available_width(), theme::size::CONTROL_HEIGHT],
                            egui::Button::new(label).wrap_mode(egui::TextWrapMode::Truncate),
                        )
                    };
                    if resp.on_hover_text(action.full_label).clicked() {
                        picked = Some(action.id);
                    }
                }
            });
        }
    }
    picked
}

/// A two-line, left-aligned next-step button: the label over a caption.
fn next_step_button(ui: &mut egui::Ui, a: &Action) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), theme::size::NEXT_STEP_HEIGHT),
        egui::Sense::click(),
    );
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let enabled = ui.is_enabled();
    let hovered = enabled && resp.hovered();
    let pressed = enabled && resp.is_pointer_button_down_on();
    let (fill, stroke, label_c, caption_c) = match (enabled, a.style) {
        (false, _) => (
            color::TRANSPARENT,
            color::STROKE_SUBTLE,
            color::TEXT_DISABLED,
            color::TEXT_DISABLED,
        ),
        (true, Style::Primary) => {
            let fill = if pressed {
                color::ACCENT_PRESSED
            } else if hovered {
                color::ACCENT_HOVERED
            } else {
                color::ACCENT
            };
            (fill, color::ACCENT, color::ACCENT_INK, color::ACCENT_INK)
        }
        (true, _) => {
            let fill = if pressed {
                color::ACCENT_PRESSED
            } else if hovered {
                color::SELECTION
            } else {
                color::TRANSPARENT
            };
            let ink = if pressed {
                color::ACCENT_INK
            } else {
                color::ACCENT
            };
            (fill, color::ACCENT, ink, ink)
        }
    };
    let p = ui.painter();
    let radius = egui::CornerRadius::same(theme::size::RADIUS_WIDGET);
    p.rect_filled(rect, radius, fill);
    if enabled {
        p.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0_f32, stroke),
            egui::StrokeKind::Inside,
        );
    } else {
        dashed_outline(p, rect, egui::Stroke::new(1.0_f32, stroke));
    }
    p.text(
        egui::pos2(rect.left() + 8.0, rect.top() + 12.0),
        egui::Align2::LEFT_CENTER,
        a.label,
        egui::FontId::new(11.0, egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into())),
        label_c,
    );
    if let Some(caption) = a.caption {
        p.text(
            egui::pos2(rect.left() + 8.0, rect.top() + 26.0),
            egui::Align2::LEFT_CENTER,
            caption,
            egui::FontId::new(10.5, egui::FontFamily::Proportional),
            caption_c,
        );
    }
    resp
}

/// The disabled look: a dashed outline and no fill.
fn dashed_outline(painter: &egui::Painter, rect: egui::Rect, stroke: egui::Stroke) {
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    for pair in corners.windows(2) {
        painter.extend(egui::Shape::dashed_line(pair, stroke, 3.0, 3.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(kind: &FileKind) -> Vec<Action> {
        groups(kind)
            .into_iter()
            .flat_map(|g| g.rows.into_iter().flatten())
            .collect()
    }

    fn labels(kind: &FileKind) -> Vec<&'static str> {
        let mut v: Vec<_> = all(kind).iter().map(|a| a.full_label).collect();
        v.sort_unstable();
        v
    }

    /// Every button each category had before the redesign still exists (Delete and the letterbox
    /// section moved: Move to Trash… is in the Library toolbar, Detect Letterbox is a button).
    #[test]
    fn every_action_a_category_had_still_exists() {
        let sorted = |mut v: Vec<&'static str>| {
            v.sort_unstable();
            v
        };
        assert_eq!(
            labels(&FileKind::Archival),
            sorted(vec!["Denoise", "Denoise+QTGMC"])
        );
        assert_eq!(
            labels(&FileKind::Stabilized),
            sorted(vec![
                "QTGMC",
                "IVTC",
                "IVTC+Decomb",
                "Field Align",
                "Fix A/V Sync"
            ])
        );
        assert_eq!(
            labels(&FileKind::EditMaster),
            sorted(vec![
                "VDecimate",
                "Viewer Encode",
                "Upscale Video",
                "Upscale Video B&W",
                "Fix A/V Sync",
                "Audio Cleanup...",
                "Detect Letterbox",
            ])
        );
        assert_eq!(
            labels(&FileKind::EditMasterVD),
            sorted(vec![
                "Viewer Encode",
                "Upscale Film",
                "Upscale Film B&W",
                "Upscale Anime",
                "Fix A/V Sync",
                "Audio Cleanup...",
                "Detect Letterbox",
            ])
        );
        assert_eq!(
            labels(&FileKind::Viewer),
            sorted(vec![
                "Upscale",
                "Upscale B&W",
                "Upscale Anime",
                "Fix A/V Sync",
                "Audio Cleanup...",
                "Rename…",
            ])
        );
    }

    #[test]
    fn an_upscale_action_keeps_its_old_label_as_the_job_name_and_picks_its_engine_variant() {
        for a in all(&FileKind::EditMasterVD) {
            if let ActionId::Upscale(variant, verb) = a.id {
                assert_eq!(
                    verb, a.full_label,
                    "the job label starts with the old button label"
                );
                let want = match a.label {
                    "Film B&W" => Variant::Bw,
                    "Anime" => Variant::Anime,
                    _ => Variant::Standard,
                };
                assert_eq!(variant, want);
            }
        }
    }

    #[test]
    fn each_category_has_one_filled_next_step_or_two_outlined_ones() {
        let styles = |k: &FileKind| -> Vec<Style> {
            all(k)
                .iter()
                .filter(|a| a.style != Style::Secondary)
                .map(|a| a.style)
                .collect()
        };
        assert_eq!(styles(&FileKind::Archival), [Style::Primary]);
        assert_eq!(styles(&FileKind::EditMasterVD), [Style::Primary]);
        assert_eq!(styles(&FileKind::Viewer), [Style::Primary]);
        assert_eq!(
            styles(&FileKind::EditMaster),
            [Style::Recommended, Style::Recommended],
            "film or video depends on the tape"
        );
        assert!(
            styles(&FileKind::Stabilized).is_empty(),
            "QTGMC or IVTC depends on the tape"
        );
    }

    #[test]
    fn only_next_steps_carry_a_caption() {
        for k in [
            FileKind::Archival,
            FileKind::Stabilized,
            FileKind::EditMaster,
            FileKind::EditMasterVD,
            FileKind::Viewer,
        ] {
            for a in all(&k) {
                assert_eq!(
                    a.caption.is_some(),
                    a.style != Style::Secondary,
                    "{}",
                    a.label
                );
            }
        }
    }

    #[test]
    fn no_row_has_more_than_three_buttons() {
        for k in [
            FileKind::Archival,
            FileKind::Stabilized,
            FileKind::EditMaster,
            FileKind::EditMasterVD,
            FileKind::Viewer,
        ] {
            for g in groups(&k) {
                assert!(
                    g.rows.iter().all(|r| (1..=3).contains(&r.len())),
                    "{}",
                    g.title
                );
            }
        }
    }
}
