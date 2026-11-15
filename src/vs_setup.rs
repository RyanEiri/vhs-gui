//! "Set up VapourSynth": a private Python environment with VapourSynth R81, the three plugins that
//! ship as wheels (`mvtools`, `znedi3`, `vivtc`) and the two Python modules the scripts import
//! (`havsfunc`, `vsutil`). It lives in `~/.local/share/vhs-gui/vapoursynth` and needs no root, no PPA
//! and no system VapourSynth. The user starts it from the Tools window; nothing runs on its own.
//!
//! What gets installed is pinned in `assets/vs/requirements.txt`, one version and one sha256 per
//! package (regenerate it with `scripts/pin-vs-requirements.py`), and pip runs with
//! `--require-hashes --only-binary=:all: --no-deps`, so it can install nothing else. `havsfunc` is
//! the exception: PyPI's copy cannot run QTGMC here (it needs the `focus2` plugin, or in 34 a whole
//! other library), so one step downloads upstream's last single-file version at a pinned commit and
//! refuses it unless its sha256 matches ([`HAVSFUNC_COMMIT`], [`HAVSFUNC_SHA256`]). The plugins
//! without a wheel (`ffms2`, `fmtconv`, `miscfilters`) come from the `.deb`
//! (`/usr/lib/vhs-gui/vs-plugins`).
//!
//! The environment's `vspipe` is found by `tools::resolve` ahead of `/usr/bin`, so once it exists it
//! is the VapourSynth vhs-gui uses; deleting the directory goes back to the system one. A system
//! VapourSynth (the PPA's R55) is left alone: the R81 wheel registers itself in
//! `~/.config/vapoursynth/vapoursynth.toml` under its own library path (`vapoursynth config`), and
//! both coexist.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::pipeline::{PipelineJob, SeqStep};

/// havsfunc (Unlicense, HomeOfVapourSynthEvolution/havsfunc) at the last commit before upstream turned
/// it into a package: it imports only `vsutil`, smooths with `AverageFrames` (VapourSynth's
/// `misc` plugin) and runs QTGMC on R81 with the plugins listed in `vs_stack`.
pub const HAVSFUNC_COMMIT: &str = "40009447073c4845750aa5e2ecba606339ca9279";
pub const HAVSFUNC_SHA256: &str =
    "95f7342271e688e20845aaaa47d548ed6aede274a514bac666145d5514d0fe58";

/// Downloads havsfunc.py into the environment's `site-packages`, and writes nothing unless the
/// digest matches. Runs with the environment's own Python, so it needs nothing else installed.
fn fetch_havsfunc_script() -> String {
    format!(
        "import hashlib, os, sysconfig, urllib.request\n\
         url = 'https://raw.githubusercontent.com/HomeOfVapourSynthEvolution/havsfunc/{HAVSFUNC_COMMIT}/havsfunc.py'\n\
         data = urllib.request.urlopen(url, timeout=60).read()\n\
         digest = hashlib.sha256(data).hexdigest()\n\
         assert digest == '{HAVSFUNC_SHA256}', 'havsfunc.py has the wrong sha256: ' + digest\n\
         dest = os.path.join(sysconfig.get_paths()['purelib'], 'havsfunc.py')\n\
         tmp = dest + '.part'\n\
         open(tmp, 'wb').write(data)\n\
         os.replace(tmp, dest)\n\
         print('havsfunc.py', digest)\n"
    )
}

/// The job's label; the panel recognises the finished job by it and refreshes the Tools window.
pub const LABEL: &str = "Set up VapourSynth";

/// What the final check imports; a failure fails the job instead of leaving a half-working install.
const CHECK: &str = "import vapoursynth as vs, havsfunc, vsutil\n\
    core = vs.core\n\
    missing = [n for n in ('mv', 'znedi3', 'vivtc') if not hasattr(core, n)]\n\
    assert not missing, 'missing plugins: ' + ', '.join(missing)\n\
    print([l.strip() for l in str(core).splitlines() if 'Core' in l][0])\n";

