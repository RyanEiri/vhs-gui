//! One resolver for every external tool vhs-gui spawns.
//!
//! Search order for each tool, first executable hit wins:
//!   1. an explicit override, `VHS_GUI_<TOOL>` (e.g. `VHS_GUI_FFMPEG=/opt/ffmpeg/bin/ffmpeg`)
//!   2. a bundled copy shipped with the application: `$APPDIR/usr/bin` (AppImage) or
//!      `<exe dir>/../lib/vhs-gui/bin`
//!   3. `/usr/bin`
//!   4. `~/bin`, then every directory on `PATH`
//!
//! Step 3 comes before `PATH` on purpose: ffmpeg here is the distro 8.0.1 build, and a
//! different ffmpeg in `/usr/local/bin` (the DeckLink-capable one) must not be picked up by
//! the processing code. The resolver itself ([`resolve_in`]) is pure so the order is testable.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Ffmpeg,
    Ffprobe,
    Sox,
    Vspipe,
    RealesrganRocm,
    RealesrganNcnn,
}

impl Tool {
    pub const ALL: [Tool; 6] = [
        Tool::Ffmpeg,
        Tool::Ffprobe,
        Tool::Sox,
        Tool::Vspipe,
        Tool::RealesrganRocm,
        Tool::RealesrganNcnn,
    ];

    /// Executable name looked up in each search directory.
    pub fn binary(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "ffmpeg",
            Tool::Ffprobe => "ffprobe",
            Tool::Sox => "sox",
            Tool::Vspipe => "vspipe",
            Tool::RealesrganRocm => "realesrgan-rocm",
            Tool::RealesrganNcnn => "realesrgan-ncnn-vulkan",
        }
    }

    /// Environment variable that overrides the search for this tool.
    pub fn env_override(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "VHS_GUI_FFMPEG",
            Tool::Ffprobe => "VHS_GUI_FFPROBE",
            Tool::Sox => "VHS_GUI_SOX",
            Tool::Vspipe => "VHS_GUI_VSPIPE",
            Tool::RealesrganRocm => "VHS_GUI_REALESRGAN_ROCM",
            Tool::RealesrganNcnn => "VHS_GUI_REALESRGAN_NCNN",
        }
    }

    /// What the tool is used for, shown in the Tools window.
    pub fn needed_for(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "capture, encode, fix A/V sync, audio cleanup, QTGMC output",
            Tool::Ffprobe => "duration and stream probing",
            Tool::Sox => "audio cleanup (hum notch, noise reduction)",
            Tool::Vspipe => "QTGMC, IVTC, VDecimate, field align (VapourSynth)",
            Tool::RealesrganRocm => "AI upscale, ROCm backend",
            Tool::RealesrganNcnn => "AI upscale, Vulkan (ncnn) backend",
        }
    }

    /// How to get the tool when it is missing.
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Ffmpeg | Tool::Ffprobe => "sudo apt-get install ffmpeg",
            Tool::Sox => "sudo apt-get install sox",
            Tool::Vspipe => "install VapourSynth (see the README system notes)",
            Tool::RealesrganRocm => "~/bin/realesrgan-rocm shim, see the README system notes",
            Tool::RealesrganNcnn => "~/bin/realesrgan-ncnn-vulkan, see the README system notes",
        }
    }

    fn version_arg(self) -> Option<&'static str> {
        match self {
            Tool::Ffmpeg | Tool::Ffprobe | Tool::Sox | Tool::Vspipe => Some("--version"),
            Tool::RealesrganRocm | Tool::RealesrganNcnn => None,
        }
    }
}

/// Directories searched for a tool, in priority order. Built from the environment by
/// [`SearchPath::from_env`]; tests build it by hand.
#[derive(Clone, Debug, Default)]
pub struct SearchPath {
    pub override_path: Option<PathBuf>,
    pub bundled: Vec<PathBuf>,
    pub system: Vec<PathBuf>,
    pub path: Vec<PathBuf>,
}

impl SearchPath {
    pub fn from_env(tool: Tool) -> Self {
        let override_path = std::env::var_os(tool.env_override())
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);

        let mut bundled = Vec::new();
        if let Some(appdir) = std::env::var_os("APPDIR").filter(|v| !v.is_empty()) {
            bundled.push(Path::new(&appdir).join("usr/bin"));
        }
        if let Some(exe_dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
        {
            bundled.push(exe_dir.join("../lib/vhs-gui/bin"));
        }

        let mut path = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            path.push(Path::new(&home).join("bin"));
        }
        if let Some(p) = std::env::var_os("PATH") {
            path.extend(std::env::split_paths(&p));
        }

        Self {
            override_path,
            bundled,
            system: vec![PathBuf::from("/usr/bin")],
            path,
        }
    }
}

