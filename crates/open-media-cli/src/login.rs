//! `open-media login <provider>` — loopback OAuth to obtain and persist a tracker token.
//!
//! AniList uses the OAuth 2.0 **implicit grant**: there is no client secret and
//! no token-exchange step. The authorization server redirects back with the
//! access token in the URL **fragment** (`#access_token=…`). Fragments are never
//! sent to the server, so our loopback listener can't read it from the HTTP
//! request line directly. We bridge it: the `/callback` route serves a tiny
//! HTML+JS page that reads `window.location.hash` in the browser and re-requests
//! `/callback?access_token=…` (a query, which *is* sent to the server). The
//! listener then captures the token from the query string and shuts down.
//!
//! MyAnimeList uses the OAuth 2.0 **authorization-code grant with PKCE**
//! (`plain` challenge only — a MAL quirk). The code arrives directly in the
//! callback's query string (no fragment bridge needed) and is exchanged for an
//! access + refresh token pair via `mal_oauth`. Because MAL requires the
//! redirect URI to exactly match the one registered on the user's own API
//! client, `login mal` needs `mal_client_id` configured first and tells the
//! user how to register one otherwise.
//!
//! No secret is ever logged: tokens are read into config and saved; only a
//! success message is printed.

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// AniList OAuth client id. This is **public**, not a secret: implicit-grant
/// clients have no secret, and the id is embedded in the authorize URL the user
/// visits. Shipped as a default; can be made overridable later.
const ANILIST_CLIENT_ID: &str = "44561";

/// Fixed loopback port. AniList requires the redirect URI to match the one
/// registered for the client **exactly**, so this is not negotiable per-run.
const LOOPBACK_PORT: u16 = 42069;

/// The redirect URI registered with the AniList client. Must match
/// [`LOOPBACK_PORT`] and the AniList app settings byte-for-byte.
const REDIRECT_URI: &str = "http://localhost:42069/callback";

/// Dispatch `open-media login <provider>`. Validates the provider so the surface is
/// future-proof: unknown providers get an actionable message rather than a panic.
pub async fn cmd_login(provider: &str) -> Result<()> {
    match provider.to_ascii_lowercase().as_str() {
        "anilist" => login_anilist().await,
        "mal" | "myanimelist" => login_mal().await,
        other => anyhow::bail!("unknown tracker `{other}` — supported: anilist, mal"),
    }
}

/// Run the AniList implicit-grant flow end to end: bind → prompt → capture →
/// persist.
async fn login_anilist() -> Result<()> {
    // Bind BEFORE printing/opening the URL so there is no race where the browser
    // redirects back before we're listening.
    let listener = bind_loopback().await?;

    let auth_url = anilist_authorize_url(ANILIST_CLIENT_ID, REDIRECT_URI);

    println!("Opening AniList authorization in your browser…");
    println!();
    println!("If it doesn't open automatically, open this URL:");
    println!("  {auth_url}");
    println!();
    println!("Waiting for the callback on {REDIRECT_URI} …");

    // Best-effort browser open. We do NOT add a dependency for this; `xdg-open`
    // (Linux) / `open` (macOS) are ubiquitous and failure is non-fatal — the URL
    // is already printed for manual use.
    try_open_browser(&auth_url);

    let token = capture_token(listener).await?;

    let mut cfg = open_media_config::load().unwrap_or_default();
    cfg.credentials.anilist_token = token;
    open_media_config::save(&cfg).context("failed to save AniList token to config")?;

    println!();
    println!("AniList token saved.");
    Ok(())
}

/// MAL OAuth2 endpoints (PKCE authorization-code grant).
const MAL_AUTHORIZE_URL: &str = "https://myanimelist.net/v1/oauth2/authorize";

/// Refresh the MAL access token this far before it actually expires. Tokens
/// last ~31 days; a 7-day margin means any regular usage keeps them fresh.
const MAL_REFRESH_MARGIN_SECS: i64 = 7 * 24 * 3600;

