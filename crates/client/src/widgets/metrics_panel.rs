use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Padding, Row, Table};

use architect_core::types::NodeStatus;

use crate::app::AgentState;

pub fn render_metrics_panel(
    frame: &mut Frame,
    area: Rect,
    agents: &std::collections::HashMap<architect_core::types::NodeId, AgentState>,
) {
    let header = Row::new(vec![
        Cell::from(Span::styled("ID", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("HOST", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("STS", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("CPU", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("RAM", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("UP", Style::default().fg(Color::DarkGray))),
    ])
    .bottom_margin(0);

    let mut rows: Vec<Row> = agents
        .iter()
        .map(|(id, a)| {
            let short_id = &id.to_string()[..8];

            let status_color = match a.status {
                NodeStatus::Online => Color::Green,
                NodeStatus::Degraded => Color::Yellow,
                NodeStatus::Offline => Color::DarkGray,
            };

            let status_sym = match a.status {
                NodeStatus::Online => "●",
                NodeStatus::Degraded => "◐",
                NodeStatus::Offline => "○",
            };

            let (cpu, ram, uptime) = if let Some(m) = &a.metrics {
                let cpu_color = if m.cpu_usage_pct > 80.0 {
                    Color::Red
                } else if m.cpu_usage_pct > 50.0 {
                    Color::Yellow
                } else {
                    Color::Green
                };
                let ram_color = if m.ram_usage_pct > 80.0 {
                    Color::Red
                } else if m.ram_usage_pct > 50.0 {
                    Color::Yellow
                } else {
                    Color::Green
                };
                let up = format_uptime(m.uptime_secs);
                (
                    Span::styled(format!("{:.0}%", m.cpu_usage_pct), Style::default().fg(cpu_color)),
                    Span::styled(format!("{:.0}%", m.ram_usage_pct), Style::default().fg(ram_color)),
                    Span::styled(up, Style::default().fg(Color::DarkGray)),
                )
            } else {
                (
                    Span::styled("--", Style::default().fg(Color::DarkGray)),
                    Span::styled("--", Style::default().fg(Color::DarkGray)),
                    Span::styled("--", Style::default().fg(Color::DarkGray)),
                )
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    short_id.to_string(),
                    Style::default().fg(Color::Cyan),
                )),
                Cell::from(Span::styled(
                    a.info.hostname.clone(),
                    Style::default().fg(Color::White),
                )),
                Cell::from(Span::styled(status_sym, Style::default().fg(status_color))),
                Cell::from(cpu),
                Cell::from(ram),
                Cell::from(uptime),
            ])
        })
        .collect();

    rows.sort_by(|_, _| std::cmp::Ordering::Equal);

    let widths = [
        Constraint::Length(8),
        Constraint::Min(6),
        Constraint::Length(3),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Length(5),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
        .padding(Padding::horizontal(1))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("NODES", Style::default().fg(Color::Cyan).bold()),
            Span::styled(
                format!(" {} ", agents.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

    let table = Table::new(rows, widths).header(header).block(block);

    frame.render_widget(table, area);
}

fn format_uptime(secs: u64) -> String {
    if secs >= 86400 {
        format!("{}d{}h", secs / 86400, (secs % 86400) / 3600)
    } else if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}m{}s", secs / 60, secs % 60)
    }
}
