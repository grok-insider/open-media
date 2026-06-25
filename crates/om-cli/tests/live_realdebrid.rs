//! Live integration tests against the real Real-Debrid + Torrentio services.
//!
//! Gated behind `--ignored` and the `OM_RD_TOKEN` env var so they never run in
//! normal/CI test passes (which must stay hermetic). These are read-only: they
//! validate the token and Torrentio's RD-cached detection without adding
//! anything to the account.
//!
//! Run with:
//!   OM_RD_TOKEN=xxxxx cargo test -p om-cli --test live_realdebrid -- --ignored --nocapture

use om_core::model::{IdSet, Media, MediaKind};
use om_core::ports::{DebridProvider, SourceProvider, SourceQuery};
use om_debrid::RealDebrid;
use om_sources::TorrentioSource;

fn token() -> Option<String> {
    std::env::var("OM_RD_TOKEN").ok().filter(|t| !t.is_empty())
}

#[tokio::test]
#[ignore = "live: requires OM_RD_TOKEN"]
async fn real_debrid_account_summary_is_valid() {
    let Some(tok) = token() else {
        eprintln!("OM_RD_TOKEN unset — skipping");
        return;
    };
    let rd = RealDebrid::new(tok);
    let summary = rd.account_summary().await.expect("account summary");
    println!("Real-Debrid account: {summary}");
    assert!(!summary.is_empty());
}

#[tokio::test]
#[ignore = "live: requires OM_RD_TOKEN"]
async fn torrentio_returns_cached_streams_for_a_popular_movie() {
    let Some(tok) = token() else {
        eprintln!("OM_RD_TOKEN unset — skipping");
        return;
    };
    // Interstellar (tt0816692) — almost always cached on Real-Debrid.
    let config = format!(
        "providers=yts,1337x,thepiratebay,torrentgalaxy|sort=qualitysize|qualityfilter=scr,cam|realdebrid={tok}"
    );
    let src = TorrentioSource::new(config);
    let query = SourceQuery {
        media: Media {
            kind: MediaKind::Movie,
            ids: IdSet::default().with_imdb("tt0816692"),
            title: "Interstellar".into(),
            original_title: None,
            year: Some(2014),
            score: None,
            overview: None,
            poster: None,
            genres: vec![],
            status: None,
            episode_count: None,
            season_count: None,
        },
        season: None,
        episode: None,
        include_uncached: false,
    };

    let candidates = src.find(&query).await.expect("torrentio query");
    println!("Torrentio returned {} candidates", candidates.len());
    assert!(!candidates.is_empty(), "expected some streams");

    let cached: Vec<_> = candidates
        .iter()
        .filter(|c| c.direct_url.is_some())
        .collect();
    println!("  {} cached (direct-URL) candidates", cached.len());
    for c in cached.iter().take(3) {
        println!(
            "  - [{}] {} {} -> {}",
            c.provider,
            c.quality.label(),
            c.human_size(),
            c.direct_url.as_deref().unwrap_or("")
        );
    }
    assert!(
        !cached.is_empty(),
        "expected at least one RD-cached stream with a direct URL"
    );
}
