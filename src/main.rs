mod app;
mod events;
mod message;
mod ui;

use anyhow::Result;
use clap::{ArgAction, Parser};
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::path::PathBuf;

use app::{App, AppAction};
use events::{AppEvent, EventHandler};

#[derive(Parser)]
#[command(name = "streamchat")]
#[command(about = "Join YouTube live stream chat from your terminal")]
struct Cli {
    /// `YouTube` stream URL or raw 11-char video ID
    url: String,

    /// Your `YouTube` @username for mention highlighting (without @)
    #[arg(short, long)]
    username: Option<String>,

    /// Hide webview window if supported
    #[arg(
        long,
        default_value_t = true,
        action = ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    headless: bool,

    /// Timeout waiting for chat DOM
    #[arg(long, default_value_t = 25)]
    timeout: u64,

    /// Verbose diagnostics to stderr
    #[arg(long)]
    verbose: bool,

    /// Write verbose debug logs to file (recommended for TUI)
    #[arg(long)]
    debug_log: Option<PathBuf>,

    /// Persisted webview profile directory for cookies/session
    #[arg(long)]
    profile_dir: Option<PathBuf>,

    /// Disable profile persistence (no saved login/session)
    #[arg(long)]
    ephemeral: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let debug_enabled = cli.verbose || cli.debug_log.is_some();

    if let Some(code) = ytchat_webview::maybe_reexec_with_linux_webview_env(debug_enabled)? {
        std::process::exit(code);
    }

    let video_hint = ytchat_webview::extract_video_id(&cli.url).ok_or_else(|| {
        anyhow::anyhow!("invalid YouTube URL/video ID; expected an 11-char video id")
    })?;
    let title = format!("YouTube Live Chat - {video_hint}");

    let mut app = App::new(title, video_hint, cli.username.unwrap_or_default());

    let debug_log_path = if debug_enabled {
        Some(
            cli.debug_log
                .clone()
                .unwrap_or_else(|| std::env::temp_dir().join("streamchat-debug.log")),
        )
    } else {
        None
    };

    if let Some(path) = &debug_log_path {
        eprintln!("debug logs -> {}", path.display());
    }

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let profile_dir = if cli.ephemeral {
        None
    } else {
        cli.profile_dir.or_else(ytchat_webview::default_profile_dir)
    };

    let mut events = match EventHandler::new(
        &cli.url,
        cli.headless,
        cli.timeout,
        debug_enabled,
        profile_dir,
        debug_log_path,
    ) {
        Ok(events) => events,
        Err(err) => {
            disable_raw_mode()?;
            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
            return Err(err);
        }
    };

    let result = run_app(&mut terminal, &mut app, &mut events).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    events.shutdown();

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    events: &mut EventHandler,
) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::render(frame, app))?;

        if let Some(event) = events.next().await {
            match event {
                AppEvent::Key(key) => {
                    if let Some(action) = app.handle_key(key) {
                        match action {
                            AppAction::SendMessage(message) => {
                                app.is_sending = true;
                                app.error_message = None;
                                if let Err(err) = events.send_message(message) {
                                    app.error_message =
                                        Some(format!("Failed to queue message send: {err}"));
                                    app.is_sending = false;
                                }
                            }
                        }
                    }
                }
                AppEvent::NewMessages(messages) => {
                    app.add_messages(messages);
                }
                AppEvent::Resize => {
                    terminal.autoresize()?;
                }
                AppEvent::MessageSent => {
                    app.is_sending = false;
                }
                AppEvent::Error(err) => {
                    app.error_message = Some(err);
                    app.is_sending = false;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
