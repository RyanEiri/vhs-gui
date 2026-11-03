//! Presence check for what the embedded VapourSynth scripts load: plugin binaries and the
//! `havsfunc` Python stack. None of it ships with vhs-gui (see ROADMAP.md), so a missing piece
//! otherwise surfaces as an obscure error from inside `vspipe`. This only checks that files
//! exist where the scripts will look; it does not import anything.
//!
//! Plugin directories mirror `assets/vs/vhs_plugins.py` (`VS_PLUGIN_DIRS` overrides). Module
//! directories are `~/.local/share/vsrepo/py` plus anything on `PYTHONPATH`, matching what
//! `vs_ops::build_vspipe` hands to `vspipe`.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A native plugin (`.so`) or data file found in a plugin directory.
    Plugin,
    /// A pure-Python module, `name.py` or a `name/` package, found in a module directory.
    Module,
}

/// One thing the scripts need, and which operations break without it.
#[derive(Clone, Copy, Debug)]
pub struct Need {
    pub name: &'static str,
    pub kind: Kind,
    pub used_by: &'static str,
}

pub const NEEDS: [Need; 12] = [
    Need {
        name: "libffms2.so",
        kind: Kind::Plugin,
        used_by: "every VapourSynth operation (source filter)",
    },
    Need {
        name: "libvivtc.so",
        kind: Kind::Plugin,
        used_by: "IVTC, IVTC+Decomb, VDecimate",
    },
    Need {
        name: "libmvtools.so",
        kind: Kind::Plugin,
        used_by: "QTGMC, IVTC+Decomb",
    },
    Need {
        name: "libfmtconv.so",
        kind: Kind::Plugin,
        used_by: "QTGMC",
    },
    Need {
        name: "libnnedi3.so",
        kind: Kind::Plugin,
        used_by: "QTGMC, IVTC+Decomb",
    },
    Need {
        name: "nnedi3_weights.bin",
        kind: Kind::Plugin,
        used_by: "QTGMC, IVTC+Decomb (weights file, next to libnnedi3.so)",
    },
    Need {
        name: "libmiscfilters.so",
        kind: Kind::Plugin,
        used_by: "QTGMC, IVTC+Decomb",
    },
    Need {
        name: "havsfunc",
        kind: Kind::Module,
        used_by: "QTGMC, IVTC+Decomb",
    },
    Need {
        name: "vsutil",
        kind: Kind::Module,
        used_by: "havsfunc",
    },
    Need {
        name: "mvsfunc",
        kind: Kind::Module,
        used_by: "havsfunc",
    },
    Need {
        name: "adjust",
        kind: Kind::Module,
        used_by: "havsfunc",
    },
    Need {
        name: "nnedi3_resample",
        kind: Kind::Module,
        used_by: "havsfunc",
    },
];

/// Where something was found, or `None`.
#[derive(Clone, Debug)]
pub struct StackStatus {
    pub need: Need,
    pub found: Option<PathBuf>,
}

pub fn plugin_dirs_from_env() -> Vec<PathBuf> {
    if let Some(v) = std::env::var_os("VS_PLUGIN_DIRS").filter(|v| !v.is_empty()) {
        return std::env::split_paths(&v)
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
    }
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/share/vsrepo/plugins"));
    }
    dirs.extend(
        [
            "/usr/lib/x86_64-linux-gnu/vapoursynth",
            "/usr/lib/vapoursynth",
            "/usr/local/lib/vapoursynth",
        ]
        .map(PathBuf::from),
    );
    dirs
}

pub fn module_dirs_from_env() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/share/vsrepo/py"));
    }
    if let Some(p) = std::env::var_os("PYTHONPATH") {
        dirs.extend(std::env::split_paths(&p).filter(|p| !p.as_os_str().is_empty()));
    }
    dirs
}

fn find_module(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter().find_map(|d| {
        let file = d.join(format!("{name}.py"));
        let pkg = d.join(name).join("__init__.py");
        [file, pkg].into_iter().find(|p| p.is_file())
    })
}

