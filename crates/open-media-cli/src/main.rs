//! `om` — open-media command-line entrypoint.
//!
//! This binary is intentionally thin: parse args, load config, build the
//! [`Engine`] via [`compose::build_engine`], and dispatch. All real work lives in
//! `om-app` (orchestration) and the adapter crates (I/O).
//!
//! [`Engine`]: open_media_app::Engine

mod compose;
mod login;
mod stills;
mod telemetry;
mod tui;

use clap::{Parser, Subcommand};
use open_media_core::model::MediaKind;

/// open-media: watch movies, series, and anime from the terminal — via
/// Real-Debrid (instant, cached) or direct P2P, into mpv/vlc.
#[derive(Debug, Parser)]
#[command(name = "om", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// First-time setup: create the config file and set required keys.
    Init,
    /// Inspect or modify configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Search for media (movies, series, anime).
    Search {
        /// Free-text query.
        query: String,
        /// Restrict to a kind: movie | series | anime.
        #[arg(long)]
        kind: Option<String>,
    },
    /// Search, pick the best source, and play.
    Play {
        /// Free-text query.
        query: String,
        #[arg(long)]
        season: Option<u32>,
        #[arg(long)]
        episode: Option<u32>,
    },
    /// Authorize a tracker via OAuth and save its token to config.
    Login {
        /// Tracker to authorize: `anilist` (MAL coming soon).
        provider: String,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Print the resolved configuration (secrets masked).
    Show,
    /// Print the config file path.
    Path,
    /// Set a key: `om config set tmdb_api_key=...`.
    Set {
        /// `key=value`. Keys: tmdb_api_key, real_debrid_token, anilist_token,
        /// mal_token, debrid_provider, player_command, telemetry.
        kv: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // The TUI owns the alternate screen, so stderr logs would corrupt it. In TUI
    // mode (no subcommand) logging is silenced unless RUST_LOG is explicitly set.
    init_tracing(cli.command.is_none());

    match cli.command {
        None => run_interactive().await,
        Some(Command::Init) => cmd_init(),
        Some(Command::Config { action }) => cmd_config(action),
        Some(Command::Search { query, kind }) => cmd_search(&query, kind.as_deref()).await,
        Some(Command::Play {
            query,
            season,
            episode,
        }) => cmd_play(&query, season, episode).await,
        Some(Command::Login { provider }) => login::cmd_login(&provider).await,
    }
}

fn init_tracing(tui_mode: bool) {
    use tracing_subscriber::EnvFilter;
    let filter = if tui_mode {
        // Don't write to stderr under the TUI unless the user opted in.
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("off"))
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("om=info,warn"))
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Default (no subcommand): the interactive TUI.
async fn run_interactive() -> anyhow::Result<()> {
    let mut cfg = match open_media_config::load() {
        Ok(c) => c,
        Err(_) => {
            println!("No configuration found. Run `om init` first.");
            return Ok(());
        }
    };
    telemetry::startup(&mut cfg);
    let engine = compose::build_engine(&cfg);
    tui::run(engine, cfg, None).await
}

fn cmd_init() -> anyhow::Result<()> {
    let path = open_media_config::config_path();
    if open_media_config::load().is_ok() {
        println!("Config already exists at {}", path.display());
        println!("Edit it directly, or use `om config set key=value`.");
        return Ok(());
    }
    let mut cfg = open_media_config::Config::default();
    // Mint the stable install id now and mark the telemetry notice as shown, since
    // we print it here — so the first real command does not repeat it.
    cfg.ensure_install_id();
    cfg.telemetry.notified = true;
    open_media_config::save(&cfg)?;
    println!("Created {}", path.display());
    println!();
    println!("Next steps (all optional — search works keyless via Cinemeta + AniList):");
    println!("  om config set real_debrid_token=<your RD token>   # recommended: instant cached playback");
    println!("  om config set tmdb_api_key=<your TMDB v3 key>     # optional: richer movie/series metadata");
    println!();
    println!(
        "Get keys: https://real-debrid.com/apitoken  +  https://www.themoviedb.org/settings/api"
    );
    println!();
    println!("Anonymous usage analytics (OS, arch, version, a random id) are ON by");
    println!("default to count active installs — never anything you watch. Opt out with:");
    println!("  om config set telemetry=false");
    Ok(())
}

fn cmd_config(action: ConfigAction) -> anyhow::Result<()> {
    match action {
        ConfigAction::Path => {
            println!("{}", open_media_config::config_path().display());
            Ok(())
        }
        ConfigAction::Show => {
            let cfg = open_media_config::load()?;
            println!("config: {}", open_media_config::config_path().display());

            // Secrets are masked via mask(); raw tokens are never printed.
            println!("[credentials]");
            println!(
                "  tmdb_api_key          = {}",
                mask(&cfg.credentials.tmdb_api_key)
            );
            println!(
                "  debrid_provider       = {}",
                cfg.credentials.debrid_provider
            );
            println!(
                "  real_debrid_token     = {}",
                mask(&cfg.credentials.real_debrid_token)
            );
            println!(
                "  anilist_token         = {}",
                mask(&cfg.credentials.anilist_token)
            );
            println!(
                "  mal_token             = {}",
                mask(&cfg.credentials.mal_token)
            );

            println!("[providers]");
            println!("  quality               = {}", cfg.providers.quality);
            println!("  show_uncached         = {}", cfg.providers.show_uncached);
            println!("  cinemeta              = {}", cfg.providers.cinemeta);
            println!("  nyaa_direct           = {}", cfg.providers.nyaa_direct);
            println!("  nyaa_category         = {}", cfg.providers.nyaa_category);
            println!(
                "  torrentio_providers   = {}",
                cfg.providers.torrentio_providers.join(", ")
            );

            println!("[player]");
            println!("  player_command        = {}", cfg.player.command);
            println!("  player.args           = {}", cfg.player.args.join(" "));

            println!("[streaming]");
            println!("  http_port             = {}", cfg.streaming.http_port);
            println!(
                "  cleanup_after_playback = {}",
                cfg.streaming.cleanup_after_playback
            );

            println!("[behavior]");
            println!(
                "  skip_intro_outro      = {}",
                cfg.behavior.skip_intro_outro
            );
            println!("  skip_filler           = {}", cfg.behavior.skip_filler);
            println!("  autoplay_next         = {}", cfg.behavior.autoplay_next);
            println!("  resume                = {}", cfg.behavior.resume);
            println!(
                "  complete_threshold    = {}",
                cfg.behavior.complete_threshold
            );
            println!(
                "  discord_presence      = {}",
                cfg.behavior.discord_presence
            );

            println!("[ui]");
            println!("  theme                 = {}", cfg.ui.theme);

            println!("[telemetry]");
            println!("  enabled               = {}", cfg.telemetry.enabled);
            // The install id is not a secret, but it is the analytics identifier;
            // show only its presence, not the raw value.
            println!(
                "  install_id            = {}",
                mask(&cfg.telemetry.install_id)
            );
            Ok(())
        }
        ConfigAction::Set { kv } => {
            let (key, value) = kv
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("expected key=value, got `{kv}`"))?;
            let mut cfg = open_media_config::load().unwrap_or_default();
            match key {
                // Strings — taken verbatim.
                "tmdb_api_key" => cfg.credentials.tmdb_api_key = value.to_string(),
                "real_debrid_token" => cfg.credentials.real_debrid_token = value.to_string(),
                "anilist_token" => cfg.credentials.anilist_token = value.to_string(),
                "mal_token" => cfg.credentials.mal_token = value.to_string(),
                "debrid_provider" => cfg.credentials.debrid_provider = value.to_string(),
                "player_command" => cfg.player.command = value.to_string(),
                "quality" => cfg.providers.quality = value.to_string(),
                "nyaa_category" => cfg.providers.nyaa_category = value.to_string(),
                "theme" => cfg.ui.theme = value.to_string(),
                // Bools — parsed with a clear error on bad input.
                "show_uncached" => cfg.providers.show_uncached = parse_bool(key, value)?,
                "nyaa_direct" => cfg.providers.nyaa_direct = parse_bool(key, value)?,
                "cinemeta" => cfg.providers.cinemeta = parse_bool(key, value)?,
                "skip_intro_outro" => cfg.behavior.skip_intro_outro = parse_bool(key, value)?,
                "skip_filler" => cfg.behavior.skip_filler = parse_bool(key, value)?,
                "autoplay_next" => cfg.behavior.autoplay_next = parse_bool(key, value)?,
                "resume" => cfg.behavior.resume = parse_bool(key, value)?,
                "discord_presence" => cfg.behavior.discord_presence = parse_bool(key, value)?,
                "telemetry" => cfg.telemetry.enabled = parse_bool(key, value)?,
                "cleanup_after_playback" => {
                    cfg.streaming.cleanup_after_playback = parse_bool(key, value)?
                }
                // Numbers — parsed with a clear error on bad input.
                "complete_threshold" => {
                    cfg.behavior.complete_threshold = value.parse::<f32>().map_err(|e| {
                        anyhow::anyhow!("`{key}` expects a number (e.g. 0.85), got `{value}`: {e}")
                    })?
                }
                "http_port" => {
                    cfg.streaming.http_port = value.parse::<u16>().map_err(|e| {
                        anyhow::anyhow!("`{key}` expects a port number 0-65535, got `{value}`: {e}")
                    })?
                }
                other => anyhow::bail!("unknown key `{other}`"),
            }
            open_media_config::save(&cfg)?;
            println!("Updated {key}.");
            Ok(())
        }
    }
}

