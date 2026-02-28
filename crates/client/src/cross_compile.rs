use std::path::PathBuf;
use architect_core::types::{OsKind, ArchKind};

pub const TARGETS: &[(&str, &str)] = &[
    // Linux
    ("x86_64-unknown-linux-gnu",      "linux-x86_64"),
    ("aarch64-unknown-linux-gnu",     "linux-aarch64"),
    ("armv7-unknown-linux-gnueabihf", "linux-armv7"),
    // macOS
    ("x86_64-apple-darwin",           "macos-x86_64"),
    ("aarch64-apple-darwin",          "macos-aarch64"),
    // Windows
    ("x86_64-pc-windows-gnu",        "windows-x86_64"),
    ("aarch64-pc-windows-gnullvm",   "windows-aarch64"),
    // Android
    ("aarch64-linux-android",        "android-aarch64"),
    ("armv7-linux-androideabi",      "android-armv7"),
    ("x86_64-linux-android",         "android-x86_64"),
    // iOS
    ("aarch64-apple-ios",            "ios-aarch64"),
];

/// Directory where agent binaries are stored.
pub fn builds_dir() -> PathBuf {
    let dir = architect_core::config::data_dir().join("builds");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Resolve the label (e.g. "linux-x86_64") for a target triple.
pub fn label_for_target(target: &str) -> &str {
    TARGETS.iter()
        .find(|(t, _)| *t == target)
        .map(|(_, l)| *l)
        .unwrap_or(target)
}

/// Construct the GitHub Release download URL for an agent binary.
pub fn github_agent_url(github_repo: &str, label: &str) -> String {
    let ext = if label.starts_with("windows") { ".exe" } else { "" };
    format!(
        "https://github.com/{}/releases/latest/download/architect-agent-{}{}",
        github_repo, label, ext,
    )
}

/// Map platform enums to a Rust target triple.
pub fn target_for_platform(os: OsKind, arch: ArchKind) -> Option<&'static str> {
    match (os, arch) {
        // Linux
        (OsKind::Linux,   ArchKind::X86_64)  => Some("x86_64-unknown-linux-gnu"),
        (OsKind::Linux,   ArchKind::Aarch64) => Some("aarch64-unknown-linux-gnu"),
        (OsKind::Linux,   ArchKind::Armv7)   => Some("armv7-unknown-linux-gnueabihf"),
        // macOS
        (OsKind::MacOS,   ArchKind::X86_64)  => Some("x86_64-apple-darwin"),
        (OsKind::MacOS,   ArchKind::Aarch64) => Some("aarch64-apple-darwin"),
        // Windows
        (OsKind::Windows, ArchKind::X86_64)  => Some("x86_64-pc-windows-gnu"),
        (OsKind::Windows, ArchKind::Aarch64) => Some("aarch64-pc-windows-gnullvm"),
        // Android
        (OsKind::Android, ArchKind::Aarch64) => Some("aarch64-linux-android"),
        (OsKind::Android, ArchKind::Armv7)   => Some("armv7-linux-androideabi"),
        (OsKind::Android, ArchKind::X86_64)  => Some("x86_64-linux-android"),
        // iOS
        (OsKind::IOS,     ArchKind::Aarch64) => Some("aarch64-apple-ios"),
        _ => None,
    }
}
