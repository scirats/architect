use std::net::IpAddr;

use tracing::{error, info, warn};

use architect_core::types::DeviceType;

/// Result of a bootstrap attempt.
#[derive(Debug, Clone)]
pub struct BootstrapResult {
    pub host: IpAddr,
    pub success: bool,
    pub method: BootstrapMethod,
    pub error: Option<String>,
}

/// Method used for bootstrapping.
#[derive(Debug, Clone, Copy)]
pub enum BootstrapMethod {
    Ssh,
    Adb,
    WinRm,
    Manual,
}

/// Target for bootstrap.
pub struct BootstrapTarget {
    pub host: IpAddr,
    pub device_type: DeviceType,
    pub ssh_user: Option<String>,
    pub ssh_pass: Option<String>,
    pub winrm_credentials: Option<(String, String)>,
}

/// Map uname output to a platform label matching GitHub Release asset names.
fn uname_to_label(os: &str, arch: &str) -> Option<String> {
    let os_part = match os {
        "Linux" => "linux",
        "Darwin" => "macos",
        _ => return None,
    };
    let arch_part = match arch {
        "x86_64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        "armv7l" => "armv7",
        _ => return None,
    };
    Some(format!("{}-{}", os_part, arch_part))
}

/// Map an architecture string to the normalized arch part used in labels.
#[cfg(any(feature = "adb", feature = "winrm"))]
fn normalize_arch(arch: &str) -> Option<&str> {
    match arch {
        "x86_64" => Some("x86_64"),
        "aarch64" | "arm64" => Some("aarch64"),
        "armv7l" => Some("armv7"),
        _ => None,
    }
}

/// Construct the GitHub Release download URL for an agent binary.
fn github_agent_url(github_repo: &str, label: &str) -> String {
    let ext = if label.starts_with("windows") { ".exe" } else { "" };
    format!(
        "https://github.com/{}/releases/latest/download/architect-agent-{}{}",
        github_repo, label, ext,
    )
}

/// Orchestrates agent deployment across discovered nodes.
/// Tries appropriate methods based on device type: SSH -> ADB -> WinRM.
/// Callback for reporting progress steps during bootstrap.
pub type ProgressFn = Box<dyn Fn(&str) + Send + Sync>;

pub struct BootstrapOrchestrator {
    github_repo: Option<String>,
    cluster_token: Option<String>,
    coordinator_addr: Option<String>,
    on_progress: Option<ProgressFn>,
}

impl BootstrapOrchestrator {
    pub fn new() -> Self {
        Self {
            github_repo: None,
            cluster_token: None,
            coordinator_addr: None,
            on_progress: None,
        }
    }

    pub fn set_github_repo(&mut self, repo: String) {
        self.github_repo = Some(repo);
    }

    pub fn set_cluster_token(&mut self, token: String) {
        self.cluster_token = Some(token);
    }

    pub fn set_coordinator_addr(&mut self, addr: String) {
        self.coordinator_addr = Some(addr);
    }

    pub fn set_on_progress(&mut self, f: ProgressFn) {
        self.on_progress = Some(f);
    }

    fn progress(&self, msg: &str) {
        info!("{}", msg);
        if let Some(f) = &self.on_progress {
            f(msg);
        }
    }

    fn agent_args(&self) -> String {
        let mut parts = Vec::new();
        if let Some(addr) = &self.coordinator_addr {
            parts.push(format!("--coordinator {}", addr));
        }
        if let Some(token) = &self.cluster_token {
            parts.push(format!("--token {}", token));
        }
        parts.join(" ")
    }

