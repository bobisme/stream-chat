use anyhow::{anyhow, Result};
use google_youtube3::api::{LiveChatMessage, LiveChatMessageSnippet, LiveChatTextMessageDetails};
use google_youtube3::hyper_util::client::legacy::connect::Connect;
use google_youtube3::YouTube;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

use crate::message::{AuthorType, ChatMessage, SuperChatInfo};

/// Extract video ID from various YouTube URL formats
pub fn extract_video_id(url_str: &str) -> Result<String> {
    let url = Url::parse(url_str)?;

    match url.host_str() {
        Some("www.youtube.com") | Some("youtube.com") | Some("m.youtube.com") => {
            // Handle: youtube.com/watch?v=VIDEO_ID
            // Handle: youtube.com/live/VIDEO_ID
            if url.path().starts_with("/live/") {
                url.path_segments()
                    .and_then(|mut s| {
                        s.next(); // skip "live"
                        s.next()
                    })
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow!("No video ID in live URL"))
            } else {
                url.query_pairs()
                    .find(|(key, _)| key == "v")
                    .map(|(_, value)| value.to_string())
                    .ok_or_else(|| anyhow!("No video ID in URL"))
            }
        }
        Some("youtu.be") => {
            // Handle: youtu.be/VIDEO_ID
            url.path_segments()
                .and_then(|mut segments| segments.next())
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("No video ID in short URL"))
        }
        _ => Err(anyhow!("Unsupported YouTube URL format")),
    }
}

pub struct StreamInfo {
    pub title: String,
    pub channel_name: String,
    pub live_chat_id: String,
}

/// Get the authenticated user's @handle (e.g., "bobnull" from "@bobnull")
pub async fn get_my_handle<C>(hub: &YouTube<C>) -> Result<String>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    let (_, response) = hub
        .channels()
        .list(&vec!["snippet".into()])
        .mine(true)
        .add_scope("https://www.googleapis.com/auth/youtube.force-ssl")
        .doit()
        .await?;

    let channel = response
        .items
        .and_then(|items| items.into_iter().next())
        .ok_or_else(|| anyhow!("Could not get your channel info"))?;

    let handle = channel
        .snippet
        .and_then(|s| s.custom_url)
        .map(|url| url.trim_start_matches('@').to_string())
        .ok_or_else(|| anyhow!("No handle found for your channel"))?;

    Ok(handle)
}

/// Get stream title, channel name, and live chat ID from a video
pub async fn get_stream_info<C>(hub: &YouTube<C>, video_id: &str) -> Result<StreamInfo>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    let (_, response) = hub
        .videos()
        .list(&vec!["snippet".into(), "liveStreamingDetails".into()])
        .add_id(video_id)
        .add_scope("https://www.googleapis.com/auth/youtube.force-ssl")
        .doit()
        .await?;

    let video = response
        .items
        .and_then(|items| items.into_iter().next())
        .ok_or_else(|| anyhow!("Video not found"))?;

    let snippet = video.snippet.ok_or_else(|| anyhow!("No snippet data"))?;

    let title = snippet.title.unwrap_or_else(|| "Unknown Stream".to_string());
    let channel_name = snippet
        .channel_title
        .unwrap_or_else(|| "Unknown Channel".to_string());

    let live_chat_id = video
        .live_streaming_details
        .and_then(|d| d.active_live_chat_id)
        .ok_or_else(|| anyhow!("No active live chat - stream may not be live"))?;

    Ok(StreamInfo {
        title,
        channel_name,
        live_chat_id,
    })
}

// Minimum poll interval to conserve quota (10 seconds)
const MIN_POLL_INTERVAL_MS: u64 = 10_000;

/// Polls YouTube live chat for new messages
pub struct ChatPoller<C>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    hub: Arc<YouTube<C>>,
    live_chat_id: String,
    next_page_token: Option<String>,
    poll_interval_ms: u64,
    error_backoff_ms: u64,
}

impl<C> ChatPoller<C>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    pub fn new(hub: Arc<YouTube<C>>, live_chat_id: String) -> Self {
        Self {
            hub,
            live_chat_id,
            next_page_token: None,
            poll_interval_ms: MIN_POLL_INTERVAL_MS,
            error_backoff_ms: 0,
        }
    }

    pub async fn poll(&mut self) -> Result<Vec<ChatMessage>> {
        let mut request = self
            .hub
            .live_chat_messages()
            .list(&self.live_chat_id, &vec!["snippet".into(), "authorDetails".into()])
            .add_scope("https://www.googleapis.com/auth/youtube.force-ssl");

        if let Some(ref token) = self.next_page_token {
            request = request.page_token(token);
        }

        match request.doit().await {
            Ok((_, response)) => {
                // Success - reset error backoff
                self.error_backoff_ms = 0;

                // Update polling state
                self.next_page_token = response.next_page_token;
                if let Some(interval) = response.polling_interval_millis {
                    // Enforce minimum interval to conserve quota
                    self.poll_interval_ms = (interval as u64).max(MIN_POLL_INTERVAL_MS);
                }

                // Convert API messages to our ChatMessage type
                let messages = response
                    .items
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(convert_to_chat_message)
                    .collect();

                Ok(messages)
            }
            Err(e) => {
                // Exponential backoff on errors: 10s, 20s, 40s, 80s, max 5min
                if self.error_backoff_ms == 0 {
                    self.error_backoff_ms = MIN_POLL_INTERVAL_MS;
                } else {
                    self.error_backoff_ms = (self.error_backoff_ms * 2).min(300_000);
                }
                Err(e.into())
            }
        }
    }

    pub fn poll_interval(&self) -> Duration {
        // Use error backoff if we're in an error state, otherwise normal interval
        let ms = if self.error_backoff_ms > 0 {
            self.error_backoff_ms
        } else {
            self.poll_interval_ms
        };
        Duration::from_millis(ms)
    }
}

fn convert_to_chat_message(item: google_youtube3::api::LiveChatMessage) -> Option<ChatMessage> {
    let snippet = item.snippet?;
    let author = item.author_details?;

    let author_type = if author.is_chat_owner.unwrap_or(false) {
        AuthorType::Owner
    } else if author.is_chat_moderator.unwrap_or(false) {
        AuthorType::Moderator
    } else if author.is_chat_sponsor.unwrap_or(false) {
        AuthorType::Member
    } else {
        AuthorType::Regular
    };

    let super_chat = snippet.super_chat_details.map(|sc| SuperChatInfo {
        amount_display: sc.amount_display_string.unwrap_or_default(),
        tier: sc.tier.unwrap_or(1) as u32,
    });

    Some(ChatMessage {
        id: item.id?,
        author_name: author.display_name?,
        message: snippet.display_message.unwrap_or_default(),
        author_type,
        super_chat,
    })
}

/// Send a message to the live chat
pub async fn send_message<C>(hub: &YouTube<C>, live_chat_id: &str, message_text: &str) -> Result<()>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    let message = LiveChatMessage {
        snippet: Some(LiveChatMessageSnippet {
            live_chat_id: Some(live_chat_id.to_string()),
            type_: Some("textMessageEvent".to_string()),
            text_message_details: Some(LiveChatTextMessageDetails {
                message_text: Some(message_text.to_string()),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    hub.live_chat_messages()
        .insert(message)
        .add_part("snippet")
        .add_scope("https://www.googleapis.com/auth/youtube.force-ssl")
        .doit()
        .await?;

    Ok(())
}
