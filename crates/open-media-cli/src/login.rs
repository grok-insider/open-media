//! `om login <provider>` — loopback OAuth to obtain and persist a tracker token.
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
//! The loopback server + [`cmd_login`] dispatch are written provider-agnostically
//! so a MAL backend (authorization-code grant) can reuse the same scaffolding by
//! adding another match arm and a `run_loopback`-style capture.
//!
//! No secret is ever logged: the token is read into config and saved; only a
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

/// Dispatch `om login <provider>`. Validates the provider so the surface is
/// future-proof: unknown providers (including `mal`) get an actionable message
/// rather than a panic.
pub async fn cmd_login(provider: &str) -> Result<()> {
    match provider.to_ascii_lowercase().as_str() {
        "anilist" => login_anilist().await,
        "mal" | "myanimelist" => {
            anyhow::bail!("mal not yet supported, coming soon")
        }
        other => anyhow::bail!("unknown tracker `{other}` — supported: anilist (mal coming soon)"),
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
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == "access_token")
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

/// Served once the token is captured.
const SUCCESS_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>open-media · signed in</title></head>
<body style="font-family:system-ui,sans-serif;text-align:center;margin-top:4rem">
<h1>You're signed in.</h1>
<p>open-media captured your AniList token. You can close this tab.</p>
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