    /// Bootstrap a single target, trying methods in priority order.
    pub async fn bootstrap_target(&self, target: &BootstrapTarget) -> BootstrapResult {
        info!("Bootstrapping {} (type: {:?})", target.host, target.device_type);

        if self.github_repo.is_none() {
            return BootstrapResult {
                host: target.host,
                success: false,
                method: BootstrapMethod::Manual,
                error: Some("github_repo not configured".into()),
            };
        }

        let mut errors: Vec<String> = Vec::new();

        match target.device_type {
            DeviceType::Phone => {
                // Try ADB for phones
                #[cfg(feature = "adb")]
                {
                    match self.try_adb(target).await {
                        Ok(()) => {
                            return BootstrapResult {
                                host: target.host,
                                success: true,
                                method: BootstrapMethod::Adb,
                                error: None,
                            };
                        }
                        Err(e) => {
                            warn!("ADB bootstrap failed for {}: {}", target.host, e);
                            errors.push(format!("adb: {}", e));
                        }
                    }
                }
            }
            DeviceType::Desktop | DeviceType::Laptop | DeviceType::RaspberryPi => {
                // Try SSH first
                #[cfg(feature = "ssh")]
                if let Some(user) = &target.ssh_user {
                    match self.try_ssh(target, user).await {
                        Ok(()) => {
                            return BootstrapResult {
                                host: target.host,
                                success: true,
                                method: BootstrapMethod::Ssh,
                                error: None,
                            };
                        }
                        Err(e) => {
                            warn!("SSH bootstrap failed for {}: {}", target.host, e);
                            errors.push(format!("ssh: {}", e));
                        }
                    }
                }

                // Try WinRM for desktops
                #[cfg(feature = "winrm")]
                if let Some((user, pass)) = &target.winrm_credentials {
                    match self.try_winrm(target, user, pass).await {
                        Ok(()) => {
                            return BootstrapResult {
                                host: target.host,
                                success: true,
                                method: BootstrapMethod::WinRm,
                                error: None,
                            };
                        }
                        Err(e) => {
                            warn!("WinRM bootstrap failed for {}: {}", target.host, e);
                            errors.push(format!("winrm: {}", e));
                        }
                    }
                }
            }
            _ => {}
        }

        let detail = if errors.is_empty() {
            "no bootstrap method available".into()
        } else {
            errors.join("; ")
        };

        error!("All bootstrap methods failed for {}: {}", target.host, detail);

        BootstrapResult {
            host: target.host,
            success: false,
            method: BootstrapMethod::Manual,
            error: Some(detail),
        }
    }

    /// Bootstrap multiple targets in parallel.
    pub async fn bootstrap_all(&self, targets: &[BootstrapTarget]) -> Vec<BootstrapResult> {
        let mut handles = Vec::new();

        for target in targets {
            let result = self.bootstrap_target(target).await;
            handles.push(result);
        }

        let successful = handles.iter().filter(|r| r.success).count();
        info!(
            "Bootstrap complete: {}/{} successful",
            successful,
            handles.len()
        );

        handles
    }

