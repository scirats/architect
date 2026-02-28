use architect_core::types::BatteryInfo;
use tracing::debug;

/// Detect battery status if present.
pub fn detect_battery() -> Option<BatteryInfo> {
    #[cfg(target_os = "linux")]
    {
        detect_battery_linux()
    }

    #[cfg(target_os = "macos")]
    {
        detect_battery_macos()
    }

    #[cfg(target_os = "android")]
    {
        detect_battery_android()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "android")))]
    {
        None
    }
}

/// Whether the agent should throttle work due to low battery.
pub fn should_throttle(battery: &BatteryInfo, limit_pct: u8) -> bool {
    !battery.charging && battery.level_pct < limit_pct
}

#[cfg(target_os = "linux")]
fn detect_battery_linux() -> Option<BatteryInfo> {
    let base = "/sys/class/power_supply";
    let entries = std::fs::read_dir(base).ok()?;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("BAT") {
            continue;
        }

        let path = entry.path();
        let capacity = std::fs::read_to_string(path.join("capacity"))
            .ok()?
            .trim()
            .parse::<u8>()
            .ok()?;

        let status = std::fs::read_to_string(path.join("status"))
            .ok()
            .unwrap_or_default();

        let charging = status.trim() == "Charging" || status.trim() == "Full";

        debug!("Battery detected: {}% (charging={})", capacity, charging);
        return Some(BatteryInfo {
            level_pct: capacity,
            charging,
        });
    }

    None
}

#[cfg(target_os = "macos")]
fn detect_battery_macos() -> Option<BatteryInfo> {
    use std::process::Command;

    let output = Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);

    // Parse output like: "InternalBattery-0 (id=...)	85%; charging; ..."
    for line in text.lines() {
        if !line.contains("InternalBattery") {
            continue;
        }

        let level_pct = line
            .split_whitespace()
            .find(|w| w.ends_with("%;"))
            .or_else(|| line.split_whitespace().find(|w| w.ends_with('%')))
            .and_then(|w| w.trim_end_matches("%;").trim_end_matches('%').parse::<u8>().ok())?;

        let charging = line.contains("charging") || line.contains("charged");

        debug!("Battery detected: {}% (charging={})", level_pct, charging);
        return Some(BatteryInfo {
            level_pct,
            charging,
        });
    }

    None
}

#[cfg(target_os = "android")]
fn detect_battery_android() -> Option<BatteryInfo> {
    // Android battery info via /sys or dumpsys
    let capacity = std::fs::read_to_string("/sys/class/power_supply/battery/capacity")
        .ok()?
        .trim()
        .parse::<u8>()
        .ok()?;

    let status = std::fs::read_to_string("/sys/class/power_supply/battery/status")
        .ok()
        .unwrap_or_default();

    let charging = status.trim() == "Charging" || status.trim() == "Full";

    Some(BatteryInfo {
        level_pct: capacity,
        charging,
    })
}