/// Run the MAL PKCE authorization-code flow end to end: bind → prompt →
/// capture code → exchange → persist access + refresh tokens.
async fn login_mal() -> Result<()> {
    let mut cfg = open_media_config::load().unwrap_or_default();
    let client_id = cfg.credentials.mal_client_id.clone();
    if client_id.is_empty() {
        anyhow::bail!(
            "MyAnimeList login needs an API client id.\n\
             \n\
             1. Create one at https://myanimelist.net/apiconfig (App Type: `other`)\n\
             2. Set its \"App Redirect URL\" to exactly: {REDIRECT_URI}\n\
             3. Save the id: open-media config set mal_client_id=<your client id>\n\
             \n\
             then run `open-media login mal` again. (If you registered a `web` app,\n\
             also set mal_client_secret.)"
        );
    }

    // MAL only supports the `plain` PKCE method: the challenge IS the verifier.
    let verifier = pkce_verifier();
    let state = uuid::Uuid::new_v4().simple().to_string();

    // Bind BEFORE printing/opening the URL so there is no race where the browser
    // redirects back before we're listening.
    let listener = bind_loopback().await?;

    let auth_url = mal_authorize_url(&client_id, REDIRECT_URI, &verifier, &state);

    println!("Opening MyAnimeList authorization in your browser…");
    println!();
    println!("If it doesn't open automatically, open this URL:");
    println!("  {auth_url}");
    println!();
    println!("Waiting for the callback on {REDIRECT_URI} …");

    try_open_browser(&auth_url);

    let code = capture_mal_code(listener, &state).await?;

    let tokens = crate::mal_oauth::exchange_code(
        crate::mal_oauth::DEFAULT_TOKEN_URL,
        &client_id,
        &cfg.credentials.mal_client_secret,
        &code,
        &verifier,
        REDIRECT_URI,
    )
    .await?;

    apply_mal_tokens(&mut cfg, tokens);
    open_media_config::save(&cfg).context("failed to save MyAnimeList tokens to config")?;

    println!();
    println!("MyAnimeList tokens saved (auto-refresh enabled).");
    Ok(())
}

/// Refresh the persisted MAL access token when it is close to expiry.
/// Best-effort and silent on the happy path: called before engine composition so
/// a stale token never reaches the tracker. Failures only warn — playback must
/// never be blocked by tracking.
pub async fn refresh_mal_if_needed(cfg: &mut open_media_config::Config) {
    if !should_refresh(
        &cfg.credentials.mal_token,
        &cfg.credentials.mal_refresh_token,
        cfg.credentials.mal_token_expires_at,
        unix_now(),
    ) {
        return;
    }
    match crate::mal_oauth::refresh(
        crate::mal_oauth::DEFAULT_TOKEN_URL,
        &cfg.credentials.mal_client_id,
        &cfg.credentials.mal_client_secret,
        &cfg.credentials.mal_refresh_token.clone(),
    )
    .await
    {
        Ok(tokens) => {
            apply_mal_tokens(cfg, tokens);
            if let Err(e) = open_media_config::save(cfg) {
                tracing::warn!(error = %e, "refreshed MAL token could not be persisted");
            }
        }
        Err(e) => {
            // The old token may still work (we refresh 7 days early); if it is
            // truly dead the tracker degrades gracefully. Tell the user how to fix.
            eprintln!("warning: MyAnimeList token refresh failed ({e}); run `open-media login mal` if tracking stops");
        }
    }
}

/// Whether the MAL access token should be refreshed now. Requires a token to
/// refresh, a refresh token to do it with, and a *known* expiry (0 = manually
/// provisioned → never auto-refresh) inside the margin.
fn should_refresh(access_token: &str, refresh_token: &str, expires_at: i64, now: i64) -> bool {
    !access_token.is_empty()
        && !refresh_token.is_empty()
        && expires_at > 0
        && expires_at - now <= MAL_REFRESH_MARGIN_SECS
}

/// Store a fresh token pair + computed expiry on the config (not yet saved).
fn apply_mal_tokens(cfg: &mut open_media_config::Config, tokens: crate::mal_oauth::MalTokens) {
    cfg.credentials.mal_token = tokens.access_token;
    if !tokens.refresh_token.is_empty() {
        cfg.credentials.mal_refresh_token = tokens.refresh_token;
    }
    cfg.credentials.mal_token_expires_at = if tokens.expires_in > 0 {
        unix_now() + tokens.expires_in
    } else {
        0
    };
}

