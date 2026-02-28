use std::collections::HashMap;

use ratatui::prelude::*;
use ratatui::widgets::canvas::{Canvas, Circle, Line};
use ratatui::widgets::{Block, Borders};

use architect_core::types::{NodeId, NodeRole, NodeStatus};

pub struct NodePosition {
    pub x: f64,
    pub y: f64,
    pub role: NodeRole,
    pub label: String,
    pub short_id: String,
    pub status: NodeStatus,
}

pub fn compute_radial_layout(
    client_id: NodeId,
    client_label: &str,
    agents: &[(NodeId, String, NodeStatus)],
    width: f64,
    height: f64,
) -> HashMap<NodeId, NodePosition> {
    let mut positions = HashMap::new();
    let cx = width / 2.0;
    let cy = height / 2.0;

    positions.insert(
        client_id,
        NodePosition {
            x: cx,
            y: cy,
            role: NodeRole::Client,
            label: client_label.to_string(),
            short_id: client_id.to_string()[..8].to_string(),
            status: NodeStatus::Online,
        },
    );

    let n = agents.len();
    if n == 0 {
        return positions;
    }

    let radius = (width.min(height) / 2.0) * 0.65;
    for (i, (id, label, status)) in agents.iter().enumerate() {
        let angle = (i as f64 / n as f64) * 2.0 * std::f64::consts::PI - std::f64::consts::FRAC_PI_2;
        let x = cx + radius * angle.cos();
        let y = cy + radius * angle.sin();
        positions.insert(
            *id,
            NodePosition {
                x,
                y,
                role: NodeRole::Agent,
                label: label.clone(),
                short_id: id.to_string()[..8].to_string(),
                status: *status,
            },
        );
    }

    positions
}

pub fn render_graph(
    frame: &mut Frame,
    area: Rect,
    positions: &HashMap<NodeId, NodePosition>,
    client_id: NodeId,
    selected: Option<NodeId>,
    _node_order: &[NodeId],
) {
    let w = area.width as f64;
    let h = (area.height as f64) * 2.0; // braille has 2:1 aspect ratio

    let canvas = Canvas::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
                .title(ratatui::text::Line::from(vec![
                    Span::styled(" ", Style::default()),
                    Span::styled("TOPOLOGY", Style::default().fg(Color::Cyan).bold()),
                    Span::styled(" ", Style::default()),
                ])),
        )
        .marker(symbols::Marker::Braille)
        .x_bounds([0.0, w])
        .y_bounds([0.0, h])
        .paint(|ctx| {
            let client_pos = positions.get(&client_id);

            // Draw connection lines
            if let Some(cp) = client_pos {
                for (id, pos) in positions.iter() {
                    if *id == client_id {
                        continue;
                    }
                    let color = match pos.status {
                        NodeStatus::Online => Color::Green,
                        NodeStatus::Degraded => Color::Yellow,
                        NodeStatus::Offline => Color::DarkGray,
                    };
                    ctx.draw(&Line {
                        x1: cp.x,
                        y1: cp.y,
                        x2: pos.x,
                        y2: pos.y,
                        color,
                    });
                }
            }

            // Crimson color for selected nodes
            let crimson = Color::Rgb(220, 20, 60);

            // Draw nodes
            for (id, pos) in positions.iter() {
                let is_selected = selected == Some(*id);
                let base_color = match pos.status {
                    NodeStatus::Online => Color::Green,
                    NodeStatus::Degraded => Color::Yellow,
                    NodeStatus::Offline => Color::DarkGray,
                };
                let color = if is_selected { crimson } else { base_color };

                match pos.role {
                    NodeRole::Client => {
                        // Diamond shape: 4 lines
                        let s = 3.0;
                        ctx.draw(&Line { x1: pos.x, y1: pos.y + s, x2: pos.x + s, y2: pos.y, color });
                        ctx.draw(&Line { x1: pos.x + s, y1: pos.y, x2: pos.x, y2: pos.y - s, color });
                        ctx.draw(&Line { x1: pos.x, y1: pos.y - s, x2: pos.x - s, y2: pos.y, color });
                        ctx.draw(&Line { x1: pos.x - s, y1: pos.y, x2: pos.x, y2: pos.y + s, color });
                    }
                    NodeRole::Agent => {
                        ctx.draw(&Circle {
                            x: pos.x,
                            y: pos.y,
                            radius: 2.0,
                            color,
                        });
                    }
                }

                // Label (hostname)
                ctx.print(pos.x - (pos.label.len() as f64 / 2.0), pos.y - 4.0, pos.label.clone().fg(color));
                // Short ID below hostname (agents only)
                if pos.role == NodeRole::Agent {
                    ctx.print(pos.x - (pos.short_id.len() as f64 / 2.0), pos.y - 6.0, pos.short_id.clone().fg(color));
                }
            }
        });

    frame.render_widget(canvas, area);
}