fn is_executable(p: &Path) -> bool {
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// First executable match for `tool` in `search`, in priority order. An override that does
/// not point at an executable is reported as "not found" rather than silently ignored, so a
/// typo in `VHS_GUI_FFMPEG` is visible instead of falling back to another binary.
pub fn resolve_in(tool: Tool, search: &SearchPath) -> Option<PathBuf> {
    if let Some(o) = &search.override_path {
        return is_executable(o).then(|| o.clone());
    }
    search
        .bundled
        .iter()
        .chain(&search.system)
        .chain(&search.path)
        .map(|dir| dir.join(tool.binary()))
        .find(|p| is_executable(p))
}

pub fn resolve(tool: Tool) -> Option<PathBuf> {
    resolve_in(tool, &SearchPath::from_env(tool))
}

/// Resolved path, or an error naming the tool and how to get it. Call before spawning so a
/// missing tool fails with an actionable message and no half-started job.
pub fn require(tool: Tool) -> anyhow::Result<PathBuf> {
    resolve(tool).ok_or_else(|| {
        anyhow::anyhow!(
            "{} not found ({}). Set {} to point at it.",
            tool.binary(),
            tool.hint(),
            tool.env_override()
        )
    })
}

/// `Command` for `tool`. Falls back to the bare name when nothing resolves so the spawn
/// error is the usual "No such file" naming the tool; use [`require`] first for a nicer one.
pub fn command(tool: Tool) -> Command {
    Command::new(resolve(tool).unwrap_or_else(|| PathBuf::from(tool.binary())))
}

/// One row of the startup check.
#[derive(Clone, Debug)]
pub struct ToolStatus {
    pub tool: Tool,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
}

/// Resolve every tool and ask the found ones for their version. Spawns subprocesses, so call
/// it off the UI thread.
pub fn check_all() -> Vec<ToolStatus> {
    Tool::ALL
        .iter()
        .map(|&tool| {
            let path = resolve(tool);
            let version = path.as_deref().and_then(|p| probe_version(tool, p));
            ToolStatus {
                tool,
                path,
                version,
            }
        })
        .collect()
}

fn probe_version(tool: Tool, path: &Path) -> Option<String> {
    let arg = tool.version_arg()?;
    let out = Command::new(path).arg(arg).output().ok()?;
    parse_version(tool, &String::from_utf8_lossy(&out.stdout))
}

/// Pull a short version string out of a tool's `--version` output.
pub fn parse_version(tool: Tool, stdout: &str) -> Option<String> {
    match tool {
        // "ffmpeg version 8.0.1-3ubuntu1 Copyright (c) 2000-2025 the FFmpeg developers"
        Tool::Ffmpeg | Tool::Ffprobe => {
            let first = stdout.lines().next()?;
            let mut words = first.split_whitespace();
            words.find(|w| *w == "version")?;
            words.next().map(str::to_string)
        }
        // "sox:      SoX v14.4.2"
        Tool::Sox => stdout
            .lines()
            .flat_map(str::split_whitespace)
            .find(|w| w.starts_with('v') && w[1..].starts_with(|c: char| c.is_ascii_digit()))
            .map(str::to_string),
        // "VapourSynth Video Processing Library\nCopyright ...\nCore R55\nAPI R4.1\n..."
        Tool::Vspipe => stdout
            .lines()
            .find_map(|l| l.strip_prefix("Core "))
            .map(|v| v.trim().to_string()),
        Tool::RealesrganRocm | Tool::RealesrganNcnn => None,
    }
}

/// Human-readable problems with the toolset, empty when everything needed is present. An
/// upscaler is needed, but either backend will do.
pub fn problems(report: &[ToolStatus]) -> Vec<String> {
    let found = |t: Tool| report.iter().any(|s| s.tool == t && s.path.is_some());
    let mut out = Vec::new();
    for t in [Tool::Ffmpeg, Tool::Ffprobe, Tool::Sox, Tool::Vspipe] {
        if !found(t) {
            out.push(format!("{} missing: {}", t.binary(), t.hint()));
        }
    }
    if !found(Tool::RealesrganRocm) && !found(Tool::RealesrganNcnn) {
        out.push("no upscaler found: need realesrgan-rocm or realesrgan-ncnn-vulkan".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A fresh temp dir per test, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("vhs-gui-tools-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn dir(&self, sub: &str) -> PathBuf {
            let d = self.0.join(sub);
            fs::create_dir_all(&d).unwrap();
            d
        }

        /// Create `dir/name` with the given mode and return its path.
        fn file(&self, sub: &str, name: &str, mode: u32) -> PathBuf {
            let p = self.dir(sub).join(name);
            fs::write(&p, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
            p
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn search(s: &Scratch) -> SearchPath {
        SearchPath {
            override_path: None,
            bundled: vec![s.dir("bundled")],
            system: vec![s.dir("system")],
            path: vec![s.dir("home-bin"), s.dir("path")],
        }
    }

    #[test]
    fn bundled_beats_system_beats_path() {
        let s = Scratch::new("order");
        let path_hit = s.file("path", "ffmpeg", 0o755);
        let sp = search(&s);
        assert_eq!(resolve_in(Tool::Ffmpeg, &sp), Some(path_hit));

        let system_hit = s.file("system", "ffmpeg", 0o755);
        assert_eq!(resolve_in(Tool::Ffmpeg, &sp), Some(system_hit));

        let bundled_hit = s.file("bundled", "ffmpeg", 0o755);
        assert_eq!(resolve_in(Tool::Ffmpeg, &sp), Some(bundled_hit));
    }

    #[test]
    fn home_bin_comes_before_later_path_entries() {
        let s = Scratch::new("homebin");
        let home_hit = s.file("home-bin", "realesrgan-rocm", 0o755);
        s.file("path", "realesrgan-rocm", 0o755);
        assert_eq!(
            resolve_in(Tool::RealesrganRocm, &search(&s)),
            Some(home_hit)
        );
    }

    #[test]
    fn non_executable_file_is_skipped() {
        let s = Scratch::new("noexec");
        s.file("system", "sox", 0o644);
        let good = s.file("path", "sox", 0o755);
        assert_eq!(resolve_in(Tool::Sox, &search(&s)), Some(good));
    }

    #[test]
    fn directory_with_tool_name_is_not_a_hit() {
        let s = Scratch::new("dir");
        fs::create_dir_all(s.dir("system").join("vspipe")).unwrap();
        assert_eq!(resolve_in(Tool::Vspipe, &search(&s)), None);
    }

    #[test]
    fn missing_everywhere_is_none() {
        let s = Scratch::new("missing");
        assert_eq!(resolve_in(Tool::Ffprobe, &search(&s)), None);
    }

    #[test]
    fn override_wins_and_never_falls_back() {
        let s = Scratch::new("override");
        s.file("system", "ffmpeg", 0o755);
        let custom = s.file("custom", "my-ffmpeg", 0o755);
        let mut sp = search(&s);

        sp.override_path = Some(custom.clone());
        assert_eq!(resolve_in(Tool::Ffmpeg, &sp), Some(custom));

        // A bad override is "not found", not a silent fallback to /usr/bin.
        sp.override_path = Some(s.0.join("custom/does-not-exist"));
        assert_eq!(resolve_in(Tool::Ffmpeg, &sp), None);
    }

    #[test]
    fn version_parsing() {
        assert_eq!(
            parse_version(
                Tool::Ffmpeg,
                "ffmpeg version 8.0.1-3ubuntu1 Copyright (c) 2000-2025 the FFmpeg developers\nbuilt with gcc"
            ),
            Some("8.0.1-3ubuntu1".into())
        );
        assert_eq!(
            parse_version(Tool::Sox, "sox:      SoX v14.4.2\n"),
            Some("v14.4.2".into())
        );
        assert_eq!(
            parse_version(
                Tool::Vspipe,
                "VapourSynth Video Processing Library\nCopyright (c) Fredrik Mellbin\nCore R55\nAPI R4.1\n"
            ),
            Some("R55".into())
        );
        assert_eq!(parse_version(Tool::Ffmpeg, "garbage"), None);
        assert_eq!(parse_version(Tool::RealesrganRocm, "anything"), None);
    }

    fn status(tool: Tool, found: bool) -> ToolStatus {
        ToolStatus {
            tool,
            path: found.then(|| PathBuf::from("/x")),
            version: None,
        }
    }

    #[test]
    fn problems_lists_missing_tools_and_accepts_either_upscaler() {
        let all_ok = vec![
            status(Tool::Ffmpeg, true),
            status(Tool::Ffprobe, true),
            status(Tool::Sox, true),
            status(Tool::Vspipe, true),
            status(Tool::RealesrganRocm, false),
            status(Tool::RealesrganNcnn, true),
        ];
        assert!(problems(&all_ok).is_empty());

        let broken = vec![
            status(Tool::Ffmpeg, true),
            status(Tool::Ffprobe, false),
            status(Tool::Sox, true),
            status(Tool::Vspipe, true),
            status(Tool::RealesrganRocm, false),
            status(Tool::RealesrganNcnn, false),
        ];
        let p = problems(&broken);
        assert_eq!(p.len(), 2);
        assert!(p[0].starts_with("ffprobe missing"));
        assert!(p[1].starts_with("no upscaler found"));
    }

    #[test]
    fn require_error_names_tool_and_override_variable() {
        // Point the override at nothing so the result does not depend on this machine.
        let key = Tool::Sox.env_override();
        // SAFETY: tests in this module that touch this variable run it only here.
        unsafe { std::env::set_var(key, "/nonexistent/vhs-gui-test-sox") };
        let err = require(Tool::Sox).unwrap_err().to_string();
        unsafe { std::env::remove_var(key) };
        assert!(err.contains("sox not found"), "{err}");
        assert!(err.contains("VHS_GUI_SOX"), "{err}");
    }
}
