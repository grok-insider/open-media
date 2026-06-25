//! `om` — open-media command-line entrypoint.
//!
//! This binary is intentionally thin: parse args, load config, build the
//! [`Engine`] via [`compose::build_engine`], and dispatch. All real work lives in
//! `om-app` (orchestration) and the adapter crates (I/O).
//!
//! [`Engine`]: om_app::Engine

mod compose;
mod tui;

use clap::{Parser, Subcommand};
use om_core::model::MediaKind;

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
        /// mal_token, debrid_provider, player_command.
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
    let cfg = match om_config::load() {
        Ok(c) => c,
        Err(_) => {
            println!("No configuration found. Run `om init` first.");
            return Ok(());
        }
    };
    let engine = compose::build_engine(&cfg);
    tui::run(engine, cfg, None).await
}

fn cmd_init() -> anyhow::Result<()> {
    let path = om_config::config_path();
    if om_config::load().is_ok() {
        println!("Config already exists at {}", path.display());
        println!("Edit it directly, or use `om config set key=value`.");
        return Ok(());
    }
    let cfg = om_config::Config::default();
    om_config::save(&cfg)?;
    println!("Created {}", path.display());
    println!();
    println!("Next steps:");
    println!("  om config set tmdb_api_key=<your TMDB v3 key>      # required");
    println!("  om config set real_debrid_token=<your RD token>   # optional, recommended");
    println!();
    println!(
        "Get keys: https://www.themoviedb.org/settings/api  +  https://real-debrid.com/apitoken"
    );
    Ok(())
}

fn cmd_config(action: ConfigAction) -> anyhow::Result<()> {
    match action {
        ConfigAction::Path => {
            println!("{}", om_config::config_path().display());
            Ok(())
        }
        ConfigAction::Show => {
            let cfg = om_config::load()?;
            println!("config: {}", om_config::config_path().display());
            println!(
                "  tmdb_api_key      = {}",
                mask(&cfg.credentials.tmdb_api_key)
            );
            println!("  debrid_provider   = {}", cfg.credentials.debrid_provider);
            println!(
                "  real_debrid_token = {}",
                mask(&cfg.credentials.real_debrid_token)
            );
            println!(
                "  anilist_token     = {}",
                mask(&cfg.credentials.anilist_token)
            );
            println!("  player.command    = {}", cfg.player.command);
            println!("  quality           = {}", cfg.providers.quality);
            println!("  nyaa_direct       = {}", cfg.providers.nyaa_direct);
            println!("  skip_intro_outro  = {}", cfg.behavior.skip_intro_outro);
            Ok(())
        }
        ConfigAction::Set { kv } => {
            let (key, value) = kv
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("expected key=value, got `{kv}`"))?;
            let mut cfg = om_config::load().unwrap_or_default();
            match key {
                "tmdb_api_key" => cfg.credentials.tmdb_api_key = value.to_string(),
                "real_debrid_token" => cfg.credentials.real_debrid_token = value.to_string(),
                "anilist_token" => cfg.credentials.anilist_token = value.to_string(),
                "mal_token" => cfg.credentials.mal_token = value.to_string(),
                "debrid_provider" => cfg.credentials.debrid_provider = value.to_string(),
                "player_command" => cfg.player.command = value.to_string(),
                other => anyhow::bail!("unknown key `{other}`"),
            }
            om_config::save(&cfg)?;
            println!("Updated {key}.");
            Ok(())
        }
    }
}

async fn cmd_search(query: &str, kind: Option<&str>) -> anyhow::Result<()> {
    let cfg = load_or_hint()?;
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
    use om_app::PlayRequest;

    let cfg = load_or_hint()?;
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

    // 4. Best-effort episode title for the player's media-title.
    let episode_title = match (req_season, req_episode) {
        (Some(s), Some(e)) => engine
            .episodes(&media.ids, s)
            .await
            .ok()
            .and_then(|eps| eps.into_iter().find(|ep| ep.number == e))
            .and_then(|ep| ep.title),
        _ => None,
    };

    // 5. Find + rank sources.
    let req = PlayRequest {
        media,
        season: req_season,
        episode: req_episode,
        episode_title,
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
            om_core::stream::CacheState::Cached => "cached",
            om_core::stream::CacheState::Uncached => "uncached",
            om_core::stream::CacheState::Unknown => "unknown",
        },
        best.seeders
            .map(|s| format!("{s} seeders"))
            .unwrap_or_else(|| "?".into())
    );

    // 6. Resolve + play.
    engine.play(&req, &best).await?;
    Ok(())
}

fn load_or_hint() -> anyhow::Result<om_config::Config> {
    om_config::load().map_err(|_| anyhow::anyhow!("no config found — run `om init` first"))
}

fn parse_kind(kind: Option<&str>) -> Option<MediaKind> {
    match kind?.to_ascii_lowercase().as_str() {
        "movie" => Some(MediaKind::Movie),
        "series" | "tv" => Some(MediaKind::Series),
        "anime" => Some(MediaKind::Anime),
        _ => None,
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
