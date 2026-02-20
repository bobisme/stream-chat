use chrono::{SecondsFormat, Utc};
use serde::Deserialize;
use std::{
    fmt,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::Duration,
};
#[cfg(target_os = "linux")]
use tao::platform::unix::EventLoopBuilderExtUnix;
#[cfg(target_os = "linux")]
use tao::platform::unix::WindowBuilderExtUnix;
#[cfg(target_os = "linux")]
use tao::platform::unix::WindowExtUnix;
#[cfg(target_os = "windows")]
use tao::platform::windows::EventLoopBuilderExtWindows;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::run_return::EventLoopExtRunReturn,
    window::WindowBuilder,
};
use url::Url;
#[cfg(target_os = "linux")]
use wry::WebViewBuilderExtUnix;
use wry::{http::Request, BackgroundThrottlingPolicy, WebContext, WebView, WebViewBuilder};

const MAX_MESSAGE_LEN: usize = 4096;

pub fn default_profile_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("streamchat").join("webview-profile"))
}

pub fn maybe_reexec_with_linux_webview_env(verbose: bool) -> std::io::Result<Option<i32>> {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("STREAMCHAT_ENV_BOOTSTRAPPED").is_some() {
            return Ok(None);
        }

        let mut overrides: Vec<(String, String)> = Vec::new();
        let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        let has_x11_display = std::env::var_os("DISPLAY").is_some();
        let backend_already_set = std::env::var_os("WINIT_UNIX_BACKEND").is_some();
        let is_wayland = session_type.eq_ignore_ascii_case("wayland");

        if is_wayland && has_x11_display && !backend_already_set {
            overrides.push(("WINIT_UNIX_BACKEND".to_string(), "x11".to_string()));
            if std::env::var_os("GDK_BACKEND").is_none() {
                overrides.push(("GDK_BACKEND".to_string(), "x11".to_string()));
            }

            if verbose {
                eprintln!(
                    "[webview] Wayland detected; preferring X11 backend for WebKit stability"
                );
            }
        }

        if is_wayland {
            if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
                overrides.push((
                    "WEBKIT_DISABLE_DMABUF_RENDERER".to_string(),
                    "1".to_string(),
                ));
                if verbose {
                    eprintln!("[webview] disabling WebKit dmabuf renderer on Wayland");
                }
            }

            if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
                overrides.push((
                    "WEBKIT_DISABLE_COMPOSITING_MODE".to_string(),
                    "1".to_string(),
                ));
                if verbose {
                    eprintln!("[webview] disabling WebKit compositing mode on Wayland");
                }
            }
        }

        if overrides.is_empty() {
            return Ok(None);
        }

        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(exe);
        cmd.args(std::env::args_os().skip(1));
        cmd.env("STREAMCHAT_ENV_BOOTSTRAPPED", "1");
        for (key, value) in overrides {
            cmd.env(key, value);
        }

        let status = cmd.status()?;
        Ok(Some(status.code().unwrap_or(1)))
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = verbose;
        Ok(None)
    }
}

