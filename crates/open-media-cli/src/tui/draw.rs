// --- Rendering ---

use open_media_core::model::{Episode, Media, MediaKind};
use open_media_core::stream::{CacheState, SourceCandidate};
use open_media_core::tracking::{LibraryItem, ListStatus};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::Image;

use super::layout::{
    episodes_layout, help_layout, results_layout, sources_layout, sources_side_panel_layout,
    top_level_layout,
};
use super::state::{Focus, Screen, Theme};
use super::{App, POSTER_ROWS, STILL_ROWS};

pub(super) fn draw(f: &mut Frame, app: &App) {
    let layout = top_level_layout(f.area());

    // Header
    let title = format!("open-media — {}", breadcrumbs(app));
    f.render_widget(
        Paragraph::new(title)
            .style(Style::new().fg(app.theme.accent).bold())
            .block(Block::default().borders(Borders::ALL)),
        layout.header,
    );

    match app.screen {
        Screen::Home => draw_home(f, app, layout.body),
        Screen::Library => draw_library(f, app, layout.body),
        Screen::Search => draw_search(f, app, layout.body),
        Screen::Results => draw_results(f, app, layout.body),
        Screen::Seasons => draw_seasons(f, app, layout.body),
        Screen::Episodes => draw_episodes(f, app, layout.body),
        Screen::Sources => draw_sources_screen(f, app, layout.body),
    }

    // Footer / status
    let hints = match app.screen {
        Screen::Home => "click: select · Enter/double-click: open · /: search · ?: help · q: quit",
        Screen::Library => "click: select · double-click: open · h/l: filter · /: search · ?: help",
        Screen::Search => "Enter: search · ?: help · Esc: quit",
        Screen::Results => "click: select · double-click/Enter: select · /: search · ?: help",
        Screen::Seasons => "click: select · double-click/Enter: open · Esc: back · ?: help",
        Screen::Episodes => "click: select · double-click/Enter: open · Esc: back · ?: help",
        Screen::Sources => match app.focus {
            Focus::List => "click: select · double-click/Enter: play · Tab: filters · Esc: back",
            Focus::Panel => "click: control · double-click/Enter: apply · h/l: change · Tab: list",
        },
    };
    let spin = if app.busy { "⏳ " } else { "" };
    f.render_widget(
        Paragraph::new(format!("{spin}{}", app.status))
            .style(Style::new().fg(app.theme.status))
            .block(Block::default().borders(Borders::ALL).title(hints)),
        layout.footer,
    );

    if app.help {
        draw_help(f, app);
    }
}

fn breadcrumbs(app: &App) -> String {
    let title = app.media.as_ref().map(|m| m.display_title());
    match app.screen {
        Screen::Home => "Home".into(),
        Screen::Library => format!("Home > Library ({})", app.library_filter.label()),
        Screen::Search => "Search".into(),
        Screen::Results => "Search > Results".into(),
        Screen::Seasons => format!("Search > {} > Seasons", title.unwrap_or("Title")),
        Screen::Episodes => match (title, app.sel_season) {
            (Some(t), Some(s)) => format!("Search > {t} > Season {s}"),
            (Some(t), None) => format!("Search > {t} > Episodes"),
            _ => "Search > Episodes".into(),
        },
        Screen::Sources => match (title, app.sel_season) {
            (Some(t), Some(s)) => format!("Search > {t} > Season {s} > Sources"),
            (Some(t), None) => format!("Search > {t} > Sources"),
            _ => "Search > Sources".into(),
        },
    }
}

fn draw_home(f: &mut Frame, app: &App, area: Rect) {
    let counts = library_counts(&app.all_library);
    let rows = [
        format!("Continue Watching   {}", counts.0),
        format!("Planned             {}", counts.1),
        format!("Completed           {}", counts.2),
        "Search              press / or Enter".to_string(),
    ];
    let items: Vec<ListItem> = rows
        .iter()
        .map(|r| ListItem::new(Line::from(r.clone())))
        .collect();
    render_list(f, &app.theme, area, "Home", items, &app.home_state, true);
}

