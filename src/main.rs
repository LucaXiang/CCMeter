mod app;
mod config;
mod data;
mod ui;
mod update_check;

use std::io;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::{
    ExecutableCommand,
    event::{self, Event as CrosstermEvent, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;

use crate::ui::loading::draw_loading;

#[derive(Parser)]
#[command(name = "ccmeter", about = "Claude Code usage statistics")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Backfill historical usage from persistent sources (stats-cache.json,
    /// Code Insights) back to 2026-01-01, merging into the local history cache.
    Backfill {
        /// Print what would be written without modifying the cache.
        #[arg(long)]
        dry_run: bool,
        /// Which source(s) to read.
        #[arg(long, value_enum, default_value_t = SourceArg::All)]
        source: SourceArg,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum SourceArg {
    All,
    StatsCache,
    CodeInsights,
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(LeaveAlternateScreen);
    }
}

enum State {
    Loading {
        rx: mpsc::Receiver<Box<app::App>>,
        start: Instant,
    },
    Ready(Box<app::App>),
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    if let Some(Command::Backfill { dry_run, source }) = cli.command {
        return run_backfill_cli(dry_run, source);
    }

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(LeaveAlternateScreen);
        original_hook(info);
    }));

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let _guard = TerminalGuard;

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let poll_timeout = Duration::from_millis(120);
    let loading_poll = Duration::from_millis(80);

    let mut state = State::Loading {
        rx: app::App::spawn_load(),
        start: Instant::now(),
    };

    loop {
        match &mut state {
            State::Loading { rx, start } => {
                let elapsed = start.elapsed();
                terminal.draw(|frame| draw_loading(frame, elapsed))?;

                if let Ok(app) = rx.try_recv() {
                    state = State::Ready(app);
                    continue;
                }

                if event::poll(loading_poll)?
                    && let CrosstermEvent::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press
                    && (key.code == KeyCode::Char('q')
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)))
                {
                    break;
                }
            }
            State::Ready(app) => {
                app.pre_render();
                terminal.draw(|frame| app.draw(frame))?;
                app.handle_reload();

                if !event::poll(poll_timeout)? {
                    continue;
                }

                if let CrosstermEvent::Key(key) = event::read()?
                    && !app.handle_input(key)
                {
                    break;
                }
            }
        }
    }

    Ok(())
}

fn run_backfill_cli(dry_run: bool, source: SourceArg) -> io::Result<()> {
    use crate::data::backfill::{self, BackfillOptions, SourceSel};

    let source = match source {
        SourceArg::All => SourceSel::All,
        SourceArg::StatsCache => SourceSel::StatsCache,
        SourceArg::CodeInsights => SourceSel::CodeInsights,
    };

    let summary = backfill::run_backfill(&BackfillOptions { dry_run, source });

    let verb = if summary.dry_run {
        "would backfill"
    } else {
        "backfilled"
    };
    match (summary.earliest, summary.latest) {
        (Some(e), Some(l)) => println!(
            "{verb} {} day(s), {e} -> {l}: {} input + {} output tokens, ${:.2}",
            summary.days, summary.total_input, summary.total_output, summary.total_cost
        ),
        _ => println!("{verb} 0 days (no historical sources found)"),
    }
    Ok(())
}
