//! The "Detect Letterbox" section under an Edit Master row: runs [`crate::crop::CropScan`], shows
//! what it found on a preview frame, and remembers the result per file so the Upscale and Viewer
//! Encode buttons can apply it. Results live in memory only: after a GUI restart the file is
//! detected again, and a resumed upscale whose crop is missing is refused by the work directory's
//! settings check rather than mixing cropped and uncropped segments.

use crate::icons;
use crate::theme;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::crop::{Crop, CropDecision, CropScan, DEFAULT_MARGIN, ScanResult, decide, describe};

/// One file's scan result and the user's choices about it.
struct Found {
    result: ScanResult,
    /// Apply the crop to Upscale and Viewer Encode.
    enabled: bool,
    /// Lines kept back inside the detected picture on each side.
    margin: u32,
    texture: Option<egui::TextureHandle>,
}

impl Found {
    fn decision(&self) -> CropDecision {
        decide(
            &self.result.samples,
            self.result.src.0,
            self.result.src.1,
            self.margin,
        )
    }

    fn crop(&self) -> Option<Crop> {
        match self.decision() {
            CropDecision::Crop { crop, .. } => Some(crop),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Letterbox {
    scan: Option<(PathBuf, CropScan)>,
    found: HashMap<PathBuf, Found>,
}

impl Letterbox {
    /// The crop to apply to `path`, if it was detected, found usable and left switched on.
    pub fn crop_for(&self, path: &Path) -> Option<Crop> {
        self.found.get(path).filter(|f| f.enabled)?.crop()
    }

    /// Begin detecting for `path` unless it already has a result or a scan is running.
    pub fn start_if_needed(&mut self, path: &Path) {
        if self.scan.is_none() && !self.found.contains_key(path) {
            self.scan = Some((path.to_owned(), CropScan::start(path)));
        }
    }

    /// The inline Letterbox panel: detecting, or the result with its preview and the crop choice.
    /// Returns true when Back was pressed.
    pub fn show_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        path: &Path,
        status: &mut String,
    ) -> bool {
        if super::actions::inline_header(ui, "Letterbox", theme::color::TEXT_STRONG) {
            return true;
        }

        if matches!(&self.scan, Some((p, _)) if p == path) {
            match self.scan.as_ref().and_then(|(_, s)| s.poll()) {
                None => {
                    if let Some((_, scan)) = &self.scan {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(14.0).color(theme::color::ACCENT));
                            ui.label(
                                egui::RichText::new(format!("Detecting… {}", scan.elapsed_str()))
                                    .size(11.5)
                                    .color(theme::color::TEXT_WEAK),
                            );
                            if ui.button("Cancel").clicked() {
                                scan.cancel();
                            }
                        });
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(500));
                    return false;
                }
                Some(outcome) => {
                    self.scan = None;
                    match outcome {
                        Ok(result) => self.finish(ctx, path, result, status),
                        Err(e) if e == "cancelled" => {
                            *status = "Letterbox detection cancelled".into();
                        }
                        Err(e) => *status = format!("Letterbox detection failed: {e}"),
                    }
                }
            }
        }

        self.show_result(ui, path);
        if self.scan.is_none() {
            let again = egui::Button::new(format!("{} Detect again", icons::SEARCH));
            if ui
                .add_sized([ui.available_width(), theme::size::CONTROL_HEIGHT], again)
                .clicked()
            {
                self.found.remove(path);
                self.scan = Some((path.to_owned(), CropScan::start(path)));
            }
        }
        false
    }

    fn finish(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        result: ScanResult,
        status: &mut String,
    ) {
        *status = describe(&result.decision);
        let texture = result
            .preview
            .as_deref()
            .and_then(super::upscale::load_jpeg_as_egui_image)
            .map(|img| ctx.load_texture("letterbox_preview", img, Default::default()));
        let enabled = matches!(result.decision, CropDecision::Crop { .. });
        self.found.insert(
            path.to_owned(),
            Found {
                result,
                enabled,
                margin: DEFAULT_MARGIN,
                texture,
            },
        );
    }

    fn show_result(&mut self, ui: &mut egui::Ui, path: &Path) {
        let Some(found) = self.found.get_mut(path) else {
            return;
        };
        let decision = found.decision();
        match &decision {
            CropDecision::Crop {
                crop,
                agree,
                usable,
            } => {
                ui.label(
                    egui::RichText::new(format!("{}x{} from line {}", crop.w, crop.h, crop.y))
                        .family(egui::FontFamily::Name(theme::FAMILY_MONO_BOLD.into()))
                        .size(12.0)
                        .color(theme::color::TEXT_STRONG),
                );
                ui.label(
                    egui::RichText::new(format!("{agree} of {usable} samples agree"))
                        .size(11.5)
                        .color(theme::color::TEXT_WEAK),
                );
            }
            CropDecision::Mixed { .. } => {
                ui.label(
                    egui::RichText::new(format!("{} {}", icons::WARNING, describe(&decision)))
                        .size(11.5)
                        .color(theme::color::WARNING),
                );
            }
            _ => {
                ui.label(
                    egui::RichText::new(describe(&decision))
                        .size(11.5)
                        .color(theme::color::TEXT_NORMAL),
                );
            }
        }

        let Some(crop) = found.crop() else {
            // Nothing to apply; keep the mixed result visible so it can be checked.
            if let CropDecision::Mixed { .. } = found.result.decision {
                ui.label(
                    egui::RichText::new(
                        "Cut the intro or credits out of the edit, or leave it uncropped.",
                    )
                    .small()
                    .weak(),
                );
            }
            return;
        };

        ui.checkbox(&mut found.enabled, "Crop on Upscale and Viewer Encode");
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Margin").small());
            ui.add(
                egui::DragValue::new(&mut found.margin)
                    .range(0..=8)
                    .suffix(" lines"),
            );
        });

        if let Some(tex) = &found.texture {
            let width = ui.available_width().min(210.0);
            let size = tex.size_vec2();
            let height = width * size.y / size.x.max(1.0);
            let resp = ui.add(egui::Image::new((tex.id(), egui::vec2(width, height))));
            // Shade what the crop removes.
            let (sw, sh) = found.result.src;
            let (sw, sh) = (sw.max(1) as f32, sh.max(1) as f32);
            let shade = theme::color::DANGER.gamma_multiply(0.4);
            let r = resp.rect;
            let top = r.top() + r.height() * crop.y as f32 / sh;
            let bottom = r.top() + r.height() * (crop.y + crop.h) as f32 / sh;
            let left = r.left() + r.width() * crop.x as f32 / sw;
            let right = r.left() + r.width() * (crop.x + crop.w) as f32 / sw;
            let p = ui.painter();
            p.rect_filled(
                egui::Rect::from_min_max(r.min, egui::pos2(r.right(), top)),
                0.0,
                shade,
            );
            p.rect_filled(
                egui::Rect::from_min_max(egui::pos2(r.left(), bottom), r.max),
                0.0,
                shade,
            );
            if crop.x > 0 {
                p.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(r.left(), top), egui::pos2(left, bottom)),
                    0.0,
                    shade,
                );
            }
            if crop.x + crop.w < found.result.src.0 {
                p.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(right, top), egui::pos2(r.right(), bottom)),
                    0.0,
                    shade,
                );
            }
        }
    }
}
