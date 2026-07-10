use open_media_core::model::{IdSet, MediaKind};
use open_media_core::stream::{CacheState, Quality, ReleaseTags, SourceCandidate};
use open_media_core::tracking::{LibraryItem, ListStatus};
use ratatui::prelude::Rect;

use super::draw::{progress_cells_filled, seed_health, source_row, SeedHealth};
use super::input::search_status;
use super::layout::{
    list_index_at, panel_control_at, results_layout, sources_layout, sources_side_panel_layout,
};
use super::state::{
    build_home_rows, cycle_opt, cycle_quality, visible_indices, HomeRow, LibraryFilter, Nav,
    PanelControl, Root, Screen, SortKey, SourceFilters, Theme,
};
use super::{sources_nav_stack, PANEL_MIN_WIDTH, PANEL_WIDTH, RESULT_PANEL_WIDTH};

fn cand(
    provider: &str,
    quality: Quality,
    size: u64,
    seeders: Option<u32>,
    cache: CacheState,
    languages: &[&str],
) -> SourceCandidate {
    SourceCandidate {
        provider: provider.into(),
        title: format!("{provider} release"),
        quality,
        size_bytes: size,
        seeders,
        info_hash: Some("hash".into()),
        magnet: None,
        direct_url: None,
        file_index: None,
        cache,
        tags: ReleaseTags {
            languages: languages.iter().map(|s| s.to_string()).collect(),
            ..ReleaseTags::default()
        },
    }
}

fn sample() -> Vec<SourceCandidate> {
    vec![
        cand(
            "1337x",
            Quality::P1080,
            2_000,
            Some(400),
            CacheState::Cached,
            &["English"],
        ),
        cand(
            "RARBG",
            Quality::P2160,
            18_000,
            Some(40),
            CacheState::Uncached,
            &["English", "Italian"],
        ),
        cand(
            "TPB",
            Quality::P720,
            800,
            Some(900),
            CacheState::Unknown,
            &["Spanish"],
        ),
    ]
}

fn filters() -> SourceFilters {
    SourceFilters {
        sort: SortKey::Relevance,
        quality: None,
        language: None,
        provider: None,
        cached_only: false,
    }
}

#[test]
fn list_click_maps_first_interior_row_to_index_zero() {
    let area = Rect::new(10, 5, 20, 6);
    assert_eq!(list_index_at(area, 11, 6, 3), Some(0));
}

#[test]
fn list_border_clicks_return_none() {
    let area = Rect::new(10, 5, 20, 6);
    assert_eq!(list_index_at(area, 10, 6, 3), None);
    assert_eq!(list_index_at(area, 11, 5, 3), None);
}

#[test]
fn list_clicks_below_item_count_return_none() {
    let area = Rect::new(10, 5, 20, 6);
    assert_eq!(list_index_at(area, 11, 8, 2), None);
}

#[test]
fn results_layout_shows_and_hides_detail_panel_by_width() {
    let wide = results_layout(Rect::new(0, 0, PANEL_MIN_WIDTH, 20));
    assert_eq!(wide.list.width, PANEL_MIN_WIDTH - RESULT_PANEL_WIDTH);
    assert_eq!(wide.panel.map(|r| r.width), Some(RESULT_PANEL_WIDTH));

    let narrow = results_layout(Rect::new(0, 0, PANEL_MIN_WIDTH - 1, 20));
    assert_eq!(narrow.list.width, PANEL_MIN_WIDTH - 1);
    assert_eq!(narrow.panel, None);
}

#[test]
fn sources_layout_shows_and_hides_side_panel_by_width() {
    let wide = sources_layout(Rect::new(0, 0, PANEL_MIN_WIDTH, 20));
    assert_eq!(wide.list.width, PANEL_MIN_WIDTH - PANEL_WIDTH);
    assert_eq!(wide.panel.map(|r| r.width), Some(PANEL_WIDTH));

    let narrow = sources_layout(Rect::new(0, 0, PANEL_MIN_WIDTH - 1, 20));
    assert_eq!(narrow.list.width, PANEL_MIN_WIDTH - 1);
    assert_eq!(narrow.panel, None);
}

