use std::net::IpAddr;

use tracing::info;

/// WinRM-based agent deployment for Windows machines.
/// Uses PowerShell Remoting via HTTP.
pub struct WinRmBootstrapper {
    port: u16,
}

impl WinRmBootstrapper {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// Execute a PowerShell command on the remote Windows host.
    pub async fn invoke_command(
        &self,
        host: IpAddr,
        username: &str,
        password: &str,
        command: &str,
    ) -> anyhow::Result<String> {
        let url = format!("http://{}:{}/wsman", host, self.port);

        // Sanitize inputs to prevent PowerShell injection
        let safe_username = username.replace('\'', "''");
        let safe_password = password.replace('\'', "''");
        let safe_host = host.to_string();
        let safe_command = command.replace('\'', "''");

        let script = format!(
            "$cred = New-Object PSCredential('{}', (ConvertTo-SecureString '{}' -AsPlainText -Force)); \
             Invoke-Command -ComputerName {} -Credential $cred -ScriptBlock {{ {} }}",
            safe_username, safe_password, safe_host, safe_command
        );

        let output = tokio::process::Command::new("pwsh")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(&script)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("WinRM command failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(stdout)
    }

    /// Deploy agent to a Windows host.
    pub async fn deploy_agent(
        &self,
        host: IpAddr,
        username: &str,
        password: &str,
        agent_url: &str,
        agent_args: &str,
    ) -> anyhow::Result<()> {
        info!("Deploying agent to Windows host {} via WinRM", host);

        // Ensure directory exists and download agent binary
        let download_cmd = format!(
            "New-Item -ItemType Directory -Force -Path 'C:\\architect' | Out-Null; \
             Invoke-WebRequest -Uri '{}' -OutFile 'C:\\architect\\agent.exe'",
            agent_url
        );

        self.invoke_command(host, username, password, &download_cmd)
            .await?;

        // Start agent as background process
        let start_cmd = format!(
            "Start-Process -FilePath 'C:\\architect\\agent.exe' \
             -ArgumentList '{}' \
             -WindowStyle Hidden -RedirectStandardOutput 'C:\\architect\\agent.log'",
            agent_args
        );

        self.invoke_command(host, username, password, &start_cmd)
            .await?;

        info!("Agent deployed on Windows host {}", host);
        Ok(())
    }
}
