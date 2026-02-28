use std::net::IpAddr;

use tracing::{debug, info};

/// SSH-based agent deployment.
pub struct SshBootstrapper {
    timeout: u64,
}

impl SshBootstrapper {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout: timeout_secs }
    }

    /// Connect to a remote host via SSH (or sshpass + ssh for password auth).
    pub async fn connect(
        &self,
        host: IpAddr,
        user: &str,
        password: Option<&str>,
    ) -> anyhow::Result<SshSession> {
        info!("SSH connecting to {}@{}", user, host);

        if password.is_some() {
            // Verify sshpass is installed
            let check = tokio::process::Command::new("which")
                .arg("sshpass")
                .output()
                .await;
            if check.is_err() || !check.unwrap().status.success() {
                anyhow::bail!(
                    "sshpass not installed (required for --pass). Install: apt install sshpass / brew install hudochenkov/sshpass/sshpass"
                );
            }
        }

        let session = SshSession {
            host,
            user: user.to_string(),
            password: password.map(String::from),
            timeout: self.timeout,
        };

        // Test connectivity
        session
            .run("echo ok")
            .await
            .map_err(|e| anyhow::anyhow!("ssh {}@{}: {}", user, host, e))?;

        info!("SSH connected to {}", host);
        Ok(session)
    }
}

/// An active SSH session that runs commands via the system ssh binary.
/// Uses ControlMaster for connection reuse across commands.
pub struct SshSession {
    host: IpAddr,
    user: String,
    password: Option<String>,
    timeout: u64,
}

impl SshSession {
    fn build_command(&self) -> tokio::process::Command {
        let mut cmd;

        if let Some(ref pass) = self.password {
            cmd = tokio::process::Command::new("sshpass");
            cmd.env("SSHPASS", pass);
            cmd.args(["-e", "ssh"]);
        } else {
            cmd = tokio::process::Command::new("ssh");
            cmd.args(["-o", "BatchMode=yes"]);
        }

        cmd.args([
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            &format!("ConnectTimeout={}", self.timeout),
            "-o",
            "ControlMaster=auto",
            "-o",
            &format!(
                "ControlPath=/tmp/architect-ssh-{}@{}",
                self.user, self.host
            ),
            "-o",
            "ControlPersist=60",
            &format!("{}@{}", self.user, self.host),
        ]);

        cmd
    }

    /// Run a command on the remote host. Returns stdout on success.
    pub async fn run(&self, remote_cmd: &str) -> anyhow::Result<String> {
        let mut cmd = self.build_command();
        cmd.arg(remote_cmd);

        let output = cmd
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("failed to spawn ssh: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let msg = if stderr.is_empty() {
                format!("exit code {}", output.status.code().unwrap_or(-1))
            } else {
                stderr
            };
            anyhow::bail!("{}", msg);
        }

        Ok(String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_string())
    }

    /// Detect the remote architecture via uname -m.
    pub async fn detect_arch(&self) -> anyhow::Result<String> {
        let arch = self.run("uname -m").await?;
        debug!("Remote architecture: {}", arch);
        Ok(arch)
    }

    /// Detect the remote OS via uname -s.
    pub async fn detect_os(&self) -> anyhow::Result<String> {
        let os = self.run("uname -s").await?;
        debug!("Remote OS: {}", os);
        Ok(os)
    }

    /// Start the agent in the background on the remote host.
    pub async fn start_agent(
        &self,
        agent_path: &str,
        agent_args: &str,
    ) -> anyhow::Result<()> {
        info!("Starting agent on remote host");

        let cmd = format!(
            "nohup {} {} > /tmp/architect-agent.log 2>&1 &",
            agent_path, agent_args
        );
        self.run(&cmd).await?;

        info!("Agent started in background");
        Ok(())
    }
}