#[test]
fn filter_box_rows_map_to_panel_controls() {
    let panel = Rect::new(50, 3, PANEL_WIDTH, 20);
    let filters = sources_side_panel_layout(panel).filters;
    assert_eq!(panel_control_at(filters, 51, 4), Some(PanelControl::Sort));
    assert_eq!(
        panel_control_at(filters, 51, 6),
        Some(PanelControl::Language)
    );
    assert_eq!(panel_control_at(filters, 51, 9), Some(PanelControl::Clear));
    assert_eq!(panel_control_at(filters, 51, 3), None);
}

#[test]
fn relevance_preserves_engine_order() {
    let c = sample();
    assert_eq!(visible_indices(&c, &filters()), vec![0, 1, 2]);
}

#[test]
fn quality_filter_selects_one() {
    let c = sample();
    let mut f = filters();
    f.quality = Some(Quality::P2160);
    assert_eq!(visible_indices(&c, &f), vec![1]);
}

#[test]
fn language_filter_matches_any_listed() {
    let c = sample();
    let mut f = filters();
    f.language = Some("Italian".into());
    assert_eq!(visible_indices(&c, &f), vec![1]);
    f.language = Some("English".into());
    assert_eq!(visible_indices(&c, &f), vec![0, 1]);
}

#[test]
fn provider_and_cached_filters() {
    let c = sample();
    let mut f = filters();
    f.provider = Some("TPB".into());
    assert_eq!(visible_indices(&c, &f), vec![2]);
    let mut f2 = filters();
    f2.cached_only = true;
    assert_eq!(visible_indices(&c, &f2), vec![0]);
}

#[test]
fn sorts_by_seeders_quality_size() {
    let c = sample();
    let mut f = filters();
    f.sort = SortKey::Seeders;
    assert_eq!(visible_indices(&c, &f), vec![2, 0, 1]); // 900, 400, 40
    f.sort = SortKey::Quality;
    assert_eq!(visible_indices(&c, &f), vec![1, 0, 2]); // 2160, 1080, 720
    f.sort = SortKey::Size;
    assert_eq!(visible_indices(&c, &f), vec![1, 0, 2]); // 18000, 2000, 800
}

#[test]
fn empty_when_filters_exclude_all() {
    let c = sample();
    let mut f = filters();
    f.quality = Some(Quality::P480);
    assert!(visible_indices(&c, &f).is_empty());
}

#[test]
fn quality_cycles_and_wraps() {
    assert_eq!(cycle_quality(None, 1), Some(Quality::P2160));
    assert_eq!(cycle_quality(Some(Quality::P360), 1), None); // wrap to All
    assert_eq!(cycle_quality(None, -1), Some(Quality::P360)); // wrap back
}

#[test]
fn opt_cycle_includes_all_sentinel() {
    let opts = vec!["English".to_string(), "Italian".to_string()];
    assert_eq!(cycle_opt(&None, &opts, 1), Some("English".into()));
    assert_eq!(cycle_opt(&Some("Italian".into()), &opts, 1), None);
}

#[test]
fn filters_roundtrip_through_config() {
    let mut ui = open_media_config::SourcesUi::default();
    let f = SourceFilters {
        sort: SortKey::Seeders,
        quality: Some(Quality::P1080),
        language: Some("English".into()),
        provider: Some("1337x".into()),
        cached_only: true,
    };
    f.write_cfg(&mut ui);
    assert_eq!(ui.sort, "seeders");
    assert_eq!(ui.quality, "1080p");
    let back = SourceFilters::from_cfg(&ui);
    assert_eq!(back.sort, SortKey::Seeders);
    assert_eq!(back.quality, Some(Quality::P1080));
    assert_eq!(back.language.as_deref(), Some("English"));
    assert!(back.cached_only);
}