fn library_counts(items: &[LibraryItem]) -> (usize, usize, usize) {
    let watching = items
        .iter()
        .filter(|i| i.status == ListStatus::Watching)
        .count();
    let planned = items
        .iter()
        .filter(|i| i.status == ListStatus::Planning)
        .count();
    let completed = items
        .iter()
        .filter(|i| i.status == ListStatus::Completed)
        .count();
    (watching, planned, completed)
}

fn draw_library(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = if app.library.is_empty() {
        vec![ListItem::new(Line::from(vec![Span::styled(
            "No saved items in this filter. Press / to search, then use library commands to plan titles.",
            Style::new().fg(app.theme.dim),
        )]))]
    } else {
        app.library
            .iter()
            .map(|item| ListItem::new(library_row(item)))
            .collect()
    };
    let title = format!("Library [{}]", app.library_filter.label());
    render_list(f, &app.theme, area, &title, items, &app.library_state, true);
}

fn library_row(item: &LibraryItem) -> String {
    let year = item
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "—".into());
    let coord = match (item.last_season, item.last_episode) {
        (Some(s), Some(e)) => format!(" S{s:02}E{e:02}"),
        _ => String::new(),
    };
    let progress = if item.duration_secs > 0 {
        format!(" {:>3.0}%", item.progress_fraction() * 100.0)
    } else {
        String::new()
    };
    format!(
        "[{:<9}] [{:<6}] {} ({year}){coord}{progress}",
        status_label(item.status),
        item.kind.label(),
        item.title
    )
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Search", Style::new().fg(app.theme.accent).bold()),
            Span::raw(" movies, series, and anime"),
        ]),
        Line::from(""),
        Line::from(format!("› {}", app.query)),
    ];
    if app.busy {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Searching configured metadata providers… results will appear as soon as the lookup finishes.",
            Style::new().fg(app.theme.dim),
        )));
    } else if app.query.is_empty() {
        let suggestions: Vec<&str> = app
            .library
            .iter()
            .take(3)
            .map(|i| i.title.as_str())
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Type a title and press Enter. Esc returns home/quit.",
            Style::new().fg(app.theme.dim),
        )));
        if !suggestions.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Recent: {}", suggestions.join(" · ")),
                Style::new().fg(app.theme.dim),
            )));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Query"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_results(f: &mut Frame, app: &App, area: Rect) {
    let layout = results_layout(area);
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|m| {
            let badge = match m.kind {
                MediaKind::Movie => "[Movie]",
                MediaKind::Series => "[TV]",
                MediaKind::Anime => "[Anime]",
            };
            let year = m.year.map(|y| format!(" ({y})")).unwrap_or_default();
            ListItem::new(format!("{badge} {}{year}", m.display_title()))
        })
        .collect();
    render_list(
        f,
        &app.theme,
        layout.list,
        "Results",
        items,
        &app.results_state,
        true,
    );
    if let Some(panel) = layout.panel {
        draw_media_panel(f, app, panel);
    }
}

fn draw_media_panel(f: &mut Frame, app: &App, area: Rect) {
    let media = app
        .results_state
        .selected()
        .and_then(|i| app.results.get(i));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(app.theme.dim))
        .title("Details");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let (image_area, text_area) = if app.stills.enabled() && inner.height > POSTER_ROWS + 2 {
        let rows =
            Layout::vertical([Constraint::Length(POSTER_ROWS), Constraint::Min(0)]).split(inner);
        (Some(rows[0]), rows[1])
    } else {
        (None, inner)
    };
    if let Some(img_area) = image_area {
        draw_image(f, app, img_area, media.and_then(|m| m.poster.as_deref()));
    }
    f.render_widget(
        Paragraph::new(media.map(media_detail_lines).unwrap_or_else(|| {
            vec![Line::from(Span::styled(
                "No result selected.",
                Style::new().fg(app.theme.dim),
            ))]
        }))
        .wrap(Wrap { trim: true }),
        text_area,
    );
}

