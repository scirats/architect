use architect_core::types::NodeId;
use uuid::Uuid;

// --- Argument structs for ML commands ---

#[derive(Debug, Clone)]
pub struct TrainArgs {
    pub dataset: String,
    pub epochs: Option<u32>,
    pub lr: Option<f32>,
    pub batch_size: Option<u32>,
    pub output: Option<String>,
    pub max_ram_mb: Option<u64>,
    pub max_cpu_pct: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct FinetuneArgs {
    pub base_model: String,
    pub dataset: String,
    pub adapter: Option<String>,
    pub epochs: Option<u32>,
    pub lr: Option<f32>,
    pub batch_size: Option<u32>,
    pub output: Option<String>,
    pub max_ram_mb: Option<u64>,
    pub max_cpu_pct: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct InferArgs {
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct PreprocessArgs {
    pub input: String,
    pub output: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EvalArgs {
    pub model: String,
    pub dataset: String,
    pub metrics: Vec<String>,
    pub output: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RagArgs {
    pub query: String,
    pub index: String,
    pub model: Option<String>,
    pub top_k: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct EmbedArgs {
    pub input: String,
    pub model: Option<String>,
    pub index: Option<String>,
    pub operation: Option<String>,
}

// --- Command enum ---

#[derive(Debug, Clone)]
pub enum Command {
    Info { node_id: Option<NodeId>, prefix: Option<String> },
    Quit,
    // ML/AI commands
    Train(TrainArgs),
    Finetune(FinetuneArgs),
    Infer(InferArgs),
    Preprocess(PreprocessArgs),
    Evaluate(EvalArgs),
    Rag(RagArgs),
    Embed(EmbedArgs),
    // Management commands
    Cancel { task_id: String },
    // Discovery/Bootstrap commands
    Pulse { subnet: Option<String> },
    Seed { target: String, user: Option<String>, pass: Option<String> },
    // Hardening commands
    Bless,
    Ascend { url: Option<String> },
    Purge { target: String },
    // Plugin system
    Plugin { name: String, config: String },
    Unknown(String),
}

/// Context determines which commands are available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandContext {
    Workspace,
}

// --- Available commands per context ---

/// Separator sentinel: entries with name "" are rendered as visual dividers.
pub const WORKSPACE_COMMANDS: &[(&str, &str, &str)] = &[
    // ── Cluster ──────────────────────────────────────────
    ("info",       "inspect node details and logs",         "[node_id]"),
    ("bless",      "rotate cluster auth token",             ""),
    ("pulse",      "scan local network for devices",        "[--subnet 192.168.1.0/24]"),
    ("seed",       "install agent on a remote device",      "<ip> [--user root] [--pass pw]"),
    ("ascend",     "push latest agent to all nodes",        "[url]"),
    ("purge",      "erase agent and all traces from device","<node_id | all>"),
    ("", "", ""),
    // ── ML / AI ──────────────────────────────────────────
    ("plugin",     "run a custom plugin task",              "<name> <json>"),
    ("train",      "start distributed model training",      "--dataset <path> [--epochs N] [--lr F] [--batch-size N] [--max-ram MB] [--max-cpu %]"),
    ("finetune",   "fine-tune a pre-trained model",         "--base <model> --dataset <path> [--adapter lora] [--epochs N] [--max-ram MB]"),
    ("infer",      "run model inference on a prompt",       "\"<prompt>\" [--model <name>] [--max-tokens N] [--temp F]"),
    ("preprocess", "transform and tokenize datasets",       "--input <path> [--output <path>] [--format tokenize]"),
    ("evaluate",   "benchmark model with metrics",          "--model <path> --dataset <path> [--metrics perplexity,bleu] [--output <path>]"),
    ("rag",        "retrieval-augmented generation query",   "\"<query>\" --index <path> [--model <name>] [--top-k N]"),
    ("embed",      "generate or search vector embeddings",  "\"<text>\" [--model <name>] [--index <path>] [--op encode|search]"),
    ("", "", ""),
    // ── Session ──────────────────────────────────────────
    ("cancel",     "abort a running task by id",            "<task_id>"),
    ("quit",       "shutdown and exit",                     ""),
];

// --- Tokenizer: splits respecting quotes ---

fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut quote_char = '"';

    for ch in input.chars() {
        if in_quote {
            if ch == quote_char {
                in_quote = false;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = true;
            quote_char = ch;
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

// --- Flag parser helpers ---

fn get_flag(tokens: &[String], flag: &str) -> Option<String> {
    tokens.iter().position(|t| t == flag).and_then(|i| tokens.get(i + 1).cloned())
}

fn get_flag_u32(tokens: &[String], flag: &str) -> Option<u32> {
    get_flag(tokens, flag).and_then(|v| v.parse().ok())
}

fn get_flag_f32(tokens: &[String], flag: &str) -> Option<f32> {
    get_flag(tokens, flag).and_then(|v| v.parse().ok())
}

/// Get the first positional argument (not a flag or flag value).
fn get_positional(tokens: &[String]) -> Option<String> {
    let mut skip_next = false;
    for token in tokens {
        if skip_next {
            skip_next = false;
            continue;
        }
        if token.starts_with("--") {
            skip_next = true;
            continue;
        }
        return Some(token.clone());
    }
    None
}

// --- Main parser ---

pub fn parse_command(input: &str) -> Command {
    let input = input.trim();
    let tokens = tokenize(input);
    if tokens.is_empty() {
        return Command::Unknown(String::new());
    }

    let cmd = tokens[0].to_lowercase();
    let args = &tokens[1..];

    match cmd.as_str() {
        "info" => {
            let node_id = args.first().and_then(|s| Uuid::parse_str(s).ok());
            let prefix = if node_id.is_none() {
                args.first().cloned()
            } else {
                None
            };
            Command::Info { node_id, prefix }
        }
        "quit" => Command::Quit,
        "cancel" => {
            let task_id = args.first().cloned().unwrap_or_default();
            Command::Cancel { task_id }
        }

        "train" => {
            let dataset = get_flag(args, "--dataset").unwrap_or_default();
            if dataset.is_empty() {
                return Command::Unknown("train: --dataset required".into());
            }
            Command::Train(TrainArgs {
                dataset,
                epochs: get_flag_u32(args, "--epochs"),
                lr: get_flag_f32(args, "--lr"),
                batch_size: get_flag_u32(args, "--batch-size"),
                output: get_flag(args, "--output"),
                max_ram_mb: get_flag(args, "--max-ram").and_then(|v| v.parse().ok()),
                max_cpu_pct: get_flag_f32(args, "--max-cpu"),
            })
        }

        "finetune" => {
            let base_model = get_flag(args, "--base").unwrap_or_default();
            let dataset = get_flag(args, "--dataset").unwrap_or_default();
            if base_model.is_empty() || dataset.is_empty() {
                return Command::Unknown("finetune: --base and --dataset required".into());
            }
            Command::Finetune(FinetuneArgs {
                base_model,
                dataset,
                adapter: get_flag(args, "--adapter"),
                epochs: get_flag_u32(args, "--epochs"),
                lr: get_flag_f32(args, "--lr"),
                batch_size: get_flag_u32(args, "--batch-size"),
                output: get_flag(args, "--output"),
                max_ram_mb: get_flag(args, "--max-ram").and_then(|v| v.parse().ok()),
                max_cpu_pct: get_flag_f32(args, "--max-cpu"),
            })
        }

        "infer" => {
            let prompt = get_positional(args).unwrap_or_default();
            if prompt.is_empty() {
                return Command::Unknown("infer: prompt required".into());
            }
            Command::Infer(InferArgs {
                prompt,
                max_tokens: get_flag_u32(args, "--max-tokens"),
                temperature: get_flag_f32(args, "--temp"),
            })
        }

        "preprocess" => {
            let input_path = get_flag(args, "--input").unwrap_or_default();
            if input_path.is_empty() {
                return Command::Unknown("preprocess: --input required".into());
            }
            Command::Preprocess(PreprocessArgs {
                input: input_path,
                output: get_flag(args, "--output"),
                format: get_flag(args, "--format"),
            })
        }

        "evaluate" => {
            let model = get_flag(args, "--model").unwrap_or_default();
            let dataset = get_flag(args, "--dataset").unwrap_or_default();
            if model.is_empty() || dataset.is_empty() {
                return Command::Unknown("evaluate: --model and --dataset required".into());
            }
            let metrics = get_flag(args, "--metrics")
                .map(|m| m.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_else(|| vec!["perplexity".into()]);
            Command::Evaluate(EvalArgs {
                model,
                dataset,
                metrics,
                output: get_flag(args, "--output"),
            })
        }

        "rag" => {
            let query = get_positional(args).unwrap_or_default();
            let index = get_flag(args, "--index").unwrap_or_default();
            if query.is_empty() || index.is_empty() {
                return Command::Unknown("rag: query and --index required".into());
            }
            Command::Rag(RagArgs {
                query,
                index,
                model: get_flag(args, "--model"),
                top_k: get_flag_u32(args, "--top-k"),
            })
        }

        "embed" => {
            let input_text = get_positional(args)
                .or_else(|| get_flag(args, "--input"))
                .unwrap_or_default();
            if input_text.is_empty() {
                return Command::Unknown("embed: input text or --input required".into());
            }
            Command::Embed(EmbedArgs {
                input: input_text,
                model: get_flag(args, "--model"),
                index: get_flag(args, "--index"),
                operation: get_flag(args, "--op"),
            })
        }

        "pulse" => {
            let subnet = get_flag(args, "--subnet").or_else(|| get_positional(args));
            Command::Pulse { subnet }
        }

        "seed" => {
            let target = get_positional(args).unwrap_or_default();
            if target.is_empty() {
                return Command::Unknown("seed: <ip> required".into());
            }
            Command::Seed {
                target,
                user: get_flag(args, "--user"),
                pass: get_flag(args, "--pass"),
            }
        }

        "bless" => Command::Bless,

        "plugin" => {
            let name = get_positional(args).unwrap_or_default();
            if name.is_empty() {
                return Command::Unknown("plugin: <name> required".into());
            }
            // Everything after the plugin name is the JSON config
            let config = args.iter()
                .skip_while(|t| t.as_str() != name.as_str())
                .skip(1)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            let config = if config.is_empty() { "{}".to_string() } else { config };
            Command::Plugin { name, config }
        }

        "ascend" => {
            let url = get_positional(args).unwrap_or_default();
            let url = if url.is_empty() { None } else { Some(url) };
            Command::Ascend { url }
        }

        "purge" => {
            let target = get_positional(args).unwrap_or_default();
            if target.is_empty() {
                return Command::Unknown("purge: <node_id | all> required".into());
            }
            Command::Purge { target }
        }

        _ => Command::Unknown(input.to_string()),
    }
}

// --- Command bar state ---

pub struct CommandBarState {
    pub input: String,
    pub cursor_pos: usize,
    pub active: bool,
    /// Autocomplete popup
    pub popup_open: bool,
    pub popup_items: Vec<(&'static str, &'static str, &'static str)>,
    pub popup_selected: usize,
    pub popup_scroll: usize,
    /// When set, popup_confirm produces "{prefix} {name}" instead of just "{name}".
    pub popup_prefix: Option<String>,
    pub context: CommandContext,
}

impl CommandBarState {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor_pos: 0,
            active: false,
            popup_open: false,
            popup_items: Vec::new(),
            popup_selected: 0,
            popup_scroll: 0,
            popup_prefix: None,
            context: CommandContext::Workspace,
        }
    }

    pub fn set_context(&mut self, context: CommandContext) {
        self.context = context;
    }

    pub fn activate(&mut self) {
        self.active = true;
        self.input.clear();
        self.cursor_pos = 0;
        self.close_popup();
    }

    pub fn deactivate(&mut self) {
        self.active = false;
        self.input.clear();
        self.cursor_pos = 0;
        self.close_popup();
    }

    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
        if self.popup_open {
            self.update_filtered_popup();
        }
    }

    pub fn delete_char(&mut self) {
        if self.cursor_pos > 0 {
            let prev = self.input[..self.cursor_pos]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor_pos -= prev;
            self.input.remove(self.cursor_pos);
        }
        if self.popup_open {
            self.update_filtered_popup();
        }
    }

    pub fn open_popup(&mut self) {
        self.popup_items = WORKSPACE_COMMANDS.to_vec();
        self.popup_selected = 0;
        self.popup_scroll = 0;
        self.popup_open = true;
        // Skip separator if first item is one
        self.skip_separator_forward();
    }

    /// Returns true if the item at `idx` is a separator (empty name).
    fn is_separator(&self, idx: usize) -> bool {
        self.popup_items.get(idx).is_some_and(|&(name, _, _)| name.is_empty())
    }

    /// Advance selection past separators going forward.
    fn skip_separator_forward(&mut self) {
        while self.is_separator(self.popup_selected) && self.popup_selected + 1 < self.popup_items.len() {
            self.popup_selected += 1;
        }
    }

    /// Advance selection past separators going backward.
    fn skip_separator_backward(&mut self) {
        while self.is_separator(self.popup_selected) && self.popup_selected > 0 {
            self.popup_selected -= 1;
        }
    }

    pub fn close_popup(&mut self) {
        self.popup_open = false;
        self.popup_items.clear();
        self.popup_selected = 0;
        self.popup_scroll = 0;
        self.popup_prefix = None;
    }

    /// Tab autocomplete (zsh-style).
    /// - Empty input → open full popup
    /// - Single match → fill command + trailing space
    /// - Multiple matches → complete common prefix + open filtered popup
    /// - Popup already open → confirm selected item + trailing space
    pub fn tab_complete(&mut self) {
        if self.popup_open {
            self.popup_confirm();
            if !self.input.is_empty() && !self.input.ends_with(' ') {
                self.input.push(' ');
                self.cursor_pos = self.input.len();
            }
            return;
        }

        let query = self.input.trim().to_lowercase();
        if query.is_empty() {
            self.open_popup();
            return;
        }
        // Only autocomplete the command name (first token)
        if query.contains(' ') {
            return;
        }

        let matches: Vec<_> = WORKSPACE_COMMANDS
            .iter()
            .filter(|&&(name, _, _)| !name.is_empty() && name.starts_with(&query))
            .copied()
            .collect();

        match matches.len() {
            0 => {}
            1 => {
                self.input = format!("{} ", matches[0].0);
                self.cursor_pos = self.input.len();
            }
            _ => {
                let common = Self::common_prefix(&matches);
                if common.len() > query.len() {
                    self.input = common;
                    self.cursor_pos = self.input.len();
                }
                self.open_filtered_popup();
            }
        }
    }

    /// Open popup filtered to commands matching the current input prefix.
    fn open_filtered_popup(&mut self) {
        let query = self.input.trim().to_lowercase();
        let filtered: Vec<_> = WORKSPACE_COMMANDS
            .iter()
            .filter(|&&(name, _, _)| !name.is_empty() && name.starts_with(&query))
            .copied()
            .collect();

        if filtered.is_empty() {
            self.close_popup();
            return;
        }

        self.popup_items = filtered;
        self.popup_selected = 0;
        self.popup_scroll = 0;
        self.popup_open = true;
    }

    /// Live-update the filtered popup as the user types.
    fn update_filtered_popup(&mut self) {
        let query = self.input.trim().to_lowercase();

        // Past the command name → close popup
        if query.contains(' ') {
            self.close_popup();
            return;
        }

        if query.is_empty() {
            self.popup_items = WORKSPACE_COMMANDS.to_vec();
            self.popup_selected = 0;
            self.popup_scroll = 0;
            self.skip_separator_forward();
            return;
        }

        let filtered: Vec<_> = WORKSPACE_COMMANDS
            .iter()
            .filter(|&&(name, _, _)| !name.is_empty() && name.starts_with(&query))
            .copied()
            .collect();

        if filtered.is_empty() {
            self.close_popup();
            return;
        }

        self.popup_items = filtered;
        if self.popup_selected >= self.popup_items.len() {
            self.popup_selected = 0;
        }
        self.popup_scroll = 0;
    }

    /// Longest common prefix among a set of command names.
    fn common_prefix(items: &[(&str, &str, &str)]) -> String {
        if items.is_empty() {
            return String::new();
        }
        let first = items[0].0;
        let mut len = first.len();
        for &(name, _, _) in &items[1..] {
            len = first
                .bytes()
                .zip(name.bytes())
                .take(len)
                .take_while(|(a, b)| a == b)
                .count();
        }
        first[..len].to_string()
    }

    /// Max visible items in the popup (without scrolling).
    const POPUP_MAX_VISIBLE: usize = 20;

    fn adjust_scroll(&mut self) {
        let max_vis = Self::POPUP_MAX_VISIBLE;
        if self.popup_selected < self.popup_scroll {
            self.popup_scroll = self.popup_selected;
        } else if self.popup_selected >= self.popup_scroll + max_vis {
            self.popup_scroll = self.popup_selected + 1 - max_vis;
        }
    }

    pub fn popup_up(&mut self) {
        if !self.popup_open {
            let query = self.input.trim();
            if query.is_empty() || query.contains(' ') {
                self.open_popup();
            } else {
                self.open_filtered_popup();
            }
            return;
        }
        if self.popup_selected > 0 {
            self.popup_selected -= 1;
        } else {
            self.popup_selected = self.popup_items.len().saturating_sub(1);
        }
        self.skip_separator_backward();
        self.adjust_scroll();
    }

    pub fn popup_down(&mut self) {
        if !self.popup_open {
            let query = self.input.trim();
            if query.is_empty() || query.contains(' ') {
                self.open_popup();
            } else {
                self.open_filtered_popup();
            }
            return;
        }
        if self.popup_selected + 1 < self.popup_items.len() {
            self.popup_selected += 1;
        } else {
            self.popup_selected = 0;
        }
        self.skip_separator_forward();
        self.adjust_scroll();
    }

    pub fn popup_confirm(&mut self) {
        if self.popup_open {
            if let Some(&(name, _, _)) = self.popup_items.get(self.popup_selected) {
                if let Some(ref prefix) = self.popup_prefix {
                    self.input = format!("{} {}", prefix, name);
                } else {
                    self.input = name.to_string();
                }
                self.cursor_pos = self.input.len();
            }
            // Clear popup state but keep prefix None so close_popup doesn't interfere
            self.popup_open = false;
            self.popup_items.clear();
            self.popup_selected = 0;
            self.popup_scroll = 0;
            self.popup_prefix = None;
        }
    }

    pub fn submit(&mut self) -> Option<Command> {
        if self.popup_open {
            self.popup_confirm();
            return None;
        }
        if self.input.is_empty() {
            self.deactivate();
            return None;
        }
        let cmd = parse_command(&self.input);
        self.deactivate();
        Some(cmd)
    }

}
