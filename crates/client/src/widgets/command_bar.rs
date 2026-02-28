use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::command::CommandBarState;

pub fn render_command_bar(frame: &mut Frame, area: Rect, state: &CommandBarState) {
    if state.active {
        let input_line = Line::from(vec![
            Span::styled(":", Style::default().fg(Color::Magenta).bold()),
            Span::styled(&state.input, Style::default().fg(Color::White)),
        ]);
        let bar = Paragraph::new(input_line)
            .style(Style::default().bg(Color::Rgb(25, 25, 25)));
        frame.render_widget(bar, area);

        // Cursor
        frame.set_cursor_position((
            area.x + 1 + state.cursor_pos as u16,
            area.y,
        ));

        // Popup (scrollable)
        if state.popup_open && !state.popup_items.is_empty() {
            let max_visible = state.popup_items.len().min(20);
            let scroll = state.popup_scroll;
            let total = state.popup_items.len();
            let has_more_above = scroll > 0;
            let has_more_below = scroll + max_visible < total;
            let mut lines: Vec<Line> = Vec::new();

            if has_more_above {
                lines.push(Line::from(Span::styled(
                    format!(" ... {} more above", scroll),
                    Style::default().fg(Color::DarkGray).italic(),
                )));
            }

            for (i, &(name, desc, usage)) in state.popup_items.iter().enumerate().skip(scroll).take(max_visible) {
                // Separator: empty name renders as a blank line
                if name.is_empty() {
                    lines.push(Line::from(" "));
                    continue;
                }

                let selected = i == state.popup_selected;
                // Line 1: :<command> <args>
                if selected {
                    let mut spans = vec![
                        Span::styled(format!(" :{}", name), Style::default().fg(Color::Cyan).bold()),
                    ];
                    if !usage.is_empty() {
                        spans.push(Span::styled(format!(" {}", usage), Style::default().fg(Color::DarkGray)));
                    }
                    spans.push(Span::styled(" ", Style::default()));
                    lines.push(Line::from(spans));
                    // Line 2: description (only for selected)
                    lines.push(Line::from(Span::styled(
                        format!("   {}", desc),
                        Style::default().fg(Color::Rgb(120, 150, 160)),
                    )));
                } else {
                    let mut spans = vec![
                        Span::styled(format!(" :{}", name), Style::default().fg(Color::White)),
                    ];
                    if !usage.is_empty() {
                        spans.push(Span::styled(format!(" {}", usage), Style::default().fg(Color::DarkGray)));
                    }
                    spans.push(Span::styled(" ", Style::default()));
                    lines.push(Line::from(spans));
                }
            }

            if has_more_below {
                lines.push(Line::from(Span::styled(
                    format!(" ... {} more below", total - scroll - max_visible),
                    Style::default().fg(Color::DarkGray).italic(),
                )));
            }

            // Width = longest line + 1 + borders (2). Skip separators.
            let max_content_width = state.popup_items.iter().filter(|&&(n, _, _)| !n.is_empty()).map(|&(name, _desc, usage)| {
                if usage.is_empty() {
                    2 + name.len() + 1 // " :" + name + " "
                } else {
                    2 + name.len() + 1 + usage.len() + 1 // " :" + name + " " + usage + " "
                }
            }).max().unwrap_or(30);
            // Also account for description lines (selected item)
            let max_desc_width = state.popup_items.iter().filter(|&&(n, _, _)| !n.is_empty()).map(|&(_, desc, _)| {
                3 + desc.len() // "   " + desc
            }).max().unwrap_or(0);
            let content_width = max_content_width.max(max_desc_width);
            let popup_width = ((content_width + 3) as u16).min(area.width); // +3 for borders + 1
            let popup_height = lines.len() as u16 + 2; // +2 for borders
            let popup_area = Rect {
                x: area.x,
                y: area.y.saturating_sub(popup_height),
                width: popup_width,
                height: popup_height,
            };

            frame.render_widget(Clear, popup_area);

            let popup = Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
                    .style(Style::default().bg(Color::Rgb(30, 30, 30))),
            );
            frame.render_widget(popup, popup_area);
        }
    } else {
        let bar = Paragraph::new(Line::from(vec![
            Span::styled("  ", Style::default().fg(Color::DarkGray)),
            Span::styled("press ", Style::default().fg(Color::DarkGray)),
            Span::styled(":", Style::default().fg(Color::Magenta).bold()),
            Span::styled(" for commands", Style::default().fg(Color::DarkGray)),
        ]))
        .style(Style::default().bg(Color::Rgb(20, 20, 20)));
        frame.render_widget(bar, area);
    }
}
