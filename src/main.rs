mod app;
mod auth;
mod events;
mod message;
mod ui;
mod youtube;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::{App, AppAction};
use events::{AppEvent, EventHandler, MessageSender};

#[derive(Parser)]
#[command(name = "streamchat")]
#[command(about = "Join YouTube live stream chat from your terminal")]
struct Cli {
    /// YouTube video URL (must be a live stream or premiere)
    url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // 1. Parse video ID from URL
    let video_id = youtube::extract_video_id(&cli.url)?;

    // 2. Initialize OAuth and create YouTube client
    eprintln!("Authenticating with YouTube...");
    let hub = auth::create_youtube_client().await?;

    // 3. Get stream info (title and live chat ID)
    eprintln!("Connecting to stream...");
    let stream_info = youtube::get_stream_info(&hub, &video_id).await?;
    eprintln!("Connected to: {} - {}", stream_info.channel_name, stream_info.title);

    // 4. Initialize app state
    let title = format!("{} - {}", stream_info.channel_name, stream_info.title);
    let live_chat_id = stream_info.live_chat_id;
    let mut app = App::new(title, live_chat_id.clone());

    // 5. Setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 6. Create event handler and message sender
    // We need to create a second hub for the message sender since YouTube client isn't Clone
    let hub2 = auth::create_youtube_client().await?;

    let mut events = EventHandler::new(hub, live_chat_id.clone());
    let sender = MessageSender::new(hub2, live_chat_id, events.sender());

    // 7. Main loop
    let result = run_app(&mut terminal, &mut app, &mut events, &sender).await;

    // 8. Cleanup terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    events: &mut EventHandler,
    sender: &MessageSender,
) -> Result<()> {
    loop {
        // Render UI
        terminal.draw(|frame| ui::render(frame, app))?;

        // Handle events
        if let Some(event) = events.next().await {
            match event {
                AppEvent::Key(key) => {
                    if let Some(action) = app.handle_key(key) {
                        match action {
                            AppAction::SendMessage(msg) => {
                                app.is_sending = true;
                                app.error_message = None;
                                sender.send(msg);
                            }
                        }
                    }
                }
                AppEvent::NewMessages(messages) => {
                    app.add_messages(messages);
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
