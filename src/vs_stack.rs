//! Presence check for what the embedded VapourSynth scripts load: plugin binaries and the
//! `havsfunc` Python stack. None of it ships with vhs-gui (see ROADMAP.md), so a missing piece
//! otherwise surfaces as an obscure error from inside `vspipe`. This only checks that files
//! exist where the scripts will look; it does not import anything.
//!
//! Plugin directories mirror `assets/vs/vhs_plugins.py` (`VS_PLUGIN_DIRS` overrides), including the
//! plugin folder of an R81 `vspipe` (`<prefix>/lib/python*/site-packages/vapoursynth/plugins`,
//! found from where `vspipe` resolves), whose API 4 plugins are named without `lib` and with `znedi3`
//! in place of `nnedi3`. Module
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

/// `mvsfunc`, `adjust` and `nnedi3_resample` are deliberately not listed: `havsfunc` imports them only
/// inside functions the scripts never reach (BM3D denoising, `Tweak`, `SMDegrain`), they carry no licence
/// to ship, and QTGMC and IVTC+Decomb were run on R81 without them.
pub const NEEDS: [Need; 9] = [
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
    if let Some(vspipe) = crate::tools::resolve(crate::tools::Tool::Vspipe) {
        dirs.extend(own_plugin_dirs(&vspipe));
    }
    dirs
}

/// The plugin folders of a pip-installed (R81) VapourSynth that `vspipe` belongs to: a wheel's
/// `vspipe` sits in `<prefix>/bin`, its plugins in `<prefix>/lib/python*/site-packages/vapoursynth/plugins`.
fn own_plugin_dirs(vspipe: &Path) -> Vec<PathBuf> {
    let Some(prefix) = vspipe.parent().and_then(Path::parent) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(prefix.join("lib")) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("python"))
        .map(|e| e.path().join("site-packages/vapoursynth/plugins"))
        .filter(|p| p.is_dir())
        .collect();
    found.sort();
    found
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

/// File names under which `name` may be installed: as listed, without the `lib` prefix (R81's API 4
/// builds), and `libnnedi3.so` also as `znedi3.so` (R81 ships znedi3 in its place).
fn plugin_names(name: &str) -> Vec<String> {
    let mut names = vec![name.to_string()];
    if let Some(bare) = name.strip_prefix("lib") {
        names.push(bare.to_string());
    }
    if name == "libnnedi3.so" {
        names.push("znedi3.so".to_string());
    }
    names
}

fn find_plugin(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let names = plugin_names(name);
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
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

    /// What an R81 wheel install provides: API 4 plugins without the `lib` prefix, znedi3 for nnedi3.
    fn r81_plugins(s: &Scratch) {
        for f in ["mvtools.so", "znedi3.so", "vivtc.so", "nnedi3_weights.bin"] {
            s.touch(&format!("r81/{f}"));
        }
    }

    #[test]
    fn r81_api4_plugins_satisfy_the_needs_they_replace() {
        let s = Scratch::new("r81");
        full(&s);
        for n in ["libmvtools.so", "libnnedi3.so", "libvivtc.so"] {
            fs::remove_file(s.0.join("plugins").join(n)).unwrap();
        }
        fs::remove_file(s.0.join("plugins/nnedi3_weights.bin")).unwrap();
        // Without the R81 folder those four are missing...
        let (p, m) = dirs(&s);
        assert_eq!(problems(&check_in(&p, &m)).len(), 4);
        // ...with it, nothing is.
        r81_plugins(&s);
        let with = check_in(&[s.0.join("plugins"), s.0.join("r81")], &m);
        assert!(problems(&with).is_empty(), "{:?}", problems(&with));
    }

    #[test]
    fn the_wheel_plugin_folder_is_found_from_vspipe() {
        let s = Scratch::new("own");
        s.touch("venv/bin/vspipe");
        fs::create_dir_all(s.0.join("venv/lib/python3.14/site-packages/vapoursynth/plugins"))
            .unwrap();
        fs::create_dir_all(s.0.join("venv/lib/python3.99/other")).unwrap();
        let found = own_plugin_dirs(&s.0.join("venv/bin/vspipe"));
        assert_eq!(
            found,
            vec![s.0.join("venv/lib/python3.14/site-packages/vapoursynth/plugins")]
        );
        // A system vspipe has no such folder.
        assert!(own_plugin_dirs(Path::new("/usr/bin/vspipe")).is_empty());
    }
}
