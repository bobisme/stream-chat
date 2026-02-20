use clap::{ArgAction, Parser};
use std::{
    io::{self, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use ytchat_webview::{spawn_chat_observer, ObserverErrorKind, ObserverEvent, ObserverOptions};

#[derive(Parser, Debug)]
#[command(name = "ytchat-stdout")]
#[command(about = "Stream YouTube live chat to stdout using an embedded WebView")]
struct Cli {
    /// YouTube stream URL or raw 11-char video ID.
    stream_url_or_video_id: String,

    /// Hide the window when supported.
    #[arg(
        long,
        default_value_t = true,
        action = ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    headless: bool,

    /// Print JSON lines instead of TSV.
    #[arg(long)]
    json: bool,

    /// Exit if chat DOM is not detected in time.
    #[arg(long, default_value_t = 25)]
    timeout: u64,

    /// Print diagnostic logs to stderr.
    #[arg(long)]
    verbose: bool,

    /// Persisted webview profile directory for cookies/session.
    #[arg(long)]
    profile_dir: Option<PathBuf>,

    /// Disable profile persistence (no saved login/session).
    #[arg(long)]
    ephemeral: bool,
}

fn main() {
    let cli = Cli::parse();
    match ytchat_webview::maybe_reexec_with_linux_webview_env(cli.verbose) {
        Ok(Some(code)) => std::process::exit(code),
        Ok(None) => {}
        Err(err) => {
            eprintln!("failed to initialize linux webview env: {err}");
            std::process::exit(3);
        }
    }

    let options = ObserverOptions {
        headless: cli.headless,
        timeout: Duration::from_secs(cli.timeout),
        verbose: cli.verbose,
        profile_dir: if cli.ephemeral {
            None
        } else {
            cli.profile_dir.or_else(ytchat_webview::default_profile_dir)
        },
    };

    let (observer, rx) = match spawn_chat_observer(&cli.stream_url_or_video_id, options) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("{err}");
            let code = if err.kind() == ObserverErrorKind::InvalidInput {
                2
            } else {
                3
            };
            std::process::exit(code);
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        if let Err(err) = ctrlc::set_handler(move || {
            stop.store(true, Ordering::SeqCst);
        }) {
            eprintln!("failed to install Ctrl-C handler: {err}");
            std::process::exit(3);
        }
    }

    let mut stdout = io::stdout();

    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ObserverEvent::Ready) => {
                if cli.verbose {
                    eprintln!("chat DOM detected");
                }
            }
            Ok(ObserverEvent::Debug(msg)) => {
                if cli.verbose {
                    eprintln!("{msg}");
                }
            }
            Ok(ObserverEvent::Chat(line)) => {
                if cli.json {
                    let json = serde_json::json!({
                        "ts": line.ts,
                        "user": line.user,
                        "msg": line.msg,
                    });
                    let _ = writeln!(stdout, "{json}");
                } else {
                    let _ = writeln!(stdout, "{}\t{}\t{}", line.ts, line.user, line.msg);
                }
                let _ = stdout.flush();
            }
            Ok(ObserverEvent::MessageSent) => {}
            Ok(ObserverEvent::SendError(msg)) => {
                if cli.verbose {
                    eprintln!("send error: {msg}");
                }
            }
            Ok(ObserverEvent::Timeout) => {
                eprintln!(
                    "timeout waiting for chat DOM. Try --timeout <secs> or run with --headless=false to handle consent/cookies."
                );
                observer.stop();
                observer.join();
                std::process::exit(4);
            }
            Ok(ObserverEvent::Error(msg)) => {
                eprintln!("{msg}");
                observer.stop();
                observer.join();
                std::process::exit(3);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    observer.stop();
    observer.join();
    std::process::exit(0);
}
