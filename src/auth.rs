use anyhow::Result;
use google_youtube3::{
    hyper_rustls::{self, HttpsConnector},
    hyper_util::{
        client::legacy::{connect::HttpConnector, Client},
        rt::TokioExecutor,
    },
    yup_oauth2::{ApplicationSecret, InstalledFlowAuthenticator, InstalledFlowReturnMethod},
    YouTube,
};
use std::path::PathBuf;

pub type YouTubeClient = YouTube<HttpsConnector<HttpConnector>>;

/// Create an authenticated YouTube client
pub async fn create_youtube_client() -> Result<YouTubeClient> {
    // Load client secret from environment or config file
    let secret = load_client_secret()?;

    // Get token cache path
    let token_path = get_token_cache_path()?;

    // Ensure parent directory exists
    if let Some(parent) = token_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Build authenticator with token persistence
    let auth = InstalledFlowAuthenticator::builder(secret, InstalledFlowReturnMethod::HTTPRedirect)
        .persist_tokens_to_disk(&token_path)
        .build()
        .await?;

    // Create HTTPS client
    let client = Client::builder(TokioExecutor::new()).build(
        hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()?
            .https_only()
            .enable_http2()
            .build(),
    );

    // Create YouTube hub
    Ok(YouTube::new(client, auth))
}

fn load_client_secret() -> Result<ApplicationSecret> {
    // Try environment variables first
    if let (Ok(client_id), Ok(client_secret)) = (
        std::env::var("YOUTUBE_CLIENT_ID"),
        std::env::var("YOUTUBE_CLIENT_SECRET"),
    ) {
        return Ok(ApplicationSecret {
            client_id,
            client_secret,
            auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
            token_uri: "https://oauth2.googleapis.com/token".to_string(),
            redirect_uris: vec!["http://localhost".to_string()],
            ..Default::default()
        });
    }

    // Try config file
    let config_path = get_config_dir()?.join("client_secret.json");
    if config_path.exists() {
        let contents = std::fs::read_to_string(&config_path)?;

        // Google's client_secret.json has a nested structure like:
        // {"installed": {"client_id": ..., "client_secret": ..., ...}}
        // or {"web": {...}}
        let json: serde_json::Value = serde_json::from_str(&contents)?;

        let secret_obj = json
            .get("installed")
            .or_else(|| json.get("web"))
            .ok_or_else(|| anyhow::anyhow!(
                "client_secret.json must contain 'installed' or 'web' key"
            ))?;

        let secret: ApplicationSecret = serde_json::from_value(secret_obj.clone())?;
        return Ok(secret);
    }

    Err(anyhow::anyhow!(
        "No YouTube API credentials found.\n\
        Either set YOUTUBE_CLIENT_ID and YOUTUBE_CLIENT_SECRET environment variables,\n\
        or place client_secret.json in {:?}",
        get_config_dir()?
    ))
}

fn get_config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|d| d.join("streamchat"))
        .ok_or_else(|| anyhow::anyhow!("Cannot find config directory"))
}

fn get_token_cache_path() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("token_cache.json"))
}