fn media_detail_lines(m: &Media) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        m.display_title().to_string(),
        Style::new().bold(),
    ))];
    let mut facts = vec![m.kind.label().to_string()];
    if let Some(y) = m.year {
        facts.push(y.to_string());
    }
    if let Some(score) = m.score.filter(|s| *s > 0.0) {
        facts.push(format!("score {score:.1}"));
    }
    if let Some(status) = &m.status {
        facts.push(status.clone());
    }
    if let (Some(s), Some(e)) = (m.season_count, m.episode_count) {
        facts.push(format!("{s} seasons · {e} eps"));
    } else if let Some(e) = m.episode_count {
        facts.push(format!("{e} eps"));
    }
    lines.push(Line::from(Span::styled(
        facts.join("   "),
        Style::new().fg(Color::Gray),
    )));
    if !m.genres.is_empty() {
        lines.push(Line::from(Span::styled(
            m.genres.join(", "),
            Style::new().fg(Color::Gray),
        )));
    }
    lines.push(Line::from(""));
    match &m.overview {
        Some(o) => push_multiline(&mut lines, o),
        None => lines.push(Line::from("No overview available.")),
    }
    lines
}

/// Push a plain-text block as one `Line` per source line. ratatui drops `\n`
/// inside a single `Line`, which would glue paragraphs together — overviews
/// carry real newlines (e.g. AniList `<br>`s converted by the adapter).
fn push_multiline(lines: &mut Vec<Line<'static>>, text: &str) {
    for l in text.split('\n') {
        lines.push(Line::from(l.to_string()));
    }
}

fn draw_seasons(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .seasons
        .iter()
        .map(|s| {
            let name = s
                .name
                .clone()
                .unwrap_or_else(|| format!("Season {}", s.number));
            ListItem::new(format!("{name}  ({} episodes)", s.episode_count))
        })
        .collect();
    render_list(
        f,
        &app.theme,
        area,
        "Seasons",
        items,
        &app.seasons_state,
        true,
    );
}

fn draw_episodes(f: &mut Frame, app: &App, area: Rect) {
    // Split off a passive detail panel when there's room; otherwise the list
    // takes the full width (narrow terminals aren't squeezed).
    let layout = episodes_layout(area);

    let items: Vec<ListItem> = app
        .episodes
        .iter()
        .map(|ep| ListItem::new(episode_row(ep)))
        .collect();
    render_list(
        f,
        &app.theme,
        layout.list,
        "Episodes",
        items,
        &app.episodes_state,
        true,
    );

    if let Some(panel) = layout.panel {
        draw_episode_panel(f, app, panel);
    }
}

/// One list line per episode: `E01 · Title`, falling back to the air date and
/// then to the bare coordinate when a provider couldn't supply a title.
fn episode_row(ep: &Episode) -> String {
    match &ep.title {
        Some(t) if !t.is_empty() => format!("E{:02} · {t}", ep.number),
        _ => match &ep.air_date {
            Some(d) if !d.is_empty() => format!("E{:02}   ({d})", ep.number),
            _ => format!("E{:02}", ep.number),
        },
    }
}