    #[cfg(feature = "ssh")]
    async fn try_ssh(&self, target: &BootstrapTarget, user: &str) -> anyhow::Result<()> {
        let repo = self.github_repo.as_ref().unwrap();
        let ssh = super::ssh::SshBootstrapper::new(30);

        self.progress("connecting via ssh...");
        let session = ssh.connect(target.host, user, target.ssh_pass.as_deref()).await?;

        self.progress("detecting platform...");
        let os = session.detect_os().await?;
        let arch = session.detect_arch().await?;

        let label = uname_to_label(&os, &arch)
            .ok_or_else(|| anyhow::anyhow!("Unsupported platform: {} {}", os, arch))?;
        self.progress(&format!("platform: {} ({})", label, arch));

        let download_url = github_agent_url(repo, &label);
        let remote_path = "/tmp/architect-agent";

        self.progress(&format!("downloading agent from {}...", download_url));
        let dl_result = session
            .run(&format!(
                "curl -fsSL -o {} '{}' 2>&1",
                remote_path, download_url
            ))
            .await;

        if let Err(e) = dl_result {
            // curl -f returns exit 22 on HTTP errors (404, etc.)
            anyhow::bail!(
                "Download failed: {}. URL: {}",
                e, download_url
            );
        }

        // Verify the downloaded file is an actual binary, not an HTML error page
        self.progress("verifying binary...");
        let file_info = session
            .run(&format!("file {}", remote_path))
            .await
            .unwrap_or_default();

        if file_info.contains("HTML")
            || file_info.contains("ASCII text")
            || file_info.contains("empty")
        {
            // Clean up the invalid file
            let _ = session.run(&format!("rm -f {}", remote_path)).await;
            anyhow::bail!(
                "Downloaded file is not a valid binary ({}). Verify release exists at: {}",
                file_info, download_url
            );
        }

        self.progress(&format!("binary verified: {}", file_info.split(':').last().unwrap_or("ok").trim()));

        session
            .run(&format!("chmod +x {}", remote_path))
            .await
            .map_err(|e| anyhow::anyhow!("chmod failed: {}", e))?;

        self.progress("starting agent...");
        session.start_agent(remote_path, &self.agent_args()).await?;

        // Verify the agent process is actually running
        self.progress("verifying agent process...");
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let ps_check = session
            .run("pgrep -f architect-agent || true")
            .await
            .unwrap_or_default();

        if ps_check.trim().is_empty() {
            // Agent didn't stay running — check its log for clues
            let log = session
                .run("tail -5 /tmp/architect-agent.log 2>/dev/null || echo 'no log'")
                .await
                .unwrap_or_else(|_| "could not read log".into());
            anyhow::bail!(
                "Agent binary started but exited immediately. Log:\n{}",
                log
            );
        }

        self.progress("agent is running, waiting for discovery...");

        Ok(())
    }

    #[cfg(feature = "adb")]
    async fn try_adb(&self, target: &BootstrapTarget) -> anyhow::Result<()> {
        let repo = self.github_repo.as_ref().unwrap();
        let adb = super::adb::AdbBootstrapper::new();
        adb.connect(target.host, 5555).await?;

        let arch = adb.detect_arch(target.host).await?;
        let arch_part = normalize_arch(&arch)
            .ok_or_else(|| anyhow::anyhow!("Unsupported Android arch: {}", arch))?;
        let label = format!("android-{}", arch_part);
        let download_url = github_agent_url(repo, &label);
        let remote_path = "/data/local/tmp/architect-agent";

        // Download directly on the device
        let serial = format!("{}:5555", target.host);
        let dl_cmd = format!("curl -fsSL -o {} '{}'", remote_path, download_url);
        let output = tokio::process::Command::new("adb")
            .args(["-s", &serial, "shell", &dl_cmd])
            .output()
            .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ADB download failed: {}. URL: {}", stderr.trim(), download_url);
        }

        // chmod +x
        let output = tokio::process::Command::new("adb")
            .args(["-s", &serial, "shell", "chmod", "+x", remote_path])
            .output()
            .await?;
        if !output.status.success() {
            anyhow::bail!("ADB chmod failed");
        }

        adb.start_agent(target.host, remote_path, &self.agent_args())
            .await?;
        Ok(())
    }

    #[cfg(feature = "winrm")]
    async fn try_winrm(
        &self,
        target: &BootstrapTarget,
        user: &str,
        pass: &str,
    ) -> anyhow::Result<()> {
        let repo = self.github_repo.as_ref().unwrap();
        let winrm = super::winrm::WinRmBootstrapper::new(5985);

        // Detect architecture via PowerShell
        let arch_raw = winrm
            .invoke_command(target.host, user, pass, "$env:PROCESSOR_ARCHITECTURE")
            .await?;
        let arch_part = match arch_raw.trim() {
            "AMD64" => "x86_64",
            "ARM64" => "aarch64",
            other => anyhow::bail!("Unsupported Windows arch: {}", other),
        };
        let label = format!("windows-{}", arch_part);
        let download_url = github_agent_url(repo, &label);

        winrm
            .deploy_agent(target.host, user, pass, &download_url, &self.agent_args())
            .await?;

        Ok(())
    }
}