/// A PKCE code verifier: 64 chars from `[a-f0-9]`, well inside the RFC 7636
/// 43–128 unreserved-character window. MAL's `plain` method sends it verbatim
/// as the challenge.
fn pkce_verifier() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Build the MAL authorize URL (PKCE `plain`: challenge == verifier).
fn mal_authorize_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    format!(
        "{MAL_AUTHORIZE_URL}?response_type=code&client_id={client_id}&redirect_uri={redirect_uri}&code_challenge={challenge}&code_challenge_method=plain&state={state}"
    )
}

/// Accept connections until the MAL redirect delivers `code` (verifying `state`
/// against CSRF), or fail fast when MAL reports a denial via `error=`.
async fn capture_mal_code(listener: TcpListener, expected_state: &str) -> Result<String> {
    loop {
        let (mut stream, _addr) = listener
            .accept()
            .await
            .context("loopback listener failed while awaiting the OAuth callback")?;

        let Some(target) = read_request_target(&mut stream).await? else {
            continue;
        };

        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
        if let Some(err) = query_param(query, "error") {
            respond(&mut stream, "200 OK", DENIED_PAGE).await?;
            anyhow::bail!("MyAnimeList authorization was not granted ({err})");
        }
        if let Some(code) = query_param(query, "code") {
            if query_param(query, "state").as_deref() != Some(expected_state) {
                respond(&mut stream, "200 OK", DENIED_PAGE).await?;
                anyhow::bail!("OAuth state mismatch on the MyAnimeList callback — try again");
            }
            respond(&mut stream, "200 OK", SUCCESS_PAGE).await?;
            return Ok(code);
        }

        // A stray request (favicon, preconnect): answer and keep waiting.
        respond(&mut stream, "404 Not Found", WAITING_PAGE).await?;
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Bind the one-shot loopback listener, mapping `EADDRINUSE` to an actionable
/// message naming the fixed port.
async fn bind_loopback() -> Result<TcpListener> {
    match TcpListener::bind(("127.0.0.1", LOOPBACK_PORT)).await {
        Ok(l) => Ok(l),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            anyhow::bail!("port {LOOPBACK_PORT} in use; close whatever holds it and retry")
        }
        Err(e) => Err(e).with_context(|| format!("failed to bind 127.0.0.1:{LOOPBACK_PORT}")),
    }
}

/// Accept connections until one delivers `access_token` in its query string.
///
/// Two request shapes arrive at `/callback`:
/// 1. The initial redirect — token is in the (server-invisible) fragment. We
///    answer with [`BRIDGE_PAGE`], whose JS reads the hash and re-requests with
///    the token as a query parameter.
/// 2. The bridge's follow-up — `/callback?access_token=…`. We extract the token,
///    answer with [`SUCCESS_PAGE`], and return.
async fn capture_token(listener: TcpListener) -> Result<String> {
    loop {
        let (mut stream, _addr) = listener
            .accept()
            .await
            .context("loopback listener failed while awaiting the OAuth callback")?;

        let Some(target) = read_request_target(&mut stream).await? else {
            // Couldn't parse a request line (e.g. a probe/preconnect). Ignore it
            // and keep listening rather than aborting the whole login.
            continue;
        };

        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
        if let Some(token) = extract_access_token(query) {
            respond(&mut stream, "200 OK", SUCCESS_PAGE).await?;
            return Ok(token);
        }

        // No token yet → serve the fragment-capture bridge page.
        respond(&mut stream, "200 OK", BRIDGE_PAGE).await?;
    }
}

/// Read just enough of the request to recover the request target (the path +
/// query from the start line: `GET <target> HTTP/1.1`).
async fn read_request_target(stream: &mut TcpStream) -> Result<Option<String>> {
    // A single read of the first segment is enough: the request line is the very
    // first thing on the wire and comfortably fits in 4 KiB.
    let mut buf = [0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(120), stream.read(&mut buf))
        .await
        .context("timed out waiting for the browser callback request")?
        .context("failed reading the callback request")?;
    if n == 0 {
        return Ok(None);
    }
    let head = String::from_utf8_lossy(&buf[..n]);
    Ok(parse_request_target(&head))
}

/// Extract the request target from an HTTP request's start line.
/// `"GET /callback?access_token=x HTTP/1.1\r\n…"` → `"/callback?access_token=x"`.
fn parse_request_target(request: &str) -> Option<String> {
    let line = request.lines().next()?;
    let mut parts = line.split_whitespace();
    let _method = parts.next()?;
    let target = parts.next()?;
    Some(target.to_string())
}

/// Pull `access_token` out of a callback query string (`a=b&access_token=…&c=d`).
///
/// Returns `None` when absent or empty. Other implicit-grant params
/// (`token_type=Bearer`, `expires_in`) are ignored — AniList tokens are
/// long-lived and we persist only the access token.
fn extract_access_token(query: &str) -> Option<String> {
    query_param(query, "access_token")
}

/// Pull one named parameter out of a query string; `None` when absent or empty.
fn query_param(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
        .filter(|v| !v.is_empty())
}

/// Build the AniList implicit-grant authorize URL.
fn anilist_authorize_url(client_id: &str, redirect_uri: &str) -> String {
    format!(
        "https://anilist.co/api/v2/oauth/authorize?client_id={client_id}&redirect_uri={redirect_uri}&response_type=token"
    )
}

/// Best-effort, dependency-free browser launch. Non-fatal on failure.
fn try_open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

/// Write a minimal HTTP/1.1 response with an HTML body and close the connection.
async fn respond(stream: &mut TcpStream, status: &str, body: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len(),
    );
    stream
        .write_all(response.as_bytes())
        .await
        .context("failed writing the callback response")?;
    let _ = stream.flush().await;
    Ok(())
}

