//! What a job leaves behind when it ends: a red banner for a failure (stays until dismissed, names
//! the job, the file and the log) and a short toast for a success (with Show). The data and the
//! wording are pure so they can be tested; `show_banner` and `show_toast` draw them.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::icons;
use crate::theme::{self, color};

/// How long a success toast stays before it fades.
pub const TOAST_LIFETIME: Duration = Duration::from_secs(8);

/// A job that has just ended, as `UpscalePanel::poll` reports it.
pub struct Finished {
    pub label: String,
    pub failed: bool,
    /// The file it was to produce.
    pub output: Option<PathBuf>,
    /// The job's own log; `None` for a reattached job, whose log we do not know.
    pub log: Option<PathBuf>,
}

pub struct Failure {
    pub label: String,
    pub log: PathBuf,
}

pub struct Toast {
    pub label: String,
    pub output: Option<PathBuf>,
    pub until: Instant,
}

#[derive(Default)]
pub struct Notices {
    pub failure: Option<Failure>,
    pub toast: Option<Toast>,
}

impl Notices {
    /// Take in an ended job. A failure replaces any earlier banner; a success replaces the toast.
    /// `log_dir` is the fallback when the job's own log is unknown.
    pub fn push(&mut self, f: Finished, log_dir: &Path, now: Instant) {
        if f.failed {
            self.toast = None;
            self.failure = Some(Failure {
                log: f.log.unwrap_or_else(|| log_dir.to_path_buf()),
                label: f.label,
            });
        } else {
            self.toast = Some(Toast {
                label: f.label,
                output: f.output,
                until: now + TOAST_LIFETIME,
            });
        }
    }

    /// Drop a toast whose time is up.
    pub fn expire(&mut self, now: Instant) {
        if self.toast.as_ref().is_some_and(|t| now >= t.until) {
            self.toast = None;
        }
    }
}

impl Failure {
    /// "Upscale Film X failed": the label already names the job and its file.
    pub fn headline(&self) -> String {
        format!("{} failed", self.label)
    }

    pub fn detail(&self) -> String {
        if self.log.is_dir() {
            format!("Logs are in {}", self.log.display())
        } else {
            format!("Log: {}", self.log.display())
        }
    }
}

/// The red banner above the status bar. Returns `(open_log, dismiss)`.
pub fn show_banner(ui: &mut egui::Ui, failure: &Failure) -> (bool, bool) {
    let (mut open, mut dismiss) = (false, false);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(
            egui::RichText::new(icons::FAILED)
                .size(16.0)
                .color(color::DANGER),
        );
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add(
                egui::Label::new(
                    egui::RichText::new(failure.headline())
                        .family(egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into()))
                        .size(12.5)
                        .color(color::TEXT_STRONG),
                )
                .truncate(),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(failure.detail())
                        .monospace()
                        .size(11.0)
                        .color(color::TEXT_NORMAL),
                )
                .truncate(),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Dismiss").clicked() {
                dismiss = true;
            }
            if ui.button(format!("{} Open log", icons::OPEN_LOG)).clicked() {
                open = true;
            }
        });
    });
    (open, dismiss)
}

/// The success toast. Returns true when Show was pressed.
pub fn show_toast(ui: &mut egui::Ui, toast: &Toast, can_show: bool) -> bool {
    let mut show = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(egui::RichText::new(icons::OK).color(color::SUCCESS));
        ui.label(
            egui::RichText::new(format!("{} finished", toast.label))
                .size(12.0)
                .color(color::TEXT_STRONG),
        );
        if can_show && ui.button("Show").clicked() {
            show = true;
        }
    });
    show
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(failed: bool, log: Option<&str>) -> Finished {
        Finished {
            label: "QTGMC tape.mkv".into(),
            failed,
            output: Some(PathBuf::from("/v/captures/stabilized/tape_QTGMC.mkv")),
            log: log.map(PathBuf::from),
        }
    }

    #[test]
    fn a_failure_names_the_job_and_the_log() {
        let mut n = Notices::default();
        n.push(
            finished(true, Some("/l/qtgmc_1.log")),
            Path::new("/l"),
            Instant::now(),
        );
        let f = n.failure.as_ref().unwrap();
        assert_eq!(f.headline(), "QTGMC tape.mkv failed");
        assert_eq!(f.detail(), "Log: /l/qtgmc_1.log");
    }

    #[test]
    fn a_reattached_job_points_at_the_log_folder() {
        let dir = std::env::temp_dir();
        let mut n = Notices::default();
        n.push(finished(true, None), &dir, Instant::now());
        let f = n.failure.as_ref().unwrap();
        assert_eq!(f.log, dir);
        assert!(f.detail().starts_with("Logs are in "));
    }

    #[test]
    fn the_banner_stays_until_dismissed_and_a_success_does_not_clear_it() {
        let t0 = Instant::now();
        let mut n = Notices::default();
        n.push(finished(true, Some("/l/a.log")), Path::new("/l"), t0);
        n.push(finished(false, None), Path::new("/l"), t0);
        n.expire(t0 + Duration::from_secs(3600));
        assert!(n.failure.is_some(), "no timer on a failure");
        assert!(n.toast.is_none(), "the toast has expired");
    }

    #[test]
    fn a_toast_lasts_eight_seconds_and_a_failure_replaces_it() {
        let t0 = Instant::now();
        let mut n = Notices::default();
        n.push(finished(false, None), Path::new("/l"), t0);
        n.expire(t0 + Duration::from_secs(7));
        assert!(n.toast.is_some());
        n.expire(t0 + Duration::from_secs(8));
        assert!(n.toast.is_none());
        n.push(finished(false, None), Path::new("/l"), t0);
        n.push(finished(true, Some("/l/a.log")), Path::new("/l"), t0);
        assert!(n.toast.is_none());
    }
}
