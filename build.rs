use std::process::Command;

fn main() {
    let hash = run_git(&["rev-parse", "--short=8", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let branch =
        run_git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = run_git(&["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    println!("cargo:rustc-env=VHS_GUI_GIT_HASH={hash}");
    println!("cargo:rustc-env=VHS_GUI_GIT_BRANCH={branch}");
    println!(
        "cargo:rustc-env=VHS_GUI_GIT_DIRTY={}",
        if dirty { "-dirty" } else { "" }
    );

    // Re-run when HEAD moves or the working tree is staged/unstaged so the
    // embedded commit/dirty flag never goes stale.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}

fn run_git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
