use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind};
use futures::StreamExt;
use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tokio::sync::mpsc;
use ytchat_webview::{
    AuthorRole, BadgeKind as WebBadgeKind, ChatLine, ObserverEvent, ObserverOptions,
    spawn_chat_observer,
};

use crate::message::{AuthorType, Badge, BadgeKind, ChatMessage};

pub enum AppEvent {
    Key(KeyEvent),
    Resize,
    NewMessages(Vec<ChatMessage>),
    MessageSent,
    Error(String),
}

pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<AppEvent>,
    keyboard_task: tokio::task::JoinHandle<()>,
    observer_task: Option<thread::JoinHandle<()>>,
    observer: Option<ytchat_webview::ObserverHandle>,
}

impl EventHandler {
    pub fn new(
        stream_url_or_video_id: &str,
        headless: bool,
        timeout_secs: u64,
        debug_enabled: bool,
        profile_dir: Option<PathBuf>,
        debug_log_path: Option<PathBuf>,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();

        let debug_logger = if let Some(path) = debug_log_path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let file = OpenOptions::new().create(true).append(true).open(path)?;
            Some(Arc::new(Mutex::new(file)))
        } else {
            None
        };

        let options = ObserverOptions {
            headless,
            timeout: Duration::from_secs(timeout_secs),
            verbose: debug_enabled,
            profile_dir,
        };
        let (observer, observer_rx) = spawn_chat_observer(stream_url_or_video_id, options)?;

        let keyboard_tx = tx.clone();
        let keyboard_task = tokio::spawn(async move {
            let mut event_stream = EventStream::new();

            loop {
                match event_stream.next().await {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        if keyboard_tx.send(AppEvent::Key(key)).is_err() {
                            break;
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        if keyboard_tx.send(AppEvent::Resize).is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        let _ = keyboard_tx
                            .send(AppEvent::Error(format!("Terminal event error: {err}")));
                    }
                    None => break,
                }
            }
        });

        let observer_tx = tx;
        let debug_logger_for_thread = debug_logger;
        let observer_task = thread::spawn(move || {
            while let Ok(event) = observer_rx.recv() {
                match event {
                    ObserverEvent::Ready => {}
                    ObserverEvent::Chat(line) => {
                        if debug_enabled {
                            let badge_summary: Vec<String> = line
                                .badges
                                .iter()
                                .map(|b| format!("{}:{:?}", b.text, b.kind))
                                .collect();
                            log_debug(
                                debug_logger_for_thread.as_ref(),
                                &format!(
                                    "[badge-debug] user={} role={:?} badges={:?}",
                                    line.user, line.role, badge_summary
                                ),
                            );
                        }

                        let chat = convert_chat_line(line);
                        let _ = observer_tx.send(AppEvent::NewMessages(vec![chat]));
                    }
                    ObserverEvent::MessageSent => {
                        let _ = observer_tx.send(AppEvent::MessageSent);
                    }
                    ObserverEvent::SendError(msg) => {
                        let _ = observer_tx
                            .send(AppEvent::Error(format!("Failed to send message: {msg}")));
                    }
                    ObserverEvent::Timeout => {
                        let _ = observer_tx.send(AppEvent::Error(
                            "Timeout waiting for chat DOM. If YouTube shows consent/cookies, rerun with --headless=false and increase --timeout.".to_string(),
                        ));
                    }
                    ObserverEvent::Error(msg) => {
                        let _ = observer_tx.send(AppEvent::Error(msg));
                    }
                    ObserverEvent::Debug(msg) => {
                        if debug_enabled {
                            log_debug(
                                debug_logger_for_thread.as_ref(),
                                &format!("[webview] {msg}"),
                            );
                        }
                    }
                }
            }
        });

        Ok(Self {
            rx,
            keyboard_task,
            observer_task: Some(observer_task),
            observer: Some(observer),
        })
    }

    pub async fn next(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
    }

    pub fn send_message(&self, message: String) -> Result<()> {
        if let Some(observer) = &self.observer {
            observer.send_message(message)?;
        }
        Ok(())
    }

    pub fn shutdown(&mut self) {
        self.keyboard_task.abort();

        if let Some(observer) = self.observer.take() {
            observer.stop();
            drop(observer);
        }

        let _ = self.observer_task.take();
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        self.keyboard_task.abort();
        if let Some(observer) = &self.observer {
            observer.stop();
        }
        let _ = self.observer_task.take();
    }
}

fn convert_chat_line(line: ChatLine) -> ChatMessage {
    ChatMessage {
        id: format!("{}:{}", line.ts, line.user),
        author_name: line.user,
        message: line.msg,
        author_type: map_role(line.role),
        badges: line.badges.into_iter().map(map_badge).collect(),
        super_chat: None,
    }
}

const fn map_role(role: AuthorRole) -> AuthorType {
    match role {
        AuthorRole::Owner => AuthorType::Owner,
        AuthorRole::Moderator => AuthorType::Moderator,
        AuthorRole::Member => AuthorType::Member,
        AuthorRole::Regular => AuthorType::Regular,
    }
}

fn map_badge(badge: ytchat_webview::ChatBadge) -> Badge {
    Badge {
        text: badge.text,
        kind: match badge.kind {
            WebBadgeKind::Owner => BadgeKind::Owner,
            WebBadgeKind::Moderator => BadgeKind::Moderator,
            WebBadgeKind::Member => BadgeKind::Member,
            WebBadgeKind::Rank => BadgeKind::Rank,
            WebBadgeKind::Other => BadgeKind::Other,
        },
    }
}

fn log_debug(logger: Option<&Arc<Mutex<std::fs::File>>>, message: &str) {
    if let Some(logger) = logger
        && let Ok(mut file) = logger.lock()
    {
        let _ = writeln!(file, "{message}");
        let _ = file.flush();
    }
}
