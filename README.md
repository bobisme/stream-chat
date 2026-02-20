# streamchat

Terminal YouTube live chat client powered by an embedded WebView (no YouTube Data API key required).

## What it does

- Reads live chat messages from YouTube chat popout DOM.
- Renders chat in a terminal UI.
- Supports sending chat messages through the WebView when signed in.
- Supports mention autocomplete from recently seen usernames.

## Binaries

- `streamchat` (root crate): interactive terminal chat UI.
- `ytchat-stdout` (`crates/ytchat-webview`): line-oriented stdout output.

## Build

```bash
cargo build
```

## Run `streamchat`

```bash
cargo run -- "https://www.youtube.com/watch?v=<VIDEO_ID>"
```

Accepted inputs include:

- `https://www.youtube.com/watch?v=VIDEOID`
- `https://youtu.be/VIDEOID`
- `https://www.youtube.com/live/VIDEOID`
- `https://www.youtube.com/live_chat?is_popout=1&v=VIDEOID`
- `VIDEOID`

### Helpful flags

- `--headless[=true|false]` (default true)
- `--timeout <secs>`
- `--profile-dir <path>`
- `--ephemeral`
- `--verbose`
- `--debug-log <path>`

## First-time sign in (for sending)

If sending fails because the chat input is unavailable, run once with a visible window:

```bash
cargo run -- --headless=false "https://www.youtube.com/watch?v=<VIDEO_ID>"
```

Sign in in that window, then subsequent runs can be headless and reuse the saved session profile.

Default profile path on Linux:

- `~/.config/streamchat/webview-profile`

## UI controls

- `Ctrl+C`: quit
- `Ctrl+J` / `Ctrl+K`: scroll newer/older chat
- `Tab`: cycle mention autocomplete
- `Space`: confirm selected mention
- `Esc`: cancel mention autocomplete
- `Enter`: send message
- `Shift+Enter` or `Ctrl+Enter`: newline in input

Input is limited to 200 characters to match YouTube chat constraints.

## Run `ytchat-stdout`

```bash
cargo run --bin ytchat-stdout --manifest-path crates/ytchat-webview/Cargo.toml -- "https://www.youtube.com/watch?v=<VIDEO_ID>"
```

Default output is TSV:

```text
<ISO-8601 timestamp>\t<username>\t<message>
```

Use `--json` for JSON lines.
