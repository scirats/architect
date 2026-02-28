use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Padding, Row, Table};

use architect_core::payload::TaskStatus;

use crate::task_manager::TaskView;

pub fn render_task_list(frame: &mut Frame, area: Rect, tasks: &[TaskView]) {
    let header = Row::new(vec![
        Cell::from(Span::styled("ID", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("TYPE", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("STS", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("PROGRESS", Style::default().fg(Color::DarkGray))),
    ])
    .bottom_margin(0);

    let rows: Vec<Row> = tasks
        .iter()
        .take(area.height.saturating_sub(3) as usize)
        .map(|t| {
            let short_id = &t.task_id.to_string()[..8];

            let (status_sym, status_color) = match t.status {
                TaskStatus::Pending => ("◦", Color::DarkGray),
                TaskStatus::Assigned => ("◉", Color::Blue),
                TaskStatus::Running => ("▶", Color::Yellow),
                TaskStatus::Completed => ("✓", Color::Green),
                TaskStatus::Failed => ("✗", Color::Red),
                TaskStatus::Cancelled => ("−", Color::DarkGray),
            };

            let progress = if t.progress > 0.0 {
                render_progress_bar(t.progress, 8)
            } else {
                match t.status {
                    TaskStatus::Completed => "done".to_string(),
                    TaskStatus::Failed => "err".to_string(),
                    _ => "--".to_string(),
                }
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    short_id.to_string(),
                    Style::default().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled(
                    format!("{}", t.task_type),
                    Style::default().fg(Color::White),
                )),
                Cell::from(Span::styled(status_sym, Style::default().fg(status_color))),
                Cell::from(Span::styled(
                    progress,
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();

    let active = tasks
        .iter()
        .filter(|t| matches!(t.status, TaskStatus::Running | TaskStatus::Assigned))
        .count();

    let widths = [
        Constraint::Length(8),
        Constraint::Min(6),
        Constraint::Length(3),
        Constraint::Min(8),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
        .padding(Padding::horizontal(1))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("TASKS", Style::default().fg(Color::Magenta).bold()),
            Span::styled(
                if active > 0 {
                    format!(" {} active ", active)
                } else {
                    format!(" {} ", tasks.len())
                },
                Style::default().fg(Color::DarkGray),
            ),
        ]));

    let table = Table::new(rows, widths).header(header).block(block);

    frame.render_widget(table, area);
}

fn render_progress_bar(pct: f32, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    format!(
        "{}{}  {:.0}%",
        "━".repeat(filled),
        "╌".repeat(empty),
        pct
    )
}
