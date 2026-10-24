use anyhow::{Context, Result};
use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use filavim::{
    cli::{self, Cli, Launch},
    config::Config,
    input::Keymap,
    runtime,
    storage::History,
    theme::Theme,
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;

fn restore() {
    let _ = filavim::pixiedust::graphics::cleanup_owned(&mut io::stdout());
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        SetCursorStyle::DefaultUserShape,
        Show,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
}
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}
fn main() -> Result<()> {
    let paths = match Cli::parse(std::env::args_os().skip(1))? {
        Cli::Help => {
            println!(
                "{}\n\nConfiguration: {}\nHistory: {}",
                cli::HELP,
                Config::path().display(),
                History::path().display()
            );
            return Ok(());
        }
        Cli::Version => {
            println!("{}", cli::version());
            return Ok(());
        }
        Cli::Open(paths) => paths,
    };
    let launch = Launch::resolve(
        &std::env::current_dir().context("Cannot determine launch directory")?,
        &paths,
    )?;
    let config = Config::load()?;
    let theme = Theme::from_config(&config)?.for_terminal(config.visual.colors);
    let keymap = Keymap::new(&config)?;
    let app = launch.into_app(config, History::load(), History::path())?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    enable_raw_mode().context("Filavim requires an interactive terminal")?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let output = io::BufWriter::with_capacity(64 * 1024, io::stdout());
    let mut terminal = Terminal::new(CrosstermBackend::new(output))?;
    runtime::run(&mut terminal, app, keymap, theme)
}
