use std::{env, io};

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use kudzu::{app, config};
use ratatui::{backend::CrosstermBackend, Terminal};

fn main() -> Result<()> {
    let cfg = config::Config::load()?;
    let root = cfg.root.clone().map(Ok).unwrap_or_else(env::current_dir)?;
    let root = root.canonicalize()?;

    let mut guard = TerminalGuard::enter()?;
    let res = app::run(guard.terminal_mut(), root, cfg);
    guard.restore()?;

    res
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    active: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        Ok(Self {
            terminal,
            active: true,
        })
    }

    fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }

    fn restore(&mut self) -> Result<()> {
        if self.active {
            disable_raw_mode()?;
            execute!(
                self.terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture
            )?;
            self.terminal.show_cursor()?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
