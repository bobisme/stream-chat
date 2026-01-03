use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::auth::YouTubeClient;
use crate::message::ChatMessage;
use crate::youtube::ChatPoller;

pub enum AppEvent {
    Key(KeyEvent),
    NewMessages(Vec<ChatMessage>),
    MessageSent,
    Error(String),
}

pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<AppEvent>,
    tx: mpsc::UnboundedSender<AppEvent>,
    _task: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub fn new(hub: YouTubeClient, live_chat_id: String) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let task = tokio::spawn(async move {
            if let Err(e) = event_loop(tx_clone, hub, live_chat_id).await {
                eprintln!("Event loop error: {}", e);
            }
        });

        Self { rx, tx, _task: task }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<AppEvent> {
        self.tx.clone()
    }

    pub async fn next(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
    }
}

async fn event_loop(
    tx: mpsc::UnboundedSender<AppEvent>,
    hub: YouTubeClient,
    live_chat_id: String,
) -> Result<()> {
    let mut event_stream = EventStream::new();
    let mut chat_poller = ChatPoller::new(hub, live_chat_id);

    // Track when we should next poll
    let mut next_poll = tokio::time::Instant::now();

    loop {
        tokio::select! {
            // Handle keyboard/terminal events
            event_result = event_stream.next() => {
                match event_result {
                    Some(Ok(Event::Key(key))) => {
                        if key.kind == KeyEventKind::Press {
                            if tx.send(AppEvent::Key(key)).is_err() {
                                break;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        let _ = tx.send(AppEvent::Error(format!("Terminal event error: {}", e)));
                    }
                    None => break,
                    _ => {}
                }
            }

            // Poll YouTube API for new messages
            _ = tokio::time::sleep_until(next_poll) => {
                match chat_poller.poll().await {
                    Ok(messages) if !messages.is_empty() => {
                        if tx.send(AppEvent::NewMessages(messages)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(format!("YouTube API error: {}", e)));
                    }
                    _ => {}
                }
                // Schedule next poll based on API response
                next_poll = tokio::time::Instant::now() + chat_poller.poll_interval();
            }
        }
    }

    Ok(())
}

/// Channel for sending messages to YouTube
pub struct MessageSender {
    tx: mpsc::UnboundedSender<String>,
    _task: tokio::task::JoinHandle<()>,
}

impl MessageSender {
    pub fn new(
        hub: YouTubeClient,
        live_chat_id: String,
        event_tx: mpsc::UnboundedSender<AppEvent>,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        let task = tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                match crate::youtube::send_message(&hub, &live_chat_id, &message).await {
                    Ok(()) => {
                        let _ = event_tx.send(AppEvent::MessageSent);
                    }
                    Err(e) => {
                        let _ = event_tx.send(AppEvent::Error(format!("Failed to send: {}", e)));
                    }
                }
            }
        });

        Self { tx, _task: task }
    }

    pub fn send(&self, message: String) {
        let _ = self.tx.send(message);
    }
}
