use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=OPENFANG_GIT_SHA");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");

    let sha = std::env::var("OPENFANG_GIT_SHA")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(read_git_sha)
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=OPENFANG_GIT_SHA={sha}");
}

fn read_git_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let sha = String::from_utf8(output.stdout).ok()?;
    let trimmed = sha.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