/// `$XDG_DATA_HOME/vhs-gui` or `~/.local/share/vhs-gui`.
fn data_dir_from(xdg: Option<&std::ffi::OsStr>, home: Option<&std::ffi::OsStr>) -> PathBuf {
    let base = xdg
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| Path::new(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("vhs-gui")
}

/// The private environment: `<data dir>/vapoursynth`.
pub fn venv_dir() -> PathBuf {
    data_dir_from(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
    .join("vapoursynth")
}

/// One command of the setup.
#[derive(Debug, PartialEq, Eq)]
pub struct Step {
    pub label: &'static str,
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// The five commands, in order: create the environment, install the pinned wheels, fetch the pinned
/// havsfunc, register VapourSynth with its config file, and check that it imports with every plugin
/// present.
pub fn steps(python: &Path, venv: &Path, requirements: &Path) -> Vec<Step> {
    let vpython = venv.join("bin/python");
    let mut install = os(&[
        "-m",
        "pip",
        "install",
        "--require-hashes",
        "--only-binary=:all:",
        "--no-deps",
        "--disable-pip-version-check",
        "--no-input",
        "-r",
    ]);
    install.push(requirements.into());

    let mut create = os(&["-m", "venv"]);
    create.push(venv.into());

    vec![
        Step {
            label: "Creating the Python environment...",
            program: python.into(),
            args: create,
        },
        Step {
            label: "Installing VapourSynth and its plugins (about 25 MB)...",
            program: vpython.clone(),
            args: install,
        },
        Step {
            label: "Downloading havsfunc...",
            program: vpython.clone(),
            args: vec!["-c".into(), fetch_havsfunc_script().into()],
        },
        Step {
            label: "Registering VapourSynth...",
            program: venv.join("bin/vapoursynth"),
            args: os(&["config"]),
        },
        Step {
            label: "Checking the install...",
            program: vpython,
            args: os(&["-c", CHECK]),
        },
    ]
}

/// `/usr/bin/python3` first (the distro's, whose `venv` module is the one that will be tested),
/// then whatever `python3` is on `PATH`.
fn find_python() -> Option<PathBuf> {
    let first = PathBuf::from("/usr/bin/python3");
    if first.is_file() {
        return Some(first);
    }
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("python3"))
            .find(|f| f.is_file())
    })
}