/// Passive detail panel for the highlighted episode: series context on top,
/// then the selected episode's title, air date, runtime, rating, and synopsis.
/// Follows the list cursor; it is not keyboard-focusable.
fn draw_episode_panel(f: &mut Frame, app: &App, area: Rect) {
    let sel = app.episodes_state.selected().unwrap_or(0);
    let ep = app.episodes.get(sel);

    let mut lines: Vec<Line> = Vec::new();
    let label = |s: &str| Span::styled(s.to_string(), Style::new().fg(app.theme.dim));

    // --- Series context (from the parent Media) ---
    if let Some(m) = &app.media {
        lines.push(Line::from(Span::styled(
            m.title.clone(),
            Style::new().fg(app.theme.accent).bold(),
        )));
        let mut meta: Vec<String> = Vec::new();
        if let Some(y) = m.year {
            meta.push(y.to_string());
        }
        if let Some(s) = m.score.filter(|s| *s > 0.0) {
            meta.push(format!("★ {s:.1}"));
        }
        if let (Some(sc), Some(ec)) = (m.season_count, m.episode_count) {
            meta.push(format!("{sc}S · {ec}E"));
        } else if let Some(ec) = m.episode_count {
            meta.push(format!("{ec}E"));
        }
        if !meta.is_empty() {
            lines.push(Line::from(label(&meta.join("   "))));
        }
        if !m.genres.is_empty() {
            lines.push(Line::from(label(&m.genres.join(", "))));
        }
        lines.push(Line::from(""));
    }

    // --- Selected episode ---
    match ep {
        Some(ep) => {
            let title = match &ep.title {
                Some(t) if !t.is_empty() => format!("E{:02} · {t}", ep.number),
                _ => format!("E{:02}", ep.number),
            };
            lines.push(Line::from(Span::styled(title, Style::new().bold())));

            let mut facts: Vec<String> = Vec::new();
            if let Some(d) = &ep.air_date {
                if !d.is_empty() {
                    facts.push(format!("Aired {d}"));
                }
            }
            if let Some(r) = ep.runtime_minutes {
                facts.push(format!("{r} min"));
            }
            // Treat a 0.0 rating as "unrated" — Cinemeta returns "0" for
            // episodes it has no score for, and `★ 0.0` reads as misleading.
            if let Some(rt) = ep.rating.filter(|r| *r > 0.0) {
                facts.push(format!("★ {rt:.1}"));
            }
            if !facts.is_empty() {
                lines.push(Line::from(label(&facts.join("   "))));
            }

            // Per-episode synopsis; fall back to the series overview (AniList
            // never carries episode synopses, but always has a description) —
            // more useful than a bare "No synopsis available.".
            let episode_overview = ep.overview.as_ref().filter(|o| !o.is_empty());
            let series_overview = app
                .media
                .as_ref()
                .and_then(|m| m.overview.as_ref())
                .filter(|o| !o.is_empty());
            match (episode_overview, series_overview) {
                (Some(o), _) => {
                    lines.push(Line::from(""));
                    push_multiline(&mut lines, o);
                }
                (None, Some(o)) => {
                    lines.push(Line::from(""));
                    lines.push(Line::from(label("About the series:")));
                    push_multiline(&mut lines, o);
                }
                (None, None) => {
                    lines.push(Line::from(""));
                    lines.push(Line::from(label("No synopsis available.")));
                }
            }
        }
        None => lines.push(Line::from(label("No episode selected."))),
    }

    // Frame the panel, then split the interior into a still image on top and
    // the text details below.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(app.theme.dim))
        .title("Episode");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let (image_area, text_area) = if app.stills.enabled() && inner.height > STILL_ROWS + 2 {
        let rows =
            Layout::vertical([Constraint::Length(STILL_ROWS), Constraint::Min(0)]).split(inner);
        (Some(rows[0]), rows[1])
    } else {
        (None, inner)
    };

    if let Some(img_area) = image_area {
        draw_image(f, app, img_area, app.current_still_url());
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), text_area);
}

/// Render an image URL into `area`: the decoded image when ready, otherwise a
/// centered status line (loading / unavailable).
fn draw_image(f: &mut Frame, app: &App, area: Rect, url: Option<&str>) {
    if let Some(protocol) = url.and_then(|u| app.stills.ready(u)) {
        // Cheap render of the already-resized+encoded protocol image.
        f.render_widget(Image::new(protocol), area);
        return;
    }

    let status = if url.is_some() {
        "  loading image…"
    } else {
        "  no image"
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            status,
            Style::new().fg(app.theme.dim),
        ))),
        area,
    );
}

fn draw_sources_screen(f: &mut Frame, app: &App, area: Rect) {
    // Split off the side panel when there's room; otherwise list takes it all.
    let layout = sources_layout(area);
    let show_panel = layout.panel.is_some();

    draw_sources_list(f, app, layout.list, show_panel);
    if let Some(panel) = layout.panel {
        draw_sources_panel(f, app, panel);
    }
}

