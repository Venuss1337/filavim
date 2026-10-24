use crate::{buffer::Buffer, config::Config, input::Action};
use ropey::Rope;
use std::{ops::Range, path::PathBuf};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    Command,
    Picker,
    Explorer,
    Dialog,
    Terminal,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Visual => "VISUAL",
            Self::Command => "COMMAND",
            Self::Picker => "SEARCH",
            Self::Explorer => "EXPLORER",
            Self::Dialog => "DIALOG",
            Self::Terminal => "TERMINAL",
        }
    }
}
pub enum Request {
    Save {
        path: PathBuf,
        text: Rope,
        state: u64,
        quit: bool,
    },
    Open {
        path: PathBuf,
        revision: u64,
    },
    Search {
        text: Rope,
        query: String,
        start: usize,
        revision: u64,
    },
    Clipboard(String),
    Quit,
}
pub struct Editor {
    pub buffer: Buffer,
    pub document_id: u64,
    pub mode: Mode,
    pub anchor: Option<usize>,
    pub dashboard: bool,
    pub help: bool,
    pub help_page: usize,
    pub command: String,
    pub searching: bool,
    pub last_search: String,
    pub message: String,
    pub error: bool,
    pub top: usize,
    pub left: usize,
    pub page_height: usize,
    pub register: String,
    pub(crate) register_linewise: bool,
    pub config: Config,
}
impl Editor {
    pub fn new(config: Config) -> Self {
        Self {
            buffer: Buffer::default(),
            document_id: 0,
            mode: Mode::Normal,
            anchor: None,
            dashboard: true,
            help: false,
            help_page: 0,
            command: String::new(),
            searching: false,
            last_search: String::new(),
            message: "Welcome to Filavim.  Press I to create, :e <path> to open, F1 for help."
                .into(),
            error: false,
            top: 0,
            left: 0,
            page_height: 20,
            register: String::new(),
            register_linewise: false,
            config,
        }
    }
    pub fn notify(&mut self, message: impl Into<String>, error: bool) {
        self.message = message.into();
        self.error = error;
    }
    pub fn set_buffer(&mut self, buffer: Buffer) {
        self.buffer = buffer;
        self.document_id += 1;
        self.dashboard = false;
        self.top = 0;
        self.left = 0;
        self.set_mode(Mode::Normal);
    }
    pub fn set_mode(&mut self, mode: Mode) {
        self.buffer.end_group();
        if mode == Mode::Insert {
            self.buffer.begin_group();
        }
        self.anchor = if mode == Mode::Visual {
            Some(self.buffer.cursor)
        } else {
            None
        };
        self.mode = mode;
    }
    /// Visual selections include the grapheme under the cursor.
    pub fn selection(&self) -> Option<Range<usize>> {
        self.anchor
            .map(|a| a.min(self.buffer.cursor)..self.buffer.next(a.max(self.buffer.cursor)))
    }
    pub fn type_text(&mut self, text: &str) {
        match self.mode {
            Mode::Insert => {
                self.dashboard = false;
                self.buffer.insert(text);
            }
            Mode::Command => self.command.push_str(text),
            _ => {}
        }
    }
    pub fn paste_external(&mut self, text: &str) {
        if self.help {
            return;
        }
        match self.mode {
            Mode::Insert | Mode::Visual => {
                if let Some(range) = self.selection() {
                    let text = self.buffer.normalize_paste(text);
                    self.buffer.replace(range, &text);
                    self.set_mode(Mode::Insert);
                } else {
                    self.buffer.insert_paste(text);
                }
            }
            Mode::Command => self
                .command
                .extend(text.chars().filter(|c| !c.is_control())),
            _ => self.notify("Enter Insert mode (I) to paste terminal text.", false),
        }
    }
    pub fn act(&mut self, action: Action) -> Option<Request> {
        use Action::*;
        if self.help {
            if matches!(action, PageDown | Right | PageUp | Left) {
                self.help_page = (self.help_page + 1) % 2;
            }
            if matches!(action, Help | Normal | Quit) {
                self.help = false;
            }
            return None;
        }
        if self.mode == Mode::Command {
            match action {
                ClearInput => self.command.clear(),
                Normal => {
                    self.set_mode(Mode::Normal);
                    self.command.clear();
                }
                Backspace => {
                    let end = self
                        .command
                        .grapheme_indices(true)
                        .next_back()
                        .map_or(0, |(i, _)| i);
                    self.command.truncate(end);
                }
                Submit => {
                    let command = std::mem::take(&mut self.command);
                    self.set_mode(Mode::Normal);
                    if self.searching {
                        self.last_search = command;
                        return self.search();
                    }
                    return self.execute_command(&command);
                }
                Help => self.help = true,
                _ => {}
            }
            return None;
        }
        match action {
            Up => self.buffer.vertical(-1, self.config.tab_width),
            Down => self.buffer.vertical(1, self.config.tab_width),
            Left => self.buffer.horizontal(false),
            Right => self.buffer.horizontal(true),
            WordBack => self.buffer.word(false),
            WordForward => self.buffer.word(true),
            Home => self.buffer.home(),
            End => self.buffer.end(),
            Top => {
                self.buffer.cursor = 0;
                self.buffer.home();
            }
            Bottom => {
                self.buffer.cursor = self.buffer.text.len_chars();
                self.buffer.home();
            }
            PageUp => self
                .buffer
                .vertical(-(self.page_height.max(1) as isize), self.config.tab_width),
            PageDown => self
                .buffer
                .vertical(self.page_height.max(1) as isize, self.config.tab_width),
            Insert => {
                self.dashboard = false;
                self.set_mode(Mode::Insert);
            }
            Append => {
                self.dashboard = false;
                self.buffer.end();
                self.set_mode(Mode::Insert);
            }
            Visual => {
                self.dashboard = false;
                self.set_mode(if self.mode == Mode::Visual {
                    Mode::Normal
                } else {
                    Mode::Visual
                });
            }
            Normal => {
                self.set_mode(Mode::Normal);
                self.notify("", false);
            }
            Command | Search => {
                self.set_mode(Mode::Command);
                self.command.clear();
                self.searching = action == Search;
            }
            Delete | Backspace => {
                if let Some(range) = self.selection() {
                    self.register = self.buffer.text.slice(range.clone()).to_string();
                    self.register_linewise = false;
                    self.buffer.replace(range, "");
                    self.set_mode(Mode::Normal);
                } else if action == Delete {
                    self.buffer.delete();
                } else {
                    self.buffer.backspace();
                }
            }
            DeleteLine => {
                self.copy_line();
                self.delete_line();
                self.set_mode(Mode::Normal);
            }
            Newline => self.buffer.newline(self.config.auto_indent),
            OpenBelow => {
                self.dashboard = false;
                self.set_mode(Mode::Insert);
                self.buffer.end();
                self.buffer.newline(self.config.auto_indent);
            }
            Tab => {
                if self.config.expand_tabs {
                    let count = self.config.tab_width
                        - self.buffer.column(self.config.tab_width) % self.config.tab_width;
                    self.buffer.insert(&" ".repeat(count));
                } else {
                    self.buffer.insert("\t");
                }
            }
            Undo => {
                let insert = self.mode == Mode::Insert;
                self.buffer.undo();
                self.set_mode(if insert { Mode::Insert } else { Mode::Normal });
            }
            Redo => {
                let insert = self.mode == Mode::Insert;
                self.buffer.redo();
                self.set_mode(if insert { Mode::Insert } else { Mode::Normal });
            }
            Copy | Cut => {
                if let Some(range) = self.selection() {
                    self.register = self.buffer.text.slice(range.clone()).to_string();
                    self.register_linewise = false;
                    if action == Cut {
                        self.buffer.replace(range, "");
                    }
                } else {
                    self.copy_line();
                    if action == Cut {
                        self.delete_line();
                    }
                }
                self.set_mode(Mode::Normal);
                self.notify(
                    "Copied to register and terminal clipboard (when supported).",
                    false,
                );
                return Some(Request::Clipboard(self.register.clone()));
            }
            Paste => {
                if !self.register.is_empty() {
                    self.dashboard = false;
                    let text = self.register.clone();
                    if let Some(range) = self.selection() {
                        self.buffer.replace(range, &text);
                        self.set_mode(Mode::Normal);
                    } else if self.register_linewise {
                        if self.buffer.text.len_chars() == 0 {
                            self.buffer.insert(&text);
                            return None;
                        }
                        self.buffer.end();
                        let row = self.buffer.row();
                        if row + 1 < self.buffer.text.len_lines() {
                            self.buffer.cursor = self.buffer.text.line_to_char(row + 1);
                            let text = if text.ends_with('\n') {
                                text
                            } else {
                                format!("{text}{}", self.buffer.newline)
                            };
                            self.buffer.insert(&text);
                        } else {
                            let text = format!("{}{text}", self.buffer.newline);
                            self.buffer.insert(&text);
                        }
                    } else {
                        self.buffer.insert(&text);
                    }
                }
            }
            Save => return self.save(None, false),
            Quit => return self.execute_command("q"),
            New => return self.execute_command("new"),
            Help => self.help = true,
            SearchNext => return self.search(),
            Submit | Noop => {}
            _ => {}
        }
        None
    }
    fn copy_line(&mut self) {
        let row = self.buffer.row();
        self.register = self.buffer.text.line(row).to_string();
        self.register_linewise = true;
    }
    fn delete_line(&mut self) {
        let row = self.buffer.row();
        let start = self.buffer.text.line_to_char(row);
        let end = if row + 1 < self.buffer.text.len_lines() {
            self.buffer.text.line_to_char(row + 1)
        } else {
            self.buffer.text.len_chars()
        };
        if row + 1 == self.buffer.text.len_lines() && row > 0 {
            self.buffer.replace(self.buffer.previous(start)..end, "");
        } else {
            self.buffer.replace(start..end, "");
        }
    }
    fn search(&mut self) -> Option<Request> {
        if self.last_search.is_empty() {
            self.notify("Enter a search with /text", false);
            return None;
        }
        self.notify("Searching…", false);
        Some(Request::Search {
            text: self.buffer.text.clone(),
            query: self.last_search.clone(),
            start: self.buffer.next(self.buffer.cursor),
            revision: self.buffer.revision,
        })
    }
    fn save(&mut self, path: Option<PathBuf>, quit: bool) -> Option<Request> {
        match path.or_else(|| self.buffer.path.clone()) {
            Some(path) => {
                self.notify("Saving…", false);
                Some(Request::Save {
                    path,
                    text: self.buffer.text.clone(),
                    state: self.buffer.state(),
                    quit,
                })
            }
            None => {
                self.notify("No file name. Use :w <path>", true);
                None
            }
        }
    }
    pub fn execute_command(&mut self, command: &str) -> Option<Request> {
        let command = command.trim().trim_start_matches(':');
        let (name, arg) = command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""));
        let arg = arg.trim();
        let arg = if arg.len() >= 2
            && ((arg.starts_with('"') && arg.ends_with('"'))
                || (arg.starts_with('\'') && arg.ends_with('\'')))
        {
            &arg[1..arg.len().saturating_sub(1)]
        } else {
            arg
        };
        match name {
            "w" | "write" | "wq" | "x" => {
                return self.save(
                    if arg.is_empty() {
                        None
                    } else {
                        Some(arg.into())
                    },
                    name == "wq" || name == "x",
                );
            }
            "q" | "quit" | "q!" => {
                if name != "q!" && self.buffer.dirty() {
                    self.notify("Unsaved changes. Use :w or :q! to discard.", true);
                } else {
                    return Some(Request::Quit);
                }
            }
            "e" | "edit" | "e!" => {
                if name != "e!" && self.buffer.dirty() {
                    self.notify("Unsaved changes. Use :w or :e! <path> to discard.", true);
                } else if let Some(path) = if arg.is_empty() {
                    self.buffer.path.clone()
                } else {
                    Some(arg.into())
                } {
                    self.notify("Opening…", false);
                    return Some(Request::Open {
                        path,
                        revision: self.buffer.revision,
                    });
                } else {
                    self.notify("Use :e <path>", true);
                }
            }
            "new" | "new!" | "enew" => {
                if self.buffer.dirty() && name != "new!" {
                    self.notify("Unsaved changes. Use :new! to discard.", true);
                } else {
                    self.set_buffer(Buffer::default());
                    self.notify("New buffer. Use :w <path> to save.", false);
                }
            }
            "help" => self.help = true,
            "" => {}
            line if line.parse::<usize>().is_ok() => {
                let row = line
                    .parse::<usize>()
                    .unwrap()
                    .saturating_sub(1)
                    .min(self.buffer.text.len_lines() - 1);
                self.buffer.cursor = self.buffer.text.line_to_char(row);
            }
            _ => self.notify(format!("Unknown command: {name}. Use :help",), true),
        }
        None
    }
}
