use std::net::IpAddr;
use std::time::Duration;

use openssh::{KnownHosts, Session, SessionBuilder};
use tracing::{debug, error, info};

/// SSH-based agent deployment.
pub struct SshBootstrapper {
    timeout: Duration,
}

impl SshBootstrapper {
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    /// Connect to a remote host via SSH.
    pub async fn connect(&self, host: IpAddr, user: &str) -> anyhow::Result<Session> {
        let dest = format!("ssh://{}@{}", user, host);

        info!("SSH connecting to {}", dest);

        let session = SessionBuilder::default()
            .known_hosts_check(KnownHosts::Accept)
            .connect_timeout(self.timeout)
            .connect(&dest)
            .await
            .map_err(|e| anyhow::anyhow!("SSH connection failed: {}", e))?;

        info!("SSH connected to {}", host);
        Ok(session)
    }

    /// Detect the remote architecture via uname.
    pub async fn detect_arch(&self, session: &Session) -> anyhow::Result<String> {
        let output = session
            .command("uname")
            .arg("-m")
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to run uname: {}", e))?;

        let arch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        debug!("Remote architecture: {}", arch);
        Ok(arch)
    }

    /// Detect the remote OS.
    pub async fn detect_os(&self, session: &Session) -> anyhow::Result<String> {
        let output = session
            .command("uname")
            .arg("-s")
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to run uname -s: {}", e))?;

        let os = String::from_utf8_lossy(&output.stdout).trim().to_string();
        debug!("Remote OS: {}", os);
        Ok(os)
    }

    /// Upload agent binary using base64 encoding over SSH.
    pub async fn upload_agent(
        &self,
        session: &Session,
        local_path: &str,
        remote_path: &str,
    ) -> anyhow::Result<()> {
        // Validate remote path to prevent shell injection
        if remote_path.chars().any(|c| matches!(c, '\'' | ';' | '|' | '`' | '$' | '&' | '>' | '<')) {
            anyhow::bail!("Invalid remote path: contains shell metacharacters");
        }

        info!("Uploading agent from {} to {}", local_path, remote_path);

        // Read local file and encode as base64 for transfer
        let data = tokio::fs::read(local_path).await?;
        let encoded = data_encoding::BASE64.encode(&data);

        // Write via echo + base64 decode on remote side
        // Split into chunks to avoid command line length limits
        let chunk_size = 65536;
        let chunks: Vec<&str> = encoded
            .as_bytes()
            .chunks(chunk_size)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            .collect();

        // Create the file empty first
        session
            .command("sh")
            .arg("-c")
            .arg(format!(": > {}.b64", remote_path))
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("File creation failed: {}", e))?;

        for chunk in &chunks {
            let cmd = format!("printf '%s' '{}' >> {}.b64", chunk, remote_path);
            session
                .command("sh")
                .arg("-c")
                .arg(&cmd)
                .status()
                .await
                .map_err(|e| anyhow::anyhow!("Upload chunk failed: {}", e))?;
        }

        // Decode and make executable
        let decode_cmd = format!(
            "base64 -d {0}.b64 > {0} && rm {0}.b64 && chmod +x {0}",
            remote_path
        );
        session
            .command("sh")
            .arg("-c")
            .arg(&decode_cmd)
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("Decode failed: {}", e))?;

        info!("Agent uploaded successfully");
        Ok(())
    }

    /// Start the agent on the remote host.
    pub async fn start_agent(
        &self,
        session: &Session,
        agent_path: &str,
        agent_args: &str,
    ) -> anyhow::Result<()> {
        info!("Starting agent on remote host");

        let cmd = format!(
            "nohup {} {} > \"$(mktemp -d)/architect-agent.log\" 2>&1 &",
            agent_path, agent_args
        );

        let status = session
            .command("sh")
            .arg("-c")
            .arg(&cmd)
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to start agent: {}", e))?;

        if !status.success() {
            error!("Agent start failed");
            anyhow::bail!("Agent start failed");
        }

        info!("Agent started in background");
        Ok(())
    }
}