/// Served on the initial redirect. Reads the token from the URL fragment (which
/// the server can't see) and re-requests `/callback?access_token=…` so the
/// server can capture it. Falls back to a clear message if the fragment is
/// missing.
const BRIDGE_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>open-media · finishing login</title></head>
<body style="font-family:system-ui,sans-serif;text-align:center;margin-top:4rem">
<h1>Finishing sign-in…</h1>
<p id="msg">One moment.</p>
<script>
  // The implicit-grant token arrives in the URL fragment (#access_token=...),
  // which is invisible to the server. Re-issue it as a query so the loopback
  // server can read it.
  var hash = window.location.hash || "";
  var params = new URLSearchParams(hash.replace(/^#/, ""));
  var token = params.get("access_token");
  if (token) {
    window.location.replace("/callback?access_token=" + encodeURIComponent(token));
  } else {
    document.getElementById("msg").textContent =
      "No access token found in the callback. You can close this tab and try again.";
  }
</script>
</body></html>"#;

/// Served once the token/code is captured.
const SUCCESS_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>open-media · signed in</title></head>
<body style="font-family:system-ui,sans-serif;text-align:center;margin-top:4rem">
<h1>You're signed in.</h1>
<p>open-media captured your login. You can close this tab.</p>
</body></html>"#;

/// Served when authorization was denied or the callback was invalid.
const DENIED_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>open-media · sign-in failed</title></head>
<body style="font-family:system-ui,sans-serif;text-align:center;margin-top:4rem">
<h1>Sign-in didn't complete.</h1>
<p>See the terminal for details. You can close this tab.</p>
</body></html>"#;

/// Served to stray requests (favicon, probes) while awaiting the real callback.
const WAITING_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>open-media · waiting</title></head>
<body style="font-family:system-ui,sans-serif;text-align:center;margin-top:4rem">
<p>Waiting for the sign-in callback…</p>
</body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_access_token_when_present() {
        assert_eq!(
            extract_access_token("access_token=abc123"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn extracts_access_token_among_other_params() {
        // AniList may include token_type / expires_in alongside the token.
        let q = "token_type=Bearer&access_token=tok_XYZ&expires_in=31536000";
        assert_eq!(extract_access_token(q), Some("tok_XYZ".to_string()));
    }

    #[test]
    fn missing_or_empty_token_is_none() {
        assert_eq!(extract_access_token(""), None);
        assert_eq!(extract_access_token("foo=bar"), None);
        assert_eq!(extract_access_token("access_token="), None);
    }

    #[test]
    fn parses_request_target_from_start_line() {
        let req = "GET /callback?access_token=x HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(
            parse_request_target(req),
            Some("/callback?access_token=x".to_string())
        );
    }

    #[test]
    fn parses_target_without_query() {
        let req = "GET /callback HTTP/1.1\r\n\r\n";
        assert_eq!(parse_request_target(req), Some("/callback".to_string()));
    }

    #[test]
    fn malformed_request_line_is_none() {
        assert_eq!(parse_request_target(""), None);
        assert_eq!(parse_request_target("GET"), None);
    }

    #[test]
    fn target_query_round_trips_into_token() {
        // End-to-end of the parse path the server uses: start line → target →
        // query → token.
        let req = "GET /callback?token_type=Bearer&access_token=deadbeef HTTP/1.1\r\n\r\n";
        let target = parse_request_target(req).unwrap();
        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
        assert_eq!(extract_access_token(query), Some("deadbeef".to_string()));
    }

    #[test]
    fn query_param_extracts_code_state_and_error() {
        let q = "code=abc&state=xyz";
        assert_eq!(query_param(q, "code"), Some("abc".to_string()));
        assert_eq!(query_param(q, "state"), Some("xyz".to_string()));
        assert_eq!(query_param(q, "error"), None);
        assert_eq!(
            query_param("error=access_denied", "error"),
            Some("access_denied".to_string())
        );
    }

    #[test]
    fn pkce_verifier_is_rfc7636_safe() {
        let v = pkce_verifier();
        // 43–128 chars of unreserved characters (ours are hex digits).
        assert_eq!(v.len(), 64);
        assert!(v.chars().all(|c| c.is_ascii_hexdigit()));
        // Two calls must not collide (it's a secret per login attempt).
        assert_ne!(v, pkce_verifier());
    }

    #[test]
    fn builds_mal_authorize_url_with_plain_pkce() {
        let url = mal_authorize_url("cid", "http://localhost:42069/callback", "ver", "st");
        assert!(url.starts_with("https://myanimelist.net/v1/oauth2/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("code_challenge=ver"));
        assert!(url.contains("code_challenge_method=plain"));
        assert!(url.contains("state=st"));
        assert!(url.contains("redirect_uri=http://localhost:42069/callback"));
    }

    #[test]
    fn refresh_decision_honors_margin_and_prerequisites() {
        let now = 1_000_000;
        let week = 7 * 24 * 3600;
        // Inside the margin → refresh.
        assert!(should_refresh("acc", "ref", now + week - 1, now));
        // Already expired → refresh.
        assert!(should_refresh("acc", "ref", now - 10, now));
        // Comfortably fresh → leave alone.
        assert!(!should_refresh("acc", "ref", now + week + 10, now));
        // Unknown expiry (manually provisioned token) → never auto-refresh.
        assert!(!should_refresh("acc", "ref", 0, now));
        // Nothing to refresh / nothing to refresh with.
        assert!(!should_refresh("", "ref", now, now));
        assert!(!should_refresh("acc", "", now, now));
    }

    #[test]
    fn applying_tokens_computes_expiry_and_keeps_old_refresh_on_empty() {
        let mut cfg = open_media_config::Config::default();
        cfg.credentials.mal_refresh_token = "old-refresh".into();

        apply_mal_tokens(
            &mut cfg,
            crate::mal_oauth::MalTokens {
                access_token: "acc".into(),
                refresh_token: "new-refresh".into(),
                expires_in: 3600,
            },
        );
        assert_eq!(cfg.credentials.mal_token, "acc");
        assert_eq!(cfg.credentials.mal_refresh_token, "new-refresh");
        let expected = unix_now() + 3600;
        assert!((cfg.credentials.mal_token_expires_at - expected).abs() <= 2);

        // An empty refresh token in the response must not clobber the stored one.
        apply_mal_tokens(
            &mut cfg,
            crate::mal_oauth::MalTokens {
                access_token: "acc2".into(),
                refresh_token: String::new(),
                expires_in: 0,
            },
        );
        assert_eq!(cfg.credentials.mal_refresh_token, "new-refresh");
        assert_eq!(cfg.credentials.mal_token_expires_at, 0);
    }

    #[test]
    fn builds_anilist_authorize_url() {
        let url = anilist_authorize_url("44561", "http://localhost:42069/callback");
        assert_eq!(
            url,
            "https://anilist.co/api/v2/oauth/authorize?client_id=44561&redirect_uri=http://localhost:42069/callback&response_type=token"
        );
    }

    #[test]
    fn authorize_url_uses_constants() {
        // Guard the registered values: redirect must match the fixed port, and
        // the client id is the public default.
        let url = anilist_authorize_url(ANILIST_CLIENT_ID, REDIRECT_URI);
        assert!(url.contains("client_id=44561"));
        assert!(url.contains("redirect_uri=http://localhost:42069/callback"));
        assert!(url.contains("response_type=token"));
    }
}
