//! The VapourSynth scripts vhs-gui runs, embedded in the binary.
//!
//! `vspipe` needs real files, so [`materialize`] writes them to a content-hashed directory
//! under the user cache (`$XDG_CACHE_HOME/vhs-gui/vs/<hash>/`, falling back to
//! `~/.cache/vhs-gui/vs/<hash>/`). A new build with different scripts gets a new directory,
//! so a running job is never edited underneath. Files are only rewritten when missing or
//! different, and each write goes through a temp file plus rename so a reader never sees a
//! half-written script.
//!
//! Only the scripts are embedded. `havsfunc` and its Python helpers stay an external
//! dependency (see ROADMAP.md): their licences are not all confirmed, and the plugin binaries
//! they drive cannot be bundled either.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// `(file name, contents)`. `vhs_plugins.py` is imported by the scripts, so the directory has
/// to be on `PYTHONPATH` (see `vs_ops::build_vspipe`).
pub const FILES: [(&str, &str); 7] = [
    ("qtgmc.vpy", include_str!("../assets/vs/qtgmc.vpy")),
    ("ivtc.vpy", include_str!("../assets/vs/ivtc.vpy")),
    (
        "ivtc_decombed.vpy",
        include_str!("../assets/vs/ivtc_decombed.vpy"),
    ),
    (
        "field_align.vpy",
        include_str!("../assets/vs/field_align.vpy"),
    ),
    ("vdecimate.vpy", include_str!("../assets/vs/vdecimate.vpy")),
    (
        "vhs_plugins.py",
        include_str!("../assets/vs/vhs_plugins.py"),
    ),
    // Not a script: the pinned wheels `vs_setup` installs, kept here so pip gets a real file.
    (
        "requirements.txt",
        include_str!("../assets/vs/requirements.txt"),
    ),
];

/// FNV-1a 64 over names and contents. Not `DefaultHasher`: its output is not guaranteed to be
/// stable across Rust releases, and this value names an on-disk directory.
fn content_hash(files: &[(&str, &str)]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff; // separator, so ("ab","c") and ("a","bc") differ
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for (name, body) in files {
        feed(name.as_bytes());
        feed(body.as_bytes());
    }
    h
}

fn cache_base() -> PathBuf {
    let root = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into())).join(".cache")
        });
    root.join("vhs-gui/vs")
}

/// Write `files` under `base/<hash>/` (only what is missing or changed) and return that dir.
fn materialize_into(base: &Path, files: &[(&str, &str)]) -> io::Result<PathBuf> {
    let dir = base.join(format!("{:016x}", content_hash(files)));
    fs::create_dir_all(&dir)?;
    for (name, body) in files {
        let path = dir.join(name);
        if fs::read(&path).is_ok_and(|b| b == body.as_bytes()) {
            continue;
        }
        let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
        fs::write(&tmp, body)?;
        fs::rename(&tmp, &path)?;
    }
    Ok(dir)
}