#[test]
fn config_all_sentinel_parses_to_none() {
    let ui = open_media_config::SourcesUi::default(); // all "all"
    let f = SourceFilters::from_cfg(&ui);
    assert_eq!(f.quality, None);
    assert_eq!(f.language, None);
    assert_eq!(f.provider, None);
    assert_eq!(f.sort, SortKey::Relevance);
}

#[test]
fn source_row_uses_scannable_badges() {
    let c = cand(
        "Torrentio",
        Quality::P1080,
        2_000,
        Some(400),
        CacheState::Cached,
        &["English"],
    );
    let row = source_row(&c);
    assert!(row.contains("[cached]"));
    assert!(row.contains("[1080p]"));
    assert!(row.contains("HOT 400S"));
    assert!(row.contains("Torrentio"));
}

#[test]
fn seed_health_tiers_are_stable() {
    assert_eq!(seed_health(Some(100)), SeedHealth::Hot);
    assert_eq!(seed_health(Some(10)), SeedHealth::Ok);
    assert_eq!(seed_health(Some(1)), SeedHealth::Cold);
    assert_eq!(seed_health(None), SeedHealth::Unknown);
}

#[test]
fn library_filter_cycles_and_maps_status() {
    assert_eq!(LibraryFilter::All.cycle(1), LibraryFilter::Watching);
    assert_eq!(LibraryFilter::Dropped.cycle(1), LibraryFilter::All);
    assert_eq!(LibraryFilter::Planned.status(), Some(ListStatus::Planning));
    assert_eq!(LibraryFilter::All.status(), None);
}

#[test]
fn theme_presets_differ_for_dark_and_light() {
    let dark = Theme::from_cfg("dark");
    let light = Theme::from_cfg("light");
    let auto = Theme::from_cfg("auto");
    assert_eq!(dark.bg, auto.bg);
    assert_ne!(dark.bg, light.bg);
    assert_ne!(dark.accent, light.accent);
    // Media-app dark: charcoal canvas + distinct cyan-ish accent and selection.
    assert_ne!(dark.bg, dark.selection_bg);
    assert_ne!(dark.text, dark.dim);
    assert_ne!(dark.success, dark.danger);
}

#[test]
fn root_digit_keys_map_to_tabs() {
    assert_eq!(Root::from_digit('1'), Some(Root::Home));
    assert_eq!(Root::from_digit('2'), Some(Root::Library));
    assert_eq!(Root::from_digit('3'), Some(Root::Search));
    assert_eq!(Root::from_digit('0'), None);
    assert_eq!(Root::Home.label(), "Home");
    assert_eq!(Root::Library.as_screen(), Screen::Library);
    assert!(Screen::Home.is_root());
    assert!(Screen::Sources.is_drill());
    assert!(!Screen::Search.is_drill());
}

#[test]
fn nav_esc_semantics_pop_to_root() {
    let mut nav = Nav::new(Root::Search);
    nav.set_stack([Screen::Results, Screen::Sources]);
    assert_eq!(nav.current(), Screen::Sources);
    assert!(nav.pop());
    assert_eq!(nav.current(), Screen::Results);
    assert!(nav.pop());
    assert!(nav.is_at_root());
    assert_eq!(nav.current(), Screen::Search);
    // Empty stack: pop is a no-op (Esc must not quit — handled in input).
    assert!(!nav.pop());
    assert_eq!(nav.current(), Screen::Search);
}

#[test]
fn nav_library_root_keeps_breadcrumb_root_through_drill() {
    // Library entry must not pretend to be under Search after drilling in.
    let mut nav = Nav::new(Root::Library);
    nav.set_stack([Screen::Results, Screen::Episodes, Screen::Sources]);
    assert_eq!(nav.root, Root::Library);
    assert_eq!(nav.root.label(), "Library");
    assert_eq!(nav.current(), Screen::Sources);
    assert!(nav.pop());
    assert_eq!(nav.current(), Screen::Episodes);
    assert_eq!(nav.root, Root::Library);
}

