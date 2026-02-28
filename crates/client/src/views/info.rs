const MAX_LOG_LINES: usize = 10_000;

/// Info view state — read-only dashboard for a node (info, tasks, logs).
pub struct InfoState {
    pub log_lines: Vec<String>,
    pub active: bool,
}

impl InfoState {
    pub fn new() -> Self {
        Self {
            log_lines: Vec::new(),
            active: true,
        }
    }

    pub fn append_log(&mut self, line: String) {
        self.log_lines.push(line);
        if self.log_lines.len() > MAX_LOG_LINES {
            let drain = self.log_lines.len() - MAX_LOG_LINES;
            self.log_lines.drain(..drain);
        }
    }
}
