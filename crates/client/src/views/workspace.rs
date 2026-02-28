use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};

use architect_core::types::{NodeId, NodeStatus};

use crate::widgets::activity_log::{ActivityLevel, ActivityLog};
use crate::widgets::graph::NodePosition;

pub struct WorkspaceState {
    pub node_positions: HashMap<NodeId, NodePosition>,
    pub selected_node: Option<NodeId>,
    pub selection_index: usize,
    pub node_order: Vec<NodeId>,
    pub activity_log: ActivityLog,
}

impl WorkspaceState {
    pub fn new(client_id: NodeId) -> Self {
        Self {
            node_positions: HashMap::new(),
            selected_node: Some(client_id),
            selection_index: 0,
            node_order: vec![client_id],
            activity_log: ActivityLog::new(200),
        }
    }

    pub fn append_activity(&mut self, text: String, level: ActivityLevel) {
        self.activity_log.push(text, level);
    }

    pub fn update_layout(
        &mut self,
        client_id: NodeId,
        client_label: &str,
        agents: &[(NodeId, String, NodeStatus)],
        width: f64,
        height: f64,
    ) {
        self.node_order = vec![client_id];
        let mut agent_ids: Vec<NodeId> = agents.iter().map(|(id, _, _)| *id).collect();
        agent_ids.sort();
        self.node_order.extend(agent_ids);

        self.node_positions =
            crate::widgets::graph::compute_radial_layout(client_id, client_label, agents, width, height);

        if let Some(selected) = self.selected_node {
            if !self.node_order.contains(&selected) {
                self.selection_index = 0;
                self.selected_node = self.node_order.first().copied();
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> WorkspaceAction {
        match key.code {
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k') | KeyCode::Char('h') => {
                self.select_prev();
                WorkspaceAction::None
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j') | KeyCode::Char('l') => {
                self.select_next();
                WorkspaceAction::None
            }
            KeyCode::Enter => {
                if let Some(id) = self.selected_node {
                    WorkspaceAction::OpenInfo(id)
                } else {
                    WorkspaceAction::None
                }
            }
            _ => WorkspaceAction::None,
        }
    }

    fn select_next(&mut self) {
        if self.node_order.is_empty() {
            return;
        }
        self.selection_index = (self.selection_index + 1) % self.node_order.len();
        self.selected_node = Some(self.node_order[self.selection_index]);
    }

    fn select_prev(&mut self) {
        if self.node_order.is_empty() {
            return;
        }
        if self.selection_index == 0 {
            self.selection_index = self.node_order.len() - 1;
        } else {
            self.selection_index -= 1;
        }
        self.selected_node = Some(self.node_order[self.selection_index]);
    }
}

pub enum WorkspaceAction {
    None,
    OpenInfo(NodeId),
}