#[test]
fn sources_stack_skips_seasons_episodes_when_not_loaded_for_title() {
    // Resume / movie path: no seasons or episodes loaded for this title.
    assert_eq!(
        sources_nav_stack(true, 0, false, true),
        vec![Screen::Results, Screen::Sources]
    );
    assert_eq!(
        sources_nav_stack(false, 0, false, true),
        vec![Screen::Sources]
    );
}

#[test]
fn sources_stack_includes_seasons_and_episodes_when_loaded() {
    assert_eq!(
        sources_nav_stack(true, 5, true, true),
        vec![
            Screen::Results,
            Screen::Seasons,
            Screen::Episodes,
            Screen::Sources
        ]
    );
    // Multi-season but jumped to sources without loading episodes (should not
    // invent an Episodes level).
    assert_eq!(
        sources_nav_stack(true, 5, false, true),
        vec![Screen::Results, Screen::Seasons, Screen::Sources]
    );
}

fn lib_item(title: &str, kind: MediaKind, updated_at: i64) -> LibraryItem {
    LibraryItem {
        media_key: format!("k:{title}"),
        ids: IdSet::default(),
        title: title.into(),
        kind,
        poster: None,
        year: Some(2021),
        status: ListStatus::Watching,
        last_season: Some(1),
        last_episode: Some(1),
        position_secs: 10,
        duration_secs: 100,
        updated_at,
    }
}

#[test]
fn progress_2x5_maps_percent_to_ten_cells() {
    assert_eq!(progress_cells_filled(0.0), 0);
    assert_eq!(progress_cells_filled(0.5), 5); // top row full
    assert_eq!(progress_cells_filled(0.94), 9);
    assert_eq!(progress_cells_filled(1.0), 10);
    assert_eq!(progress_cells_filled(1.5), 10);
}

#[test]
fn home_rows_group_by_kind_newest_groups_first() {
    let mut sorted = vec![
        lib_item("Fresh Anime", MediaKind::Anime, 300),
        lib_item("Mid Anime", MediaKind::Anime, 200),
        lib_item("Older Series", MediaKind::Series, 100),
    ];
    sorted.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
    let rows = build_home_rows(&sorted);
    let labels: Vec<&str> = rows
        .iter()
        .filter_map(|r| match r {
            HomeRow::Section(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(labels[0], "Anime · 2");
    assert_eq!(labels[1], "Tv · 1");
    assert!(labels.contains(&"Actions"));
    assert!(!HomeRow::Section("x".into()).is_selectable());
    assert!(HomeRow::Continue(0).is_selectable());
}

#[test]
fn input_source_never_sets_quit_on_esc() {
    // Structural guarantee: shipped handle_key Esc arms must not assign
    // should_quit. Read the input module source once so a future regression
    // that reintroduces `Esc => should_quit` fails this test.
    let src = include_str!("input.rs");
    // No single-arm quit on Esc (historical Search bug).
    assert!(
        !src.contains("KeyCode::Esc => app.should_quit"),
        "Esc must not quit the TUI"
    );
    assert!(
        src.contains("Esc from Search always returns to Home"),
        "Search Esc must document go-home behavior"
    );
    assert!(
        src.contains("app.go_root(Root::Home)"),
        "Search Esc must call go_root(Home)"
    );
}

#[test]
fn search_status_distinguishes_partial_final_and_failures() {
    assert_eq!(
        search_status(12, &[], false),
        "12 results · still searching..."
    );
    assert_eq!(search_status(12, &[], true), "12 results");
    assert_eq!(
        search_status(12, &[String::from("anilist")], false),
        "12 results · still searching... · anilist failed"
    );
    assert_eq!(
        search_status(12, &[String::from("tmdb"), String::from("anilist")], false),
        "12 results · still searching... · 2 providers failed: tmdb, anilist"
    );
}