#[derive(Debug, Clone)]
pub struct ChatLine {
    pub ts: String,
    pub user: String,
    pub msg: String,
    pub role: AuthorRole,
    pub badges: Vec<ChatBadge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorRole {
    Owner,
    Moderator,
    Member,
    Regular,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    Owner,
    Moderator,
    Member,
    Rank,
    Other,
}

#[derive(Debug, Clone)]
pub struct ChatBadge {
    pub text: String,
    pub kind: BadgeKind,
}

#[derive(Debug, Clone)]
pub enum ObserverEvent {
    Ready,
    Chat(ChatLine),
    MessageSent,
    SendError(String),
    Debug(String),
    Timeout,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct ObserverOptions {
    pub headless: bool,
    pub timeout: Duration,
    pub verbose: bool,
    pub profile_dir: Option<PathBuf>,
}

impl Default for ObserverOptions {
    fn default() -> Self {
        Self {
            headless: true,
            timeout: Duration::from_secs(25),
            verbose: false,
            profile_dir: default_profile_dir(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ObserverError {
    kind: ObserverErrorKind,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObserverErrorKind {
    InvalidInput,
    Startup,
    ControlChannelClosed,
}

impl ObserverError {
    fn invalid_input(message: impl Into<String>) -> Self {
        Self {
            kind: ObserverErrorKind::InvalidInput,
            message: message.into(),
        }
    }

    fn startup(message: impl Into<String>) -> Self {
        Self {
            kind: ObserverErrorKind::Startup,
            message: message.into(),
        }
    }

    fn control_channel_closed(message: impl Into<String>) -> Self {
        Self {
            kind: ObserverErrorKind::ControlChannelClosed,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> ObserverErrorKind {
        self.kind
    }
}

impl fmt::Display for ObserverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ObserverError {}

#[derive(Clone, Debug)]
enum UserEvent {
    Stop,
    DomTimeout,
    SendMessage(String),
}

#[derive(Debug, Deserialize)]
struct IpcEvent {
    kind: String,
    ts: Option<String>,
    user: Option<String>,
    msg: Option<String>,
    role: Option<String>,
    badges: Option<Vec<IpcBadge>>,
}

#[derive(Debug, Deserialize)]
struct IpcBadge {
    text: Option<String>,
    kind: Option<String>,
}

pub struct ObserverHandle {
    proxy: EventLoopProxy<UserEvent>,
    join: Option<thread::JoinHandle<()>>,
}

impl ObserverHandle {
    pub fn stop(&self) {
        let _ = self.proxy.send_event(UserEvent::Stop);
    }

    pub fn join(mut self) {
        self.stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    pub fn send_message(&self, message: String) -> Result<(), ObserverError> {
        self.proxy
            .send_event(UserEvent::SendMessage(message))
            .map_err(|_| {
                ObserverError::control_channel_closed("failed to send message to webview thread")
            })
    }
}

impl Drop for ObserverHandle {
    fn drop(&mut self) {
        let _ = self.proxy.send_event(UserEvent::Stop);
        let _ = self.join.take();
    }
}

pub fn spawn_chat_observer(
    stream_url_or_video_id: &str,
    options: ObserverOptions,
) -> Result<(ObserverHandle, Receiver<ObserverEvent>), ObserverError> {
    let video_id = extract_video_id(stream_url_or_video_id).ok_or_else(|| {
        ObserverError::invalid_input("invalid YouTube URL/video ID; expected an 11-char video id")
    })?;

    let chat_url = format!(
        "https://www.youtube.com/live_chat?is_popout=1&v={}",
        video_id
    );

    let (event_tx, event_rx) = mpsc::channel::<ObserverEvent>();
    let (proxy_tx, proxy_rx) = mpsc::sync_channel(1);

    let join = thread::spawn(move || {
        run_observer_thread(chat_url, options, event_tx, proxy_tx);
    });

    let proxy = proxy_rx
        .recv()
        .map_err(|_| ObserverError::startup("observer failed to initialize event loop"))?;

    Ok((
        ObserverHandle {
            proxy,
            join: Some(join),
        },
        event_rx,
    ))
}

fn run_observer_thread(
    chat_url: String,
    options: ObserverOptions,
    event_tx: Sender<ObserverEvent>,
    proxy_tx: mpsc::SyncSender<EventLoopProxy<UserEvent>>,
) {
    let profile_dir = options.profile_dir.clone();

    let mut event_loop_builder = EventLoopBuilder::<UserEvent>::with_user_event();
    #[cfg(target_os = "linux")]
    event_loop_builder.with_any_thread(true);
    #[cfg(target_os = "windows")]
    event_loop_builder.with_any_thread(true);
    let mut event_loop = event_loop_builder.build();
    let proxy = event_loop.create_proxy();

    if proxy_tx.send(proxy.clone()).is_err() {
        return;
    }

    let dom_ready = Arc::new(AtomicBool::new(false));

    {
        let proxy = proxy.clone();
        let dom_ready = Arc::clone(&dom_ready);
        let timeout = options.timeout;
        thread::spawn(move || {
            thread::sleep(timeout);
            if !dom_ready.load(Ordering::SeqCst) {
                let _ = proxy.send_event(UserEvent::DomTimeout);
            }
        });
    }

    let window_builder = WindowBuilder::new()
        .with_title("ytchat-webview")
        .with_visible(!options.headless)
        .with_inner_size(LogicalSize::new(640.0, 480.0))
        .with_min_inner_size(LogicalSize::new(320.0, 240.0));
    #[cfg(target_os = "linux")]
    let window_builder = window_builder.with_default_vbox(false);

    let window = match window_builder.build(&event_loop) {
        Ok(window) => window,
        Err(err) => {
            let _ = event_tx.send(ObserverEvent::Error(format!(
                "failed to create window: {err}"
            )));
            return;
        }
    };

    let verbose = options.verbose;

    let mut web_context = if let Some(profile_dir) = profile_dir {
        if let Err(err) = std::fs::create_dir_all(&profile_dir) {
            let _ = event_tx.send(ObserverEvent::Error(format!(
                "failed to create webview profile dir {}: {err}",
                profile_dir.display()
            )));
            return;
        }

        if options.verbose {
            let _ = event_tx.send(ObserverEvent::Debug(format!(
                "using webview profile dir {}",
                profile_dir.display()
            )));
        }

        Some(WebContext::new(Some(profile_dir)))
    } else {
        if options.verbose {
            let _ = event_tx.send(ObserverEvent::Debug(
                "using ephemeral webview session".to_string(),
            ));
        }
        None
    };

    let webview = match build_webview(
        &window,
        &chat_url,
        Arc::clone(&dom_ready),
        event_tx.clone(),
        verbose,
        web_context.as_mut(),
    ) {
        Ok(webview) => webview,
        Err(first_err) => {
            if web_context.is_some() {
                let _ = event_tx.send(ObserverEvent::Debug(
                    "persistent webview profile failed, retrying with ephemeral session"
                        .to_string(),
                ));

                match build_webview(
                    &window,
                    &chat_url,
                    Arc::clone(&dom_ready),
                    event_tx.clone(),
                    verbose,
                    None,
                ) {
                    Ok(webview) => {
                        let _ = event_tx.send(ObserverEvent::Debug(
                            "webview started in ephemeral fallback mode".to_string(),
                        ));
                        webview
                    }
                    Err(second_err) => {
                        let _ = event_tx.send(ObserverEvent::Error(format!(
                            "failed to build webview with profile ({first_err}); fallback failed ({second_err})"
                        )));
                        return;
                    }
                }
            } else {
                let _ = event_tx.send(ObserverEvent::Error(first_err));
                return;
            }
        }
    };

    if options.verbose {
        let _ = event_tx.send(ObserverEvent::Debug("webview started".to_string()));
    }

    let _exit_code = event_loop.run_return(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(UserEvent::Stop) => {
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::SendMessage(message)) => {
                let js = build_send_message_script(&message);
                if let Err(err) = webview.evaluate_script(&js) {
                    let _ = event_tx.send(ObserverEvent::SendError(format!(
                        "failed to evaluate send-message script: {err}"
                    )));
                }
            }
            Event::UserEvent(UserEvent::DomTimeout) => {
                if !dom_ready.load(Ordering::SeqCst) {
                    let _ = event_tx.send(ObserverEvent::Timeout);
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Destroyed,
                ..
            } => {
                let _ = event_tx.send(ObserverEvent::Error(
                    "webview window destroyed unexpectedly".to_string(),
                ));
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

fn build_webview(
    window: &tao::window::Window,
    chat_url: &str,
    dom_ready_for_ipc: Arc<AtomicBool>,
    event_tx_for_ipc: Sender<ObserverEvent>,
    verbose: bool,
    web_context: Option<&mut WebContext>,
) -> Result<WebView, String> {
    let webview_builder = if let Some(context) = web_context {
        WebViewBuilder::new_with_web_context(context)
    } else {
        WebViewBuilder::new()
    }
    .with_url(chat_url)
    .with_background_throttling(BackgroundThrottlingPolicy::Disabled)
    .with_initialization_script(CHAT_OBSERVER_JS)
    .with_ipc_handler(move |request: Request<String>| {
        handle_ipc(
            request.body(),
            verbose,
            &dom_ready_for_ipc,
            &event_tx_for_ipc,
        );
    });

    #[cfg(target_os = "linux")]
    {
        webview_builder
            .build_gtk(window.gtk_window())
            .map_err(|err| format!("failed to build webview: {err}"))
    }

    #[cfg(not(target_os = "linux"))]
    {
        webview_builder
            .build(window)
            .map_err(|err| format!("failed to build webview: {err}"))
    }
}

fn handle_ipc(
    payload: &str,
    verbose: bool,
    dom_ready: &Arc<AtomicBool>,
    event_tx: &Sender<ObserverEvent>,
) {
    let event = match serde_json::from_str::<IpcEvent>(payload) {
        Ok(event) => event,
        Err(err) => {
            if verbose {
                let _ = event_tx.send(ObserverEvent::Debug(format!(
                    "failed to parse ipc payload: {err}; payload={payload}"
                )));
            }
            return;
        }
    };

    match event.kind.as_str() {
        "ready" => {
            dom_ready.store(true, Ordering::SeqCst);
            let _ = event_tx.send(ObserverEvent::Ready);
        }
        "debug" => {
            if verbose {
                let debug_msg = event.msg.unwrap_or_else(|| "(no message)".to_string());
                let _ = event_tx.send(ObserverEvent::Debug(format!("js: {debug_msg}")));
            }
        }
        "chat" => {
            let mut ts = event.ts.unwrap_or_else(now_iso);
            let mut user = event.user.unwrap_or_default();
            let mut msg = event.msg.unwrap_or_default();
            let role = parse_author_role(event.role.as_deref());

            ts = sanitize_field(&ts);
            user = sanitize_field(&user);
            msg = sanitize_field(&msg);

            if ts.is_empty() {
                ts = now_iso();
            }

            if user.is_empty() || msg.is_empty() {
                return;
            }

            if msg.chars().count() > MAX_MESSAGE_LEN {
                msg = truncate_message(&msg, MAX_MESSAGE_LEN);
            }

            let badges = event
                .badges
                .unwrap_or_default()
                .into_iter()
                .filter_map(|badge| {
                    let text = badge.text.unwrap_or_default();
                    let text = sanitize_field(&text).trim().to_string();
                    if text.is_empty() {
                        return None;
                    }

                    Some(ChatBadge {
                        text,
                        kind: parse_badge_kind(badge.kind.as_deref()),
                    })
                })
                .collect();

            let _ = event_tx.send(ObserverEvent::Chat(ChatLine {
                ts,
                user,
                msg,
                role,
                badges,
            }));
        }
        "send_ok" => {
            let _ = event_tx.send(ObserverEvent::MessageSent);
        }
        "send_error" => {
            let msg = event
                .msg
                .unwrap_or_else(|| "failed to send message from webview".to_string());
            let _ = event_tx.send(ObserverEvent::SendError(msg));
        }
        _ => {
            if verbose {
                let _ = event_tx.send(ObserverEvent::Debug(format!(
                    "unknown IPC event kind: {}",
                    event.kind
                )));
            }
        }
    }
}

fn build_send_message_script(message: &str) -> String {
    let encoded_message = serde_json::to_string(message).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(() => {{
  const text = {encoded_message};
  try {{
    if (typeof window.__ytchatSendText === 'function') {{
      window.__ytchatSendText(text);
      return;
    }}

    if (window.ipc && typeof window.ipc.postMessage === 'function') {{
      window.ipc.postMessage(JSON.stringify({{kind: 'send_error', msg: 'chat input not ready yet'}}));
    }}
  }} catch (err) {{
    if (window.ipc && typeof window.ipc.postMessage === 'function') {{
      window.ipc.postMessage(JSON.stringify({{kind: 'send_error', msg: String(err)}}));
    }}
  }}
}})();"#
    )
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn sanitize_field(input: &str) -> String {
    input
        .replace('\t', " ")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\\n")
}

fn parse_author_role(input: Option<&str>) -> AuthorRole {
    let lowered = input.unwrap_or("regular").to_ascii_lowercase();
    if lowered.contains("owner") {
        AuthorRole::Owner
    } else if lowered.contains("mod") {
        AuthorRole::Moderator
    } else if lowered.contains("member") {
        AuthorRole::Member
    } else {
        AuthorRole::Regular
    }
}

fn parse_badge_kind(input: Option<&str>) -> BadgeKind {
    match input.unwrap_or("other") {
        "owner" => BadgeKind::Owner,
        "moderator" => BadgeKind::Moderator,
        "member" => BadgeKind::Member,
        "rank" => BadgeKind::Rank,
        _ => BadgeKind::Other,
    }
}

fn truncate_message(input: &str, max_len: usize) -> String {
    if input.chars().count() <= max_len {
        return input.to_string();
    }

    let keep = max_len.saturating_sub(3);
    let mut truncated = String::new();

    for (idx, ch) in input.chars().enumerate() {
        if idx >= keep {
            break;
        }
        truncated.push(ch);
    }

    truncated.push_str("...");
    truncated
}

pub fn extract_video_id(input: &str) -> Option<String> {
    let raw = input.trim();

    if is_video_id(raw) {
        return Some(raw.to_string());
    }

    let parsed = Url::parse(raw).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();

    if host == "youtu.be" || host.ends_with(".youtu.be") {
        let candidate = parsed.path().trim_start_matches('/').split('/').next()?;
        return is_video_id(candidate).then(|| candidate.to_string());
    }

    if host == "youtube.com" || host.ends_with(".youtube.com") {
        for (key, value) in parsed.query_pairs() {
            if key == "v" && is_video_id(&value) {
                return Some(value.to_string());
            }
        }

        if parsed.path().starts_with("/live/") {
            let candidate = parsed
                .path()
                .trim_start_matches("/live/")
                .split('/')
                .next()?;
            return is_video_id(candidate).then(|| candidate.to_string());
        }
    }

    None
}

fn is_video_id(s: &str) -> bool {
    s.len() == 11
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

const CHAT_OBSERVER_JS: &str = r#"
(() => {
  const MAX_LEN = 4096;
  const seen = new WeakSet();
  let started = false;
  let badgeDomDebugCount = 0;

  function post(payload) {
    if (window.ipc && typeof window.ipc.postMessage === 'function') {
      window.ipc.postMessage(JSON.stringify(payload));
    }
  }

  function findChatInput() {
    return (
      document.querySelector('yt-live-chat-message-input-renderer yt-live-chat-text-input-field-renderer div#input[contenteditable]') ||
      document.querySelector('yt-live-chat-text-input-field-renderer div#input[contenteditable]') ||
      document.querySelector('yt-live-chat-message-input-renderer #input[contenteditable]') ||
      document.querySelector('div#input[contenteditable]') ||
      document.querySelector('#input[contenteditable]') ||
      document.querySelector('[contenteditable][aria-label*="Chat"]') ||
      document.querySelector('[contenteditable][role="textbox"]')
    );
  }

  function findSendButton() {
    return (
      document.querySelector('yt-live-chat-message-input-renderer #send-button button') ||
      document.querySelector('yt-live-chat-message-input-renderer button[aria-label*="Send"]') ||
      document.querySelector('#send-button button') ||
      document.querySelector('yt-live-chat-message-input-renderer #send-button') ||
      document.querySelector('button[aria-label*="Send"]')
    );
  }

  function setEditableText(input, message) {
    input.focus();

    try {
      const selection = window.getSelection();
      if (selection) {
        selection.removeAllRanges();
        const range = document.createRange();
        range.selectNodeContents(input);
        range.collapse(false);
        selection.addRange(range);
      }
    } catch (_) {}

    let execInserted = false;
    if (typeof document.execCommand === 'function') {
      try {
        document.execCommand('selectAll', false, null);
        execInserted = document.execCommand('insertText', false, message) === true;
      } catch (_) {}
    }

    if (!execInserted || (input.textContent || '').trim() !== message) {
      input.textContent = message;
    }

    try {
      input.dispatchEvent(new InputEvent('beforeinput', {
        bubbles: true,
        composed: true,
        cancelable: true,
        data: message,
        inputType: 'insertText'
      }));
    } catch (_) {}

    try {
      input.dispatchEvent(new InputEvent('input', {
        bubbles: true,
        composed: true,
        data: message,
        inputType: 'insertText'
      }));
    } catch (_) {
      input.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
    }

    input.dispatchEvent(new KeyboardEvent('keyup', {
      bubbles: true,
      composed: true,
      key: 'a'
    }));

    input.dispatchEvent(new Event('change', { bubbles: true, composed: true }));
  }

  function pressEnter(input) {
    const keyboardOpts = {
      bubbles: true,
      composed: true,
      cancelable: true,
      key: 'Enter',
      code: 'Enter',
      which: 13,
      keyCode: 13
    };
    input.dispatchEvent(new KeyboardEvent('keydown', keyboardOpts));
    input.dispatchEvent(new KeyboardEvent('keypress', keyboardOpts));
    input.dispatchEvent(new KeyboardEvent('keyup', keyboardOpts));
  }

  function classifyBadgeKind(text) {
    const lower = (text || '').toLowerCase();
    if (lower.includes('owner')) return 'owner';
    if (lower.includes('moderator') || lower.includes('mod')) return 'moderator';
    if (lower.includes('member')) return 'member';
    if (
      /#\s*\d+/.test(lower) ||
      /\blevel\s*\d+\b/.test(lower) ||
      /\bxp\b/.test(lower) ||
      lower.includes('rank') ||
      lower.includes('crown') ||
      lower.includes('tier')
    ) {
      return 'rank';
    }
    return 'other';
  }

  function extractBadges(renderer, user) {
    const badges = [];
    const seen = new Set();
    const badgeRoot =
      renderer.querySelector('#chat-badges') ||
      renderer.querySelector('yt-live-chat-author-chip #chat-badges') ||
      renderer.querySelector('yt-live-chat-author-chip');
    const beforeContentButtons =
      renderer.querySelector('#before-content-buttons') ||
      renderer.querySelector('div#before-content-buttons');

    const badgeRoots = [
      beforeContentButtons,
      badgeRoot,
      renderer.querySelector('#before-content'),
      renderer.querySelector('yt-live-chat-author-chip'),
      renderer.querySelector('#chip-badges')
    ].filter(Boolean);

    const skipTexts = new Set([
      (user || '').toLowerCase(),
      (`@${user || ''}`).toLowerCase()
    ]);

    for (const root of badgeRoots) {
      const badgeNodes = root.querySelectorAll(
        'yt-live-chat-author-badge-renderer, yt-button-view-model, button-view-model, button[aria-label], .yt-spec-button-shape-next__button-text-content, [aria-label], [title], img[alt], img[aria-label], tp-yt-paper-tooltip, title, span, yt-icon'
      );

      for (const badgeNode of badgeNodes) {
        let text =
          badgeNode.getAttribute('aria-label') ||
          badgeNode.getAttribute('title') ||
          badgeNode.getAttribute('alt') ||
          badgeNode.textContent ||
          '';

        if (!text) {
          const img = badgeNode.querySelector('img');
          if (img) {
            text = img.getAttribute('alt') || img.getAttribute('aria-label') || '';
          }
        }

        text = text.replace(/\s+/g, ' ').trim();
        if (!text) continue;
        if (text.length > 64) continue;
        if (!/[#\w]/.test(text)) continue;

        const lower = text.toLowerCase();
        if (skipTexts.has(lower)) continue;

        const dedupeKey = lower;
        if (seen.has(dedupeKey)) continue;
        seen.add(dedupeKey);

        badges.push({ text, kind: classifyBadgeKind(text) });
      }
    }

    return badges;
  }

  function maybeDebugBadgeDom(renderer, user, badges) {
    if (badges.length > 0 || badgeDomDebugCount >= 8) {
      return;
    }

    badgeDomDebugCount += 1;

    const chip = renderer.querySelector('yt-live-chat-author-chip');
    const chatBadges = renderer.querySelector('#chat-badges');
    const beforeContent = renderer.querySelector('#before-content');
    const compact = (el) => {
      if (!el) return 'none';
      return (el.outerHTML || '')
        .replace(/\s+/g, ' ')
        .slice(0, 320);
    };

    post({
      kind: 'debug',
      msg: `badge_dom user=${user} chip=${compact(chip)} chatBadges=${compact(chatBadges)} beforeContent=${compact(beforeContent)}`
    });
  }

  function inferRole(renderer, badges, authorEl) {
    const attr = (renderer.getAttribute('author-type') || '').toLowerCase();
    if (attr.includes('owner')) return 'owner';
    if (attr.includes('moderator') || attr.includes('mod')) return 'moderator';
    if (attr.includes('member')) return 'member';

    const authorTypeAttr = (authorEl && (authorEl.getAttribute('type') || authorEl.getAttribute('author-type')) || '').toLowerCase();
    if (authorTypeAttr.includes('owner')) return 'owner';
    if (authorTypeAttr.includes('moderator') || authorTypeAttr.includes('mod')) return 'moderator';
    if (authorTypeAttr.includes('member')) return 'member';

    const authorClass = (authorEl && authorEl.className ? String(authorEl.className) : '').toLowerCase();
    if (authorClass.includes('owner')) return 'owner';
    if (authorClass.includes('moderator') || authorClass.includes('mod')) return 'moderator';
    if (authorClass.includes('member')) return 'member';

    for (const badge of badges) {
      if (badge.kind === 'owner') return 'owner';
      if (badge.kind === 'moderator') return 'moderator';
      if (badge.kind === 'member') return 'member';
    }

    return 'regular';
  }

  function sendChatMessage(text) {
    const message = (text || '').replace(/\r\n/g, '\n').replace(/\r/g, '\n').trim();
    if (!message) {
      post({ kind: 'send_error', msg: 'empty message' });
      return;
    }

    let attempts = 0;
    const maxAttempts = 25;

    const trySend = () => {
      const input = findChatInput();
      const sendButton = findSendButton();

      if (!input) {
        attempts += 1;
        if (attempts < maxAttempts) {
          setTimeout(trySend, 200);
          return;
        }

        if (document.querySelector('yt-live-chat-message-input-renderer #author-photo')) {
          post({ kind: 'send_error', msg: 'chat input not ready yet' });
        } else {
          post({ kind: 'send_error', msg: 'chat input unavailable: sign in required. Run once with --headless=false to log in, then reuse profile dir.' });
        }
        return;
      }

      setEditableText(input, message);

      if (sendButton) {
        const disabled =
          sendButton.disabled ||
          sendButton.getAttribute('aria-disabled') === 'true';

        if (!disabled) {
          sendButton.click();
          post({ kind: 'send_ok' });
          return;
        }
      }

      pressEnter(input);

      setTimeout(() => {
        const currentText = (input.textContent || '').trim();
        if (!currentText || currentText !== message) {
          post({ kind: 'send_ok' });
        } else {
          post({ kind: 'send_error', msg: 'send action did not clear input; message may not have sent' });
        }
      }, 180);
    };

    trySend();
  }

  window.__ytchatSendText = sendChatMessage;

  function textFromNode(node) {
    if (!node) return '';

    if (node.nodeType === Node.TEXT_NODE) {
      return node.nodeValue || '';
    }

    if (node.nodeType !== Node.ELEMENT_NODE) {
      return '';
    }

    const tag = node.tagName ? node.tagName.toLowerCase() : '';
    if (tag === 'img') {
      return node.getAttribute('alt') || '';
    }

    if (tag === 'br') {
      return '\n';
    }

    let text = '';
    for (const child of node.childNodes) {
      text += textFromNode(child);
    }
    return text;
  }

  function extract(renderer) {
    const authorEl = renderer.querySelector('#author-name');
    const tsEl = renderer.querySelector('#timestamp');
    const messageEl = renderer.querySelector('#message');
    if (!authorEl || !messageEl) return null;

    const user = (authorEl.textContent || '').trim();
    let msg = textFromNode(messageEl)
      .replace(/\r\n/g, '\n')
      .replace(/\r/g, '\n')
      .trim();

    if (!user || !msg) return null;

    if (msg.length > MAX_LEN) {
      msg = msg.slice(0, MAX_LEN - 3) + '...';
    }

    const badges = extractBadges(renderer, user);
    maybeDebugBadgeDom(renderer, user, badges);
    const role = inferRole(renderer, badges, authorEl);

    const renderedTs = tsEl ? (tsEl.textContent || '').trim() : '';
    let ts = new Date().toISOString();
    if (renderedTs) {
      const maybeDate = new Date(renderedTs);
      if (!Number.isNaN(maybeDate.valueOf())) {
        ts = maybeDate.toISOString();
      }
    }

    return {
      ts,
      user,
      msg,
      role,
      badges,
    };
  }

  function emit(renderer) {
    if (!(renderer instanceof Element)) return;
    if (seen.has(renderer)) return;

    seen.add(renderer);
    const payload = extract(renderer);
    if (!payload) return;
    post({ kind: 'chat', ...payload });
  }

  function processAdded(node) {
    if (!(node instanceof Element)) return;

    if (node.matches('yt-live-chat-text-message-renderer')) {
      emit(node);
    }

    const nested = node.querySelectorAll('yt-live-chat-text-message-renderer');
    for (const renderer of nested) {
      emit(renderer);
    }
  }

  function findMessageContainer() {
    return (
      document.querySelector('#items.yt-live-chat-item-list-renderer') ||
      document.querySelector('yt-live-chat-item-list-renderer #items')
    );
  }

  function startObserver(container) {
    if (started) return;
    started = true;

    const existing = container.querySelectorAll('yt-live-chat-text-message-renderer');
    for (const renderer of existing) {
      emit(renderer);
    }

    post({ kind: 'ready' });

    const observer = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
        for (const added of mutation.addedNodes) {
          processAdded(added);
        }
      }
    });

    observer.observe(container, { childList: true, subtree: true });
  }

  function tryStart() {
    const app = document.querySelector('yt-live-chat-app');
    if (!app) return false;

    const container = findMessageContainer();
    if (!container) return false;

    startObserver(container);
    return true;
  }

  if (tryStart()) {
    return;
  }

  let attempts = 0;
  const timer = setInterval(() => {
    attempts += 1;

    if (tryStart()) {
      clearInterval(timer);
      return;
    }

    if (attempts % 20 === 0) {
      post({ kind: 'debug', msg: 'waiting_for_chat_dom' });
    }
  }, 250);
})();
"#;

#[cfg(test)]
mod tests {
    use super::extract_video_id;

    #[test]
    fn parses_watch_url() {
        assert_eq!(
            extract_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn parses_short_url() {
        assert_eq!(
            extract_video_id("https://youtu.be/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn parses_live_url() {
        assert_eq!(
            extract_video_id("https://www.youtube.com/live/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn parses_live_chat_popout_url() {
        assert_eq!(
            extract_video_id("https://www.youtube.com/live_chat?is_popout=1&v=2tDS9uEpqCI"),
            Some("2tDS9uEpqCI".to_string())
        );
    }

    #[test]
    fn accepts_raw_video_id() {
        assert_eq!(
            extract_video_id("dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }
}