/// Start the setup as a background job (progress and Cancel work like any other job). Refuses with
/// an actionable message when Python, its `venv` module or its shared library is missing.
pub fn launch(cfg: &Config, label: String) -> anyhow::Result<PipelineJob> {
    let python = find_python().ok_or_else(|| {
        anyhow::anyhow!("python3 not found. Install it with: sudo apt install python3")
    })?;
    let venv_ok = Command::new(&python)
        .args(["-c", "import venv, ensurepip"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !venv_ok {
        anyhow::bail!(
            "{} cannot create virtual environments. Install the venv module with: \
             sudo apt install python3-venv",
            python.display()
        );
    }

    // VapourSynth's R81 wheel loads Python as a shared library (`vapoursynth config` records it), and
    // Ubuntu ships that in a separate package.
    let libpython_ok = Command::new(&python)
        .args([
            "-c",
            "import ctypes, sysconfig; ctypes.CDLL(sysconfig.get_config_var('LDLIBRARY'))",
        ])
        .output()
        .is_ok_and(|o| o.status.success());
    if !libpython_ok {
        anyhow::bail!(
            "{} has no shared libpython, which VapourSynth needs. Install the one matching \
             your Python, for example: sudo apt install libpython3.14",
            python.display()
        );
    }

    let requirements = crate::vs_assets::materialize()?.join("requirements.txt");
    let venv = venv_dir();
    if let Some(parent) = venv.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let seq: Vec<(&'static str, SeqStep)> = steps(&python, &venv, &requirements)
        .into_iter()
        .map(|s| {
            let mut cmd = Command::new(&s.program);
            cmd.args(&s.args)
                .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
                .env("PIP_NO_INPUT", "1");
            (s.label, SeqStep::Cmd(cmd))
        })
        .collect();
    PipelineJob::start_native_sequence(label, &requirements, seq, None, &cfg.log_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pinned wheels, as written by `scripts/pin-vs-requirements.py` (installed from the copy
    /// `vs_assets` writes to the cache, which embeds this same file).
    const REQUIREMENTS: &str = include_str!("../assets/vs/requirements.txt");

    #[test]
    fn the_data_dir_follows_xdg_then_home() {
        use std::ffi::OsStr;
        assert_eq!(
            data_dir_from(Some(OsStr::new("/x")), Some(OsStr::new("/home/u"))),
            Path::new("/x/vhs-gui")
        );
        assert_eq!(
            data_dir_from(None, Some(OsStr::new("/home/u"))),
            Path::new("/home/u/.local/share/vhs-gui")
        );
        // An empty XDG_DATA_HOME counts as unset, as the spec says.
        assert_eq!(
            data_dir_from(Some(OsStr::new("")), Some(OsStr::new("/home/u"))),
            Path::new("/home/u/.local/share/vhs-gui")
        );
    }

    #[test]
    fn the_setup_runs_five_commands_in_order_inside_the_private_environment() {
        let s = steps(
            Path::new("/usr/bin/python3"),
            Path::new("/d/vapoursynth"),
            Path::new("/c/requirements.txt"),
        );
        let programs: Vec<_> = s.iter().map(|x| x.program.to_str().unwrap()).collect();
        assert_eq!(
            programs,
            [
                "/usr/bin/python3",
                "/d/vapoursynth/bin/python",
                "/d/vapoursynth/bin/python",
                "/d/vapoursynth/bin/vapoursynth",
                "/d/vapoursynth/bin/python",
            ]
        );
        assert_eq!(s[0].args, ["-m", "venv", "/d/vapoursynth"]);
        assert_eq!(s[3].args, ["config"]);
        // Only pinned wheels: hashes required, binaries only, no dependency resolution.
        let install: Vec<_> = s[1].args.iter().map(|a| a.to_str().unwrap()).collect();
        for flag in ["--require-hashes", "--only-binary=:all:", "--no-deps"] {
            assert!(install.contains(&flag), "{flag} missing");
        }
        assert_eq!(install.last(), Some(&"/c/requirements.txt"));
    }

    #[test]
    fn every_requirement_is_pinned_to_a_version_and_a_hash() {
        // Pairs of lines: `name==version \` then `--hash=sha256:...  # wheel file name`.
        let lines: Vec<&str> = REQUIREMENTS
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect();
        assert_eq!(lines.len() % 2, 0, "{lines:?}");
        let mut names = Vec::new();
        for pair in lines.chunks(2) {
            let req = pair[0].trim_end_matches('\\').trim();
            let (name, version) = req.split_once("==").expect("pinned with ==");
            assert!(!version.contains(['>', '<', '*', ',']), "{req}");
            let hash = pair[1].split_whitespace().next().unwrap();
            let digest = hash.strip_prefix("--hash=sha256:").expect("a sha256 hash");
            assert!(
                digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
                "{name}: {hash}"
            );
            names.push(name.to_string());
        }
        // The five wheels the scripts need and nothing else. havsfunc is fetched separately (PyPI's
        // 33 needs the `focus2` plugin and 34 needs vsjetpack).
        names.sort();
        let mut want = [
            "VapourSynth",
            "vapoursynth-mvtools",
            "vapoursynth-znedi3",
            "vapoursynth-vivtc",
            "vsutil",
        ];
        want.sort_unstable();
        assert_eq!(names, want);
        assert!(REQUIREMENTS.contains("VapourSynth==81"));
    }

    #[test]
    fn havsfunc_is_fetched_at_a_pinned_commit_and_refused_unless_the_digest_matches() {
        assert_eq!(HAVSFUNC_COMMIT.len(), 40);
        assert!(HAVSFUNC_COMMIT.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(HAVSFUNC_SHA256.len(), 64);
        assert!(HAVSFUNC_SHA256.bytes().all(|b| b.is_ascii_hexdigit()));
        let script = fetch_havsfunc_script();
        // The commit is in the URL (not a branch name), and the digest is checked before anything is
        // written.
        assert!(script.contains(&format!("/havsfunc/{HAVSFUNC_COMMIT}/havsfunc.py")));
        let check = script.find(&format!("== '{HAVSFUNC_SHA256}'")).unwrap();
        let write = script.find("open(tmp, 'wb')").unwrap();
        assert!(check < write);
        // It must at least be valid Python.
        let ok = Command::new("python3")
            .args([
                "-I",
                "-c",
                "import ast, sys; ast.parse(sys.argv[1])",
                &script,
            ])
            .status()
            .unwrap();
        assert!(ok.success());
    }

    #[test]
    fn the_check_script_names_every_plugin_the_wheels_provide() {
        for ns in ["mv", "znedi3", "vivtc"] {
            assert!(CHECK.contains(&format!("'{ns}'")), "{ns}");
        }
    }

    /// Runs the real five commands in a scratch home: downloads the pinned wheels from PyPI (about
    /// 25 MB) and havsfunc from GitHub, so it needs network access and the `python3-venv` package. The
    /// last step imports the downloaded havsfunc, so run it only where executing that is acceptable
    /// (it was verified in a container instead). Not run by default
    /// (`cargo test -- --ignored vs_setup`). The scratch dir is under the cache dir, not /tmp (RAM).
    #[test]
    #[ignore]
    fn the_setup_installs_a_working_environment_end_to_end() {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        let scratch = home.join(".cache/vhs-gui-setup-e2e");
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();

        let venv = scratch.join("data/vhs-gui/vapoursynth");
        let req = scratch.join("requirements.txt");
        std::fs::write(&req, REQUIREMENTS).unwrap();
        for step in steps(&find_python().unwrap(), &venv, &req) {
            let out = Command::new(&step.program)
                .args(&step.args)
                // Registration writes ~/.config/vapoursynth: keep it away from the real one.
                .env("HOME", &scratch)
                .env("XDG_CONFIG_HOME", scratch.join("config"))
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}\n{}{}",
                step.label,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        assert!(venv.join("bin/vspipe").is_file());
        assert!(
            scratch
                .join("config/vapoursynth/vapoursynth.toml")
                .is_file()
        );
        // A second run (a repair, or a user pressing the button again) must also succeed.
        for step in steps(&find_python().unwrap(), &venv, &req) {
            let out = Command::new(&step.program)
                .args(&step.args)
                .env("HOME", &scratch)
                .env("XDG_CONFIG_HOME", scratch.join("config"))
                .output()
                .unwrap();
            assert!(out.status.success(), "second run: {}", step.label);
        }
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