/// Directory holding the embedded scripts, created or repaired as needed.
pub fn materialize() -> io::Result<PathBuf> {
    materialize_into(&cache_base(), &FILES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("vhs-gui-vsassets-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn writes_every_embedded_file_verbatim() {
        let base = scratch("write");
        let dir = materialize_into(&base, &FILES).unwrap();
        for (name, body) in FILES {
            assert_eq!(fs::read_to_string(dir.join(name)).unwrap(), body, "{name}");
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn second_call_reuses_the_directory_and_leaves_files_untouched() {
        let base = scratch("reuse");
        let first = materialize_into(&base, &FILES).unwrap();
        let before = fs::metadata(first.join("qtgmc.vpy"))
            .unwrap()
            .modified()
            .unwrap();
        let second = materialize_into(&base, &FILES).unwrap();
        let after = fs::metadata(second.join("qtgmc.vpy"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(before, after, "an up-to-date file must not be rewritten");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn repairs_a_modified_or_deleted_file() {
        let base = scratch("repair");
        let dir = materialize_into(&base, &FILES).unwrap();
        fs::write(dir.join("ivtc.vpy"), "tampered").unwrap();
        fs::remove_file(dir.join("vhs_plugins.py")).unwrap();
        let again = materialize_into(&base, &FILES).unwrap();
        assert_eq!(dir, again);
        for (name, body) in FILES {
            assert_eq!(
                fs::read_to_string(again.join(name)).unwrap(),
                body,
                "{name}"
            );
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn different_contents_get_a_different_directory() {
        let a = [("x.vpy", "one")];
        let b = [("x.vpy", "two")];
        assert_ne!(content_hash(&a), content_hash(&b));
        // The separator keeps the name/body boundary significant.
        assert_ne!(content_hash(&[("ab", "c")]), content_hash(&[("a", "bc")]));
        // And the hash is a fixed function of its input, not of this run.
        assert_eq!(content_hash(&[("a", "b")]), content_hash(&[("a", "b")]));
    }

    #[test]
    fn ivtc_decombed_callback_does_not_read_its_own_output() {
        let body = FILES
            .iter()
            .find(|(n, _)| *n == "ivtc_decombed.vpy")
            .unwrap()
            .1;
        // `else clip` inside the FrameEval lambda reads the FrameEval clip itself and deadlocks.
        assert!(!body.contains("else clip"));
        assert!(body.contains("else base"));
    }

    #[test]
    fn scripts_are_portable() {
        for (name, body) in FILES {
            assert!(!body.contains("/home/"), "{name} hardcodes a home path");
            if name.ends_with(".vpy") {
                assert!(
                    body.contains("import vhs_plugins"),
                    "{name} must load plugins through vhs_plugins"
                );
            }
        }
    }

    /// Run `vhs_plugins.py` against a fake `vapoursynth` module. `namespaces` are the plugin
    /// namespaces the fake core already has (R81 autoloads its own); `plugin_dir` is what a fake
    /// `get_plugin_dir` returns (`None` leaves it out, as R55 does). Prints what happened.
    fn run_loader(
        name: &str,
        namespaces: &[&str],
        plugin_dir: Option<&Path>,
        body: &str,
    ) -> String {
        let dir = scratch(name);
        fs::create_dir_all(&dir).unwrap();
        let loader = FILES
            .iter()
            .find(|(n, _)| *n == "vhs_plugins.py")
            .unwrap()
            .1;
        fs::write(dir.join("vhs_plugins.py"), loader).unwrap();
        let get_dir = plugin_dir.map_or(String::new(), |d| {
            format!(
                "vs.get_plugin_dir = lambda: {:?}\n",
                d.display().to_string()
            )
        });
        let fake = format!(
            "import sys, types\n\
             vs = types.ModuleType('vapoursynth')\n\
             class Error(Exception): pass\n\
             vs.Error = Error\n\
             loaded = []\n\
             class Std:\n    def LoadPlugin(self, path): loaded.append(path)\n\
             class Core:\n    std = Std()\n\
             core = Core()\n\
             for ns in {namespaces:?}: setattr(core, ns, object())\n\
             vs.core = core\n{get_dir}\
             sys.modules['vapoursynth'] = vs\n\
             sys.path.insert(0, {dir:?})\n\
             import vhs_plugins\n{body}\n",
            dir = dir.display().to_string(),
        );
        let out = std::process::Command::new("python3")
            .args(["-I", "-c", &fake])
            .env_remove("VS_PLUGIN_DIRS")
            // `~` in the default directories must not find this machine's own plugins.
            .env("HOME", &dir)
            .output()
            .unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn a_plugin_the_core_already_provides_is_not_loaded() {
        // R81: the API 4 mvtools/znedi3 wheels are autoloaded; the API 3 files must not be touched.
        let out = run_loader(
            "provided",
            &["mv", "znedi3"],
            None,
            "print(vhs_plugins.load('libmvtools.so'), vhs_plugins.load('libnnedi3.so'), loaded)",
        );
        assert_eq!(out, "None None []");
    }

    #[test]
    fn a_missing_namespace_is_loaded_from_the_directory_as_before() {
        // R55: nothing autoloaded, so the file is looked up by name and loaded.
        let d = scratch("dirs");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("libmvtools.so"), b"").unwrap();
        let body = format!(
            "import os; os.environ['VS_PLUGIN_DIRS'] = {:?}\nprint(vhs_plugins.load('libmvtools.so') == {:?}, loaded == [{:?}])",
            d.display().to_string(),
            d.join("libmvtools.so").display().to_string(),
            d.join("libmvtools.so").display().to_string(),
        );
        let out = run_loader("load", &[], None, &body);
        let _ = fs::remove_dir_all(&d);
        assert_eq!(out, "True True");
    }

    #[test]
    fn the_core_s_own_plugin_folder_is_searched_when_it_has_one() {
        // A name no real plugin has: this test must not depend on what is installed on the host
        // (the vhs-gui package puts libffms2.so in /usr/lib/vhs-gui/vs-plugins).
        let own = scratch("own");
        fs::create_dir_all(&own).unwrap();
        fs::write(own.join("libvhsguitest.so"), b"").unwrap();
        let with = run_loader(
            "with-own",
            &[],
            Some(&own),
            "print(vhs_plugins.load('libvhsguitest.so'))",
        );
        assert_eq!(with, own.join("libvhsguitest.so").display().to_string());
        // R55 has no get_plugin_dir: the same file is then not found.
        let without = run_loader(
            "without-own",
            &[],
            None,
            "try:\n    vhs_plugins.load('libvhsguitest.so')\nexcept FileNotFoundError:\n    print('missing')",
        );
        assert_eq!(without, "missing");
        let _ = fs::remove_dir_all(&own);
    }

    #[test]
    fn a_plugin_that_is_nowhere_is_reported_with_the_folders_searched() {
        let body = "try:\n    vhs_plugins.load('libnothere.so')\nexcept FileNotFoundError as e:\n    print('missing' if 'libnothere.so' in str(e) else str(e))";
        let out = run_loader("nowhere", &[], None, body);
        assert_eq!(out, "missing");
    }

    const FAKE_HAVSFUNC: &str = "import types\n\
        haf = types.ModuleType('havsfunc')\n\
        haf.core = vs.core\n\
        original = haf.core\n";

    #[test]
    fn havsfunc_is_pointed_at_znedi3_when_only_znedi3_exists() {
        // R81: znedi3 and `std` exist, nnedi3 does not.
        let body = format!(
            "{FAKE_HAVSFUNC}vhs_plugins.prepare_havsfunc(haf)\n\
             print(haf.core.nnedi3 is vs.core.znedi3, haf.core.std is vs.core.std, haf.core is original)\n\
             haf.core.num_threads = 3\n\
             print(vs.core.num_threads)"
        );
        let out = run_loader("znedi3", &["znedi3", "std"], None, &body);
        assert_eq!(out, "True True False\n3");
    }

    #[test]
    fn nnedi3_is_not_remapped_when_it_is_present_or_znedi3_is_absent() {
        // Present: passed through untouched.
        let body = format!(
            "{FAKE_HAVSFUNC}vhs_plugins.prepare_havsfunc(haf)\nprint(haf.core.nnedi3 is vs.core.nnedi3)"
        );
        let out = run_loader("both", &["nnedi3", "znedi3"], None, &body);
        assert_eq!(out, "True");
        // Neither exists: the lookup fails as it always did, not silently.
        let body = format!(
            "{FAKE_HAVSFUNC}vhs_plugins.prepare_havsfunc(haf)\n\
             try:\n    haf.core.nnedi3\nexcept AttributeError:\n    print('missing')"
        );
        assert_eq!(run_loader("none", &[], None, &body), "missing");
    }

    #[test]
    fn a_missing_eedi3m_is_a_stand_in_that_only_fails_when_called() {
        // upstream havsfunc builds partial(core.eedi3m.EEDI3, ...) on every QTGMC call.
        let body = format!(
            "{FAKE_HAVSFUNC}vhs_plugins.prepare_havsfunc(haf)\n\
             from functools import partial\n\
             f = partial(haf.core.eedi3m.EEDI3, field=3)\n\
             print('built')\n\
             try:\n    f(None)\nexcept vs.Error as e:\n    print(e)"
        );
        let out = run_loader("eedi3m-missing", &[], None, &body);
        assert_eq!(
            out,
            "built\needi3m.EEDI3 needs the eedi3m plugin, which is not installed"
        );
    }

    #[test]
    fn an_installed_eedi3m_is_passed_through() {
        let body = format!(
            "{FAKE_HAVSFUNC}vhs_plugins.prepare_havsfunc(haf)\nprint(haf.core.eedi3m is vs.core.eedi3m)"
        );
        assert_eq!(run_loader("eedi3m-there", &["eedi3m"], None, &body), "True");
    }
}
