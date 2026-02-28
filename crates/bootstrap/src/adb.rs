use std::net::IpAddr;

use tracing::{debug, info};

/// ADB-based agent deployment for Android devices.
pub struct AdbBootstrapper;

impl AdbBootstrapper {
    pub fn new() -> Self {
        Self
    }

    /// Connect to an Android device via ADB.
    pub async fn connect(&self, host: IpAddr, port: u16) -> anyhow::Result<()> {
        let addr = format!("{}:{}", host, port);
        info!("ADB connecting to {}", addr);

        let output = tokio::process::Command::new("adb")
            .arg("connect")
            .arg(&addr)
            .output()
            .await?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.contains("connected") {
            anyhow::bail!("ADB connect failed: {}", stdout);
        }

        info!("ADB connected to {}", addr);
        Ok(())
    }

    /// Detect the architecture of the Android device via `uname -m`.
    pub async fn detect_arch(&self, host: IpAddr) -> anyhow::Result<String> {
        let serial = format!("{}:5555", host);
        let output = tokio::process::Command::new("adb")
            .args(["-s", &serial, "shell", "uname", "-m"])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ADB uname failed: {}", stderr);
        }

        let arch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        debug!("Android device {} arch: {}", host, arch);
        Ok(arch)
    }

    /// Push agent binary to Android device.
    pub async fn push_agent(
        &self,
        host: IpAddr,
        local_path: &str,
        remote_path: &str,
    ) -> anyhow::Result<()> {
        info!("ADB pushing {} to {}:{}", local_path, host, remote_path);

        let serial = format!("{}:5555", host);
        let output = tokio::process::Command::new("adb")
            .arg("-s")
            .arg(&serial)
            .arg("push")
            .arg(local_path)
            .arg(remote_path)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ADB push failed: {}", stderr);
        }

        debug!("ADB push completed");
        Ok(())
    }

    /// Start agent on Android device.
    pub async fn start_agent(
        &self,
        host: IpAddr,
        agent_path: &str,
        agent_args: &str,
    ) -> anyhow::Result<()> {
        info!("Starting agent on Android device {}", host);

        let serial = format!("{}:5555", host);
        let cmd = format!(
            "nohup {} {} > /data/local/tmp/architect-agent.log 2>&1 &",
            agent_path, agent_args
        );

        let output = tokio::process::Command::new("adb")
            .arg("-s")
            .arg(&serial)
            .arg("shell")
            .arg(&cmd)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ADB shell failed: {}", stderr);
        }

        info!("Agent started on Android device");
        Ok(())
    }
}

impl Default for AdbBootstrapper {
    fn default() -> Self {
        Self::new()
    }
}