async fn cmd_search(query: &str, kind: Option<&str>) -> anyhow::Result<()> {
    let mut cfg = load_or_hint()?;
    telemetry::startup(&mut cfg);
    let engine = compose::build_engine(&cfg);
    match engine.search(query, parse_kind(kind)).await {
        Ok(results) if results.is_empty() => {
            println!("No results for “{query}”.");
        }
        Ok(results) => {
            for m in results {
                println!(
                    "[{}] {} ({})",
                    m.kind.label(),
                    m.display_title(),
                    m.year.map(|y| y.to_string()).unwrap_or_else(|| "—".into())
                );
            }
        }
        Err(e) => println!("Search unavailable: {e}"),
    }
    Ok(())
}

async fn cmd_play(query: &str, season: Option<u32>, episode: Option<u32>) -> anyhow::Result<()> {
    use open_media_app::PlayRequest;

    let mut cfg = load_or_hint()?;
    telemetry::startup(&mut cfg);
    let engine = compose::build_engine(&cfg);

    // 1. Search and take the best match.
    let results = engine.search(query, None).await?;
    let top = results
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no results for `{query}`"))?;
    println!("▶ {} ({})", top.display_title(), top.kind.label());

    // 2. Hydrate ids (IMDB) needed by the sources.
    let media = engine.details(&top.ids).await.unwrap_or(top);

    // 3. Resolve coordinates: episodic content defaults to S1E1 when unspecified
    //    (a movie stays None/None).
    let (req_season, req_episode) = if media.kind.is_episodic() {
        (Some(season.unwrap_or(1)), Some(episode.unwrap_or(1)))
    } else {
        (None, None)
    };

    // 4. Best-effort episode title + runtime for the player's media-title and
    //    AniSkip interval validation.
    let (episode_title, episode_runtime_minutes) = match (req_season, req_episode) {
        (Some(s), Some(e)) => engine
            .episodes(&media.ids, s)
            .await
            .ok()
            .and_then(|eps| eps.into_iter().find(|ep| ep.number == e))
            .map(|ep| (ep.title, ep.runtime_minutes))
            .unwrap_or((None, None)),
        _ => (None, None),
    };

    // 5. Find + rank sources.
    let req = PlayRequest {
        media,
        season: req_season,
        episode: req_episode,
        episode_title,
        episode_runtime_minutes,
        include_uncached: cfg.providers.show_uncached,
    };
    let candidates = engine.find_sources(&req).await?;
    let best = candidates
        .into_iter()
        .find(|c| c.is_resolvable())
        .ok_or_else(|| anyhow::anyhow!("no playable source found"))?;
    println!(
        "  source: [{}] {} {} ({}, {})",
        best.provider,
        best.quality.label(),
        best.human_size(),
        match best.cache {
            open_media_core::stream::CacheState::Cached => "cached",
            open_media_core::stream::CacheState::Uncached => "uncached",
            open_media_core::stream::CacheState::Unknown => "unknown",
        },
        best.seeders
            .map(|s| format!("{s} seeders"))
            .unwrap_or_else(|| "?".into())
    );

    // 6. Resolve + play.
    engine.play(&req, &best).await?;
    Ok(())
}

fn load_or_hint() -> anyhow::Result<open_media_config::Config> {
    open_media_config::load().map_err(|_| anyhow::anyhow!("no config found — run `om init` first"))
}

fn parse_kind(kind: Option<&str>) -> Option<MediaKind> {
    match kind?.to_ascii_lowercase().as_str() {
        "movie" => Some(MediaKind::Movie),
        "series" | "tv" => Some(MediaKind::Series),
        "anime" => Some(MediaKind::Anime),
        _ => None,
    }
}

/// Parse a boolean config value, accepting common spellings and returning an
/// actionable error (naming the key) on anything else.
fn parse_bool(key: &str, value: &str) -> anyhow::Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => anyhow::bail!("`{key}` expects a boolean (true/false), got `{other}`"),
    }
}

/// Mask a secret for display: show nothing but presence + length hint.
fn mask(secret: &str) -> String {
    if secret.is_empty() {
        "(not set)".to_string()
    } else {
        format!("set ({} chars)", secret.len())
    }
}
