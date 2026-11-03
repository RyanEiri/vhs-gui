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
pub const FILES: [(&str, &str); 6] = [
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
}
