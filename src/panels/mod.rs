pub mod actions;
pub mod letterbox;
pub mod monitor;
pub mod notice;
pub mod status_bar;
pub mod tools;
pub mod upscale;

/// The three views, chosen in the rail. `Library` and `Upscale` are the two halves of what used to
/// be the single Upscale view; a settings file saved by an older version may still say `upscale`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    /// Shown as "Capture".
    #[default]
    Monitor,
    Library,
    Upscale,
}

impl ViewMode {
    /// The view to open with. The Upscale view only has something to show while a job runs, so a
    /// saved `Upscale` (which in older versions meant the library) opens the Library otherwise.
    pub fn at_startup(self, upscale_running: bool) -> ViewMode {
        if self == ViewMode::Upscale && !upscale_running {
            ViewMode::Library
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ViewMode;

    #[test]
    fn a_saved_upscale_view_opens_the_library_unless_a_job_is_running() {
        assert_eq!(ViewMode::Upscale.at_startup(false), ViewMode::Library);
        assert_eq!(ViewMode::Upscale.at_startup(true), ViewMode::Upscale);
        assert_eq!(ViewMode::Monitor.at_startup(false), ViewMode::Monitor);
        assert_eq!(ViewMode::Library.at_startup(false), ViewMode::Library);
    }

    #[test]
    fn views_round_trip_through_toml_and_old_files_still_load() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct W {
            mode: ViewMode,
        }
        for (text, want) in [
            ("mode = \"monitor\"", ViewMode::Monitor),
            ("mode = \"upscale\"", ViewMode::Upscale),
            ("mode = \"library\"", ViewMode::Library),
        ] {
            assert_eq!(toml::from_str::<W>(text).unwrap().mode, want);
        }
        let out = toml::to_string(&W {
            mode: ViewMode::Library,
        })
        .unwrap();
        assert_eq!(out.trim(), "mode = \"library\"");
    }
}
