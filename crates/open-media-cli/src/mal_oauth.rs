//! MyAnimeList OAuth2 token client (PKCE authorization-code grant + refresh).
//!
//! MAL quirks this module encodes:
//! - PKCE supports **only** `code_challenge_method=plain`, so the challenge *is*
//!   the verifier (no S256).
//! - Access tokens are short-lived (~31 days) and come with a refresh token;
//!   both are rotated on refresh, so callers must persist the new pair.
//! - Public app types ("other"/"android"/"ios") have no client secret; "web"
//!   apps do. The secret is sent only when configured.
//!
//! Only the HTTP token exchange lives here (base URL injectable for tests); the
//! interactive loopback flow is in `login.rs`, and persistence in the caller.

use anyhow::{Context, Result};

/// Production token endpoint.
pub const DEFAULT_TOKEN_URL: &str = "https://myanimelist.net/v1/oauth2/token";

/// The token pair MAL returns from both grant types.
#[derive(Debug)]
pub struct MalTokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Lifetime of `access_token` in seconds from now.
    pub expires_in: i64,
}

/// Exchange an authorization code (+ PKCE verifier) for tokens.
pub async fn exchange_code(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<MalTokens> {
    let mut form = vec![
        ("client_id", client_id),
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", redirect_uri),
    ];
    if !client_secret.is_empty() {
        form.push(("client_secret", client_secret));
    }
    request_tokens(token_url, &form).await
}

/// Trade a refresh token for a fresh token pair. MAL rotates the refresh token
/// too — persist both from the result.
pub async fn refresh(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<MalTokens> {
    let mut form = vec![
        ("client_id", client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    if !client_secret.is_empty() {
        form.push(("client_secret", client_secret));
    }
    request_tokens(token_url, &form).await
}

async fn request_tokens(token_url: &str, form: &[(&str, &str)]) -> Result<MalTokens> {
    let resp = open_media_net::client()
        .post(token_url)
        .form(form)
        .send()
        .await
        .context("failed to reach the MyAnimeList token endpoint")?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .context("failed reading the MyAnimeList token response")?;

    if !status.is_success() {
        // MAL errors are JSON like {"error":"invalid_grant","message":"..."} —
        // surface the useful part, never any token material.
        let detail = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                let err = v.get("error")?.as_str()?.to_string();
                let msg = v
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or_default();
                Some(if msg.is_empty() {
                    err
                } else {
                    format!("{err}: {msg}")
                })
            })
            .unwrap_or_else(|| format!("HTTP {status}"));
        anyhow::bail!("MyAnimeList rejected the token request ({detail})");
    }

    parse_tokens(&body)
}

/// Parse the token JSON. Split out for unit testing.
fn parse_tokens(body: &str) -> Result<MalTokens> {
    let v: serde_json::Value =
        serde_json::from_str(body).context("MyAnimeList token response was not valid JSON")?;
    let access_token = v
        .get("access_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .context("MyAnimeList token response had no access_token")?
        .to_string();
    let refresh_token = v
        .get("refresh_token")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string();
    let expires_in = v.get("expires_in").and_then(|e| e.as_i64()).unwrap_or(0);
    Ok(MalTokens {
        access_token,
        refresh_token,
        expires_in,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_token_response() {
        let body = r#"{
            "token_type": "Bearer",
            "expires_in": 2678400,
            "access_token": "acc",
            "refresh_token": "ref"
        }"#;
        let t = parse_tokens(body).unwrap();
        assert_eq!(t.access_token, "acc");
        assert_eq!(t.refresh_token, "ref");
        assert_eq!(t.expires_in, 2_678_400);
    }

    #[test]
    fn missing_access_token_is_an_error() {
        assert!(parse_tokens(r#"{"refresh_token":"r"}"#).is_err());
        assert!(parse_tokens(r#"{"access_token":""}"#).is_err());
        assert!(parse_tokens("not json").is_err());
    }

    #[test]
    fn missing_optional_fields_default() {
        let t = parse_tokens(r#"{"access_token":"acc"}"#).unwrap();
        assert_eq!(t.refresh_token, "");
        assert_eq!(t.expires_in, 0);
    }

    mod e2e {
        use super::super::*;
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        #[tokio::test]
        async fn exchanges_code_with_pkce_form_fields() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/token"))
                .and(body_string_contains("grant_type=authorization_code"))
                .and(body_string_contains("client_id=cid"))
                .and(body_string_contains("code=the-code"))
                .and(body_string_contains("code_verifier=the-verifier"))
                .and(body_string_contains(
                    "redirect_uri=http%3A%2F%2Flocalhost%3A42069%2Fcallback",
                ))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "token_type": "Bearer",
                    "expires_in": 2678400,
                    "access_token": "acc",
                    "refresh_token": "ref"
                })))
                .expect(1)
                .mount(&server)
                .await;

            let tokens = exchange_code(
                &format!("{}/token", server.uri()),
                "cid",
                "", // public app: no client_secret field at all
                "the-code",
                "the-verifier",
                "http://localhost:42069/callback",
            )
            .await
            .unwrap();
            assert_eq!(tokens.access_token, "acc");
            assert_eq!(tokens.refresh_token, "ref");
            assert_eq!(tokens.expires_in, 2_678_400);
        }

        #[tokio::test]
        async fn refresh_rotates_pair_and_sends_secret_when_configured() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/token"))
                .and(body_string_contains("grant_type=refresh_token"))
                .and(body_string_contains("refresh_token=old-ref"))
                .and(body_string_contains("client_secret=sec"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "token_type": "Bearer",
                    "expires_in": 2678400,
                    "access_token": "acc2",
                    "refresh_token": "ref2"
                })))
                .expect(1)
                .mount(&server)
                .await;

            let tokens = refresh(&format!("{}/token", server.uri()), "cid", "sec", "old-ref")
                .await
                .unwrap();
            assert_eq!(tokens.access_token, "acc2");
            assert_eq!(tokens.refresh_token, "ref2");
        }

        #[tokio::test]
        async fn error_response_surfaces_mal_message_without_tokens() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/token"))
                .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                    "error": "invalid_grant",
                    "message": "Invalid refresh token."
                })))
                .mount(&server)
                .await;

            let err = refresh(&format!("{}/token", server.uri()), "cid", "", "dead")
                .await
                .unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("invalid_grant"));
            assert!(msg.contains("Invalid refresh token."));
            assert!(!msg.contains("dead"), "no token material in errors");
        }
    }
}