fn draw_sources_list(f: &mut Frame, app: &App, area: Rect, show_panel: bool) {
    let items: Vec<ListItem> = app
        .visible
        .iter()
        .filter_map(|&i| app.candidates.get(i))
        .map(|c| ListItem::new(source_row_line(c)))
        .collect();
    let focused = !show_panel || app.focus == Focus::List;
    let title = format!("Sources ({})", app.visible.len());
    render_list(
        f,
        &app.theme,
        area,
        &title,
        items,
        &app.candidates_state,
        focused,
    );
}

/// One scannable line per candidate: cache, quality, seed health, provider, size,
/// and release name (the full text lives in the panel).
#[cfg(test)]
pub(super) fn source_row(c: &SourceCandidate) -> String {
    let cache = cache_badge(c.cache);
    let seeders = seed_badge(c.seeders);
    let name = c.title.lines().next().unwrap_or(&c.title).trim();
    format!(
        "{cache:<8} [{:<5}] {seeders:<10} {:<12} {:>9}  {}",
        c.quality.label(),
        truncate(&c.provider, 12),
        c.human_size(),
        name,
    )
}

fn source_row_line(c: &SourceCandidate) -> Line<'static> {
    let style = match seed_health(c.seeders) {
        SeedHealth::Hot => Style::new().fg(Color::Green),
        SeedHealth::Ok => Style::new().fg(Color::Yellow),
        SeedHealth::Cold => Style::new().fg(Color::Red),
        SeedHealth::Unknown => Style::new().fg(Color::Gray),
    };
    let name = c
        .title
        .lines()
        .next()
        .unwrap_or(&c.title)
        .trim()
        .to_string();
    Line::from(vec![
        Span::raw(format!("{:<8} ", cache_badge(c.cache))),
        Span::styled(format!("[{:<5}] ", c.quality.label()), Style::new().bold()),
        Span::styled(format!("{:<10} ", seed_badge(c.seeders)), style),
        Span::raw(format!("{:<12} ", truncate(&c.provider, 12))),
        Span::raw(format!("{:>9}  ", c.human_size())),
        Span::raw(name),
    ])
}

fn cache_badge(cache: CacheState) -> &'static str {
    match cache {
        CacheState::Cached => "[cached]",
        CacheState::Uncached => "[fetch]",
        CacheState::Unknown => "[?]",
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SeedHealth {
    Hot,
    Ok,
    Cold,
    Unknown,
}

pub(super) fn seed_health(seeders: Option<u32>) -> SeedHealth {
    match seeders {
        Some(s) if s >= 100 => SeedHealth::Hot,
        Some(s) if s >= 10 => SeedHealth::Ok,
        Some(_) => SeedHealth::Cold,
        None => SeedHealth::Unknown,
    }
}

fn seed_badge(seeders: Option<u32>) -> String {
    match (seed_health(seeders), seeders) {
        (SeedHealth::Hot, Some(s)) => format!("HOT {s}S"),
        (SeedHealth::Ok, Some(s)) => format!("OK {s}S"),
        (SeedHealth::Cold, Some(s)) => format!("LOW {s}S"),
        _ => "SEEDS ?".into(),
    }
}

fn status_label(status: ListStatus) -> &'static str {
    match status {
        ListStatus::Watching => "watching",
        ListStatus::Completed => "completed",
        ListStatus::Planning => "planned",
        ListStatus::Paused => "paused",
        ListStatus::Dropped => "dropped",
        ListStatus::Repeating => "repeating",
    }
}

