use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

#[derive(Debug, Clone)]
pub enum ActivityLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct ActivityEntry {
    pub text: String,
    pub level: ActivityLevel,
}

/// Ring-buffer activity log with fixed capacity.
/// Always shows the most recent entries at the bottom.
pub struct ActivityLog {
    entries: Vec<ActivityEntry>,
    capacity: usize,
}

impl ActivityLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, text: String, level: ActivityLevel) {
        if self.entries.len() >= self.capacity {
            self.entries.remove(0);
        }
        self.entries.push(ActivityEntry { text, level });
    }

    pub fn entries(&self) -> &[ActivityEntry] {
        &self.entries
    }
}

pub fn render_activity_log(frame: &mut Frame, area: Rect, log: &ActivityLog) {
    let entries = log.entries();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
        .padding(Padding::new(1, 1, 0, 0))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("ACTIVITY", Style::default().fg(Color::Yellow).bold()),
            Span::styled(
                format!(" {} ", entries.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

    let inner = block.inner(area);
    let max_width = inner.width as usize;
    let max_height = inner.height as usize;

    if max_width == 0 || max_height == 0 {
        frame.render_widget(block, area);
        return;
    }

    // Split entry text at '\n' into separate Lines for proper rendering.
    // Without this, embedded newlines (e.g. from build errors) are invisible
    // and the wrap calculation is wrong.
    let mut lines: Vec<Line> = Vec::new();
    for entry in entries {
        let color = match entry.level {
            ActivityLevel::Info => Color::DarkGray,
            ActivityLevel::Success => Color::Green,
            ActivityLevel::Warning => Color::Yellow,
            ActivityLevel::Error => Color::Red,
        };
        let style = Style::default().fg(color);
        for part in entry.text.split('\n') {
            lines.push(Line::from(Span::styled(part.to_string(), style)));
        }
    }

    // Estimate total visual lines after wrapping.
    // Ratatui uses word-level wrapping which can produce more lines than
    // character-level math predicts, so add +1 per wrapping line as buffer.
    let total_visual: usize = lines.iter().map(|l| {
        let w = l.width();
        if w <= max_width {
            1
        } else {
            (w + max_width - 1) / max_width + 1
        }
    }).sum();

    let scroll_y = total_visual.saturating_sub(max_height) as u16;

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_y, 0));
    frame.render_widget(paragraph, area);
}