fn find_plugin(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// Look for every [`NEEDS`] entry in the given directories.
pub fn check_in(plugin_dirs: &[PathBuf], module_dirs: &[PathBuf]) -> Vec<StackStatus> {
    NEEDS
        .iter()
        .map(|&need| StackStatus {
            need,
            found: match need.kind {
                Kind::Plugin => find_plugin(plugin_dirs, need.name),
                Kind::Module => find_module(module_dirs, need.name),
            },
        })
        .collect()
}

pub fn check() -> Vec<StackStatus> {
    check_in(&plugin_dirs_from_env(), &module_dirs_from_env())
}

/// Missing items as one-line problems; empty when everything is present.
pub fn problems(status: &[StackStatus]) -> Vec<String> {
    status
        .iter()
        .filter(|s| s.found.is_none())
        .map(|s| {
            let what = match s.need.kind {
                Kind::Plugin => "VapourSynth plugin",
                Kind::Module => "Python module",
            };
            format!(
                "{what} {} missing (needed for {})",
                s.need.name, s.need.used_by
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let d =
                std::env::temp_dir().join(format!("vhs-gui-vsstack-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(&d).unwrap();
            Scratch(d)
        }
        fn touch(&self, rel: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "").unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Populate `s` with every plugin and module the scripts need.
    fn full(s: &Scratch) {
        for n in NEEDS {
            match n.kind {
                Kind::Plugin => s.touch(&format!("plugins/{}", n.name)),
                // Mix both layouts: a lone .py and a package.
                Kind::Module if n.name == "vsutil" || n.name == "mvsfunc" => {
                    s.touch(&format!("py/{}/__init__.py", n.name))
                }
                Kind::Module => s.touch(&format!("py/{}.py", n.name)),
            }
        }
    }

    fn dirs(s: &Scratch) -> (Vec<PathBuf>, Vec<PathBuf>) {
        (vec![s.0.join("plugins")], vec![s.0.join("py")])
    }

    #[test]
    fn complete_install_has_no_problems() {
        let s = Scratch::new("full");
        full(&s);
        let (p, m) = dirs(&s);
        let status = check_in(&p, &m);
        assert_eq!(status.len(), NEEDS.len());
        assert!(problems(&status).is_empty());
    }

    #[test]
    fn empty_install_reports_everything() {
        let s = Scratch::new("empty");
        let (p, m) = dirs(&s);
        let probs = problems(&check_in(&p, &m));
        assert_eq!(probs.len(), NEEDS.len());
        assert!(
            probs
                .iter()
                .any(|l| l.contains("havsfunc") && l.contains("Python module"))
        );
        assert!(
            probs
                .iter()
                .any(|l| l.contains("libffms2.so") && l.contains("plugin"))
        );
    }

    #[test]
    fn a_single_missing_module_is_named() {
        let s = Scratch::new("one");
        full(&s);
        fs::remove_file(s.0.join("py/havsfunc.py")).unwrap();
        let (p, m) = dirs(&s);
        let probs = problems(&check_in(&p, &m));
        assert_eq!(probs.len(), 1);
        assert!(
            probs[0].starts_with("Python module havsfunc missing"),
            "{probs:?}"
        );
    }

    #[test]
    fn package_directory_without_init_does_not_count() {
        let s = Scratch::new("noinit");
        full(&s);
        fs::remove_file(s.0.join("py/vsutil/__init__.py")).unwrap();
        s.touch("py/vsutil/clips.py");
        let (p, m) = dirs(&s);
        assert!(
            problems(&check_in(&p, &m))
                .iter()
                .any(|l| l.contains("vsutil"))
        );
    }

    #[test]
    fn later_directories_are_searched() {
        let s = Scratch::new("multi");
        full(&s);
        // Move the plugins out of the first directory into a second one.
        fs::rename(s.0.join("plugins"), s.0.join("sys")).unwrap();
        let status = check_in(&[s.0.join("plugins"), s.0.join("sys")], &[s.0.join("py")]);
        assert!(problems(&status).is_empty());
    }
}