fn draw_help(f: &mut Frame, app: &App) {
    let area = help_layout(f.area());
    f.render_widget(ratatui::widgets::Clear, area);
    let text = help_lines(app);
    f.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(app.theme.accent))
                    .title("Help"),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn help_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            breadcrumbs(app),
            Style::new().fg(app.theme.accent).bold(),
        )),
        Line::from(""),
        Line::from("j/k or arrows  move selection"),
        Line::from("/              jump to search"),
        Line::from("?              toggle this help"),
        Line::from("q or Ctrl-C     quit"),
    ];
    match app.screen {
        Screen::Home => lines.extend([
            Line::from(""),
            Line::from("Enter opens the selected shelf or search action."),
            Line::from("l opens the full library."),
        ]),
        Screen::Library => lines.extend([
            Line::from(""),
            Line::from("h/l or Left/Right cycles library filters."),
            Line::from("Enter resumes a saved episode when possible."),
        ]),
        Screen::Sources => lines.extend([
            Line::from(""),
            Line::from("Tab switches between source list and filters."),
            Line::from("In filters, h/l changes values and Enter applies."),
        ]),
        _ => lines.extend([
            Line::from(""),
            Line::from("Enter follows the selected result, season, episode, or source."),
            Line::from("Esc goes back where available."),
        ]),
    }
    lines
}

fn draw_sources_panel(f: &mut Frame, app: &App, area: Rect) {
    let layout = sources_side_panel_layout(area);
    draw_filter_box(f, app, layout.filters);
    draw_details_box(f, app, layout.details);
}

fn draw_filter_box(f: &mut Frame, app: &App, area: Rect) {
    let panel_focused = app.focus == Focus::Panel;
    let sel = app.panel_state.selected().unwrap_or(0);
    let q = app
        .filters
        .quality
        .map(|q| q.label().to_string())
        .unwrap_or_else(|| "All".into());
    let lang = app.filters.language.clone().unwrap_or_else(|| "All".into());
    let prov = app.filters.provider.clone().unwrap_or_else(|| "All".into());
    let cached = if app.filters.cached_only { "On" } else { "Off" };

    let rows = [
        format!("Sort     : {}", app.filters.sort.label()),
        format!("Quality  : {q}"),
        format!("Language : {lang}"),
        format!("Provider : {prov}"),
        format!("Cached   : {cached}"),
        "[ Clear filters ]".to_string(),
    ];
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut item = ListItem::new(r.clone());
            if panel_focused && i == sel {
                item = item.style(app.theme.highlight());
            }
            item
        })
        .collect();
    let border = app.theme.border(panel_focused);
    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(border))
                .title("Filters / Sort"),
        ),
        area,
    );
}

fn draw_details_box(f: &mut Frame, app: &App, area: Rect) {
    let text = match app.current_candidate() {
        Some(c) => {
            let t = &c.tags;
            let langs = if t.languages.is_empty() {
                "—".to_string()
            } else {
                t.languages.join(", ")
            };
            let cache = match c.cache {
                CacheState::Cached => "cached ⚡",
                CacheState::Uncached => "uncached",
                CacheState::Unknown => "unknown",
            };
            let mut lines = vec![
                c.title
                    .lines()
                    .next()
                    .unwrap_or(&c.title)
                    .trim()
                    .to_string(),
                String::new(),
                format!("Provider : {}", c.provider),
                format!("Quality  : {}", c.quality.label()),
                format!("Size     : {}", c.human_size()),
                format!(
                    "Seeders  : {}",
                    c.seeders
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "?".into())
                ),
                format!("Cache    : {cache}"),
                format!("Language : {langs}"),
            ];
            if let Some(v) = &t.video_codec {
                lines.push(format!("Video    : {v}"));
            }
            if let Some(a) = &t.audio {
                lines.push(format!("Audio    : {a}"));
            }
            if let Some(h) = &t.hdr {
                lines.push(format!("HDR      : {h}"));
            }
            if let Some(s) = &t.source_type {
                lines.push(format!("Source   : {s}"));
            }
            lines.join("\n")
        }
        None => "No candidate selected.".to_string(),
    };
    f.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(app.theme.dim))
                    .title("Details"),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

/// Truncate to `max` chars (with an ellipsis) for fixed-width columns.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn render_list(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    title: &str,
    items: Vec<ListItem>,
    state: &ListState,
    focused: bool,
) {
    let border = theme.border(focused);
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(border))
                .title(title.to_string()),
        )
        .highlight_style(theme.highlight())
        .highlight_symbol("▶ ");
    let mut s = *state;
    f.render_stateful_widget(list, area, &mut s);
}
