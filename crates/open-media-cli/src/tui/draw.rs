// --- Rendering ---

use open_media_core::model::{Episode, Media, MediaKind};
use open_media_core::stream::{CacheState, SourceCandidate};
use open_media_core::tracking::{LibraryItem, ListStatus};
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui_image::Image;

use super::layout::{
    episodes_layout, header_layout, help_layout, results_layout, sources_layout,
    sources_side_panel_layout, top_level_layout,
};
use super::state::{Focus, HomeRow, Root, Screen, Theme};
use super::{App, POSTER_ROWS, STILL_ROWS};

pub(super) fn draw(f: &mut Frame, app: &App) {
    // Full-frame charcoal canvas.
    f.render_widget(
        Block::default().style(Style::new().bg(app.theme.bg).fg(app.theme.text)),
        f.area(),
    );

    let layout = top_level_layout(f.area());
    draw_header(f, app, layout.header);

    match app.screen() {
        Screen::Home => draw_home(f, app, layout.body),
        Screen::Library => draw_library(f, app, layout.body),
        Screen::Search => draw_search(f, app, layout.body),
        Screen::Results => draw_results(f, app, layout.body),
        Screen::Seasons => draw_seasons(f, app, layout.body),
        Screen::Episodes => draw_episodes(f, app, layout.body),
        Screen::Sources => draw_sources_screen(f, app, layout.body),
    }

    draw_footer(f, app, layout.footer);

    if app.help {
        draw_help(f, app);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let parts = header_layout(area);
    let compact = area.width < 80;

    // Brand
    f.render_widget(
        Paragraph::new(Span::styled(
            " open-media",
            Style::new().fg(app.theme.accent).bold(),
        )),
        parts.brand,
    );

    // Tabs — highlight the active root even while drilling down.
    let mut tab_spans: Vec<Span> = Vec::new();
    for root in Root::ALL {
        let active = app.nav.root == root;
        let label = if compact {
            root.short().to_string()
        } else {
            root.label().to_string()
        };
        if active {
            tab_spans.push(Span::styled(
                format!(" {label} "),
                Style::new()
                    .fg(app.theme.selection_fg)
                    .bg(app.theme.selection_bg)
                    .bold(),
            ));
        } else {
            tab_spans.push(Span::styled(
                format!(" {label} "),
                Style::new().fg(app.theme.dim),
            ));
        }
        tab_spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(tab_spans)), parts.tabs);

    // Trail / breadcrumbs — pad right so text doesn't kiss the terminal edge.
    let trail = format!("{}  ", breadcrumbs(app));
    f.render_widget(
        Paragraph::new(Span::styled(trail, Style::new().fg(app.theme.muted)))
            .alignment(Alignment::Right),
        parts.trail,
    );

    // Bottom rule under header
    if area.height >= 2 {
        let rule = Rect {
            x: area.x,
            y: area.y + area.height - 1,
            width: area.width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new("─".repeat(area.width as usize))
                .style(Style::new().fg(app.theme.border)),
            rule,
        );
    }
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let hints = footer_hints(app);
    let spin = if app.busy {
        format!("{} ", app.spinner_frame())
    } else {
        String::new()
    };
    let status_line = Line::from(vec![
        Span::styled(spin, Style::new().fg(app.theme.accent)),
        Span::styled(app.status.clone(), Style::new().fg(app.theme.status)),
    ]);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(app.theme.border))
        .border_type(BorderType::Plain);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(inner);
    f.render_widget(Paragraph::new(status_line), rows[0]);
    f.render_widget(Paragraph::new(hints), rows[1]);
}

fn footer_hints(app: &App) -> Line<'static> {
    let t = &app.theme;
    let mut parts: Vec<(&'static str, &'static str)> = Vec::new();
    match app.screen() {
        Screen::Home => {
            parts.extend([
                ("Enter", "open"),
                ("1-3", "tabs"),
                ("/", "search"),
                ("?", "help"),
                ("q", "quit"),
            ]);
        }
        Screen::Library => {
            parts.extend([
                ("Enter", "open"),
                ("h/l", "filter"),
                ("1-3", "tabs"),
                ("/", "search"),
                ("?", "help"),
                ("q", "quit"),
            ]);
        }
        Screen::Search => {
            parts.extend([
                ("Enter", "search"),
                ("Esc", "back"),
                ("?", "help"),
                ("q", "quit"),
            ]);
        }
        Screen::Results | Screen::Seasons | Screen::Episodes => {
            parts.extend([
                ("Enter", "select"),
                ("Esc", "back"),
                ("/", "search"),
                ("?", "help"),
                ("q", "quit"),
            ]);
        }
        Screen::Sources => match app.focus {
            Focus::List => {
                parts.extend([
                    ("Enter", "play"),
                    ("Tab", "filters"),
                    ("Esc", "back"),
                    ("?", "help"),
                ]);
            }
            Focus::Panel => {
                parts.extend([
                    ("h/l", "change"),
                    ("Enter", "apply"),
                    ("Tab", "list"),
                    ("Esc", "list"),
                ]);
            }
        },
    }
    key_hints_line(parts, t)
}

fn key_hints_line(parts: Vec<(&'static str, &'static str)>, theme: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, action)) in parts.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::new().fg(theme.muted)));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::new().fg(theme.text).bold(),
        ));
        spans.push(Span::styled(
            format!(" {action}"),
            Style::new().fg(theme.dim),
        ));
    }
    Line::from(spans)
}

fn breadcrumbs(app: &App) -> String {
    let root = app.nav.root.label();
    let title = app.media.as_ref().map(|m| m.display_title());
    match app.screen() {
        Screen::Home => root.into(),
        Screen::Library => format!("{root} · {}", app.library_filter.label()),
        Screen::Search => root.into(),
        Screen::Results => format!("{root} › Results"),
        Screen::Seasons => format!("{root} › {} › Seasons", title.unwrap_or("Title")),
        Screen::Episodes => match (title, app.sel_season) {
            (Some(t), Some(s)) => format!("{root} › {t} › S{s}"),
            (Some(t), None) => format!("{root} › {t} › Episodes"),
            _ => format!("{root} › Episodes"),
        },
        Screen::Sources => match (title, app.sel_season, app.sel_episode) {
            (Some(t), Some(s), Some(e)) => format!("{root} › {t} › S{s:02}E{e:02}"),
            (Some(t), Some(s), None) => format!("{root} › {t} › S{s} › Sources"),
            (Some(t), None, None) => format!("{root} › {t} › Sources"),
            _ => format!("{root} › Sources"),
        },
    }
}

fn draw_home(f: &mut Frame, app: &App, area: Rect) {
    // Interior width for title truncation (list border + highlight symbol).
    let inner_w = area.width.saturating_sub(4) as usize;

    let items: Vec<ListItem> = if app.home_rows.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "Nothing here yet. Press / to search, or open the library.",
            Style::new().fg(app.theme.dim),
        )))]
    } else {
        app.home_rows
            .iter()
            .map(|row| match row {
                HomeRow::Section(label) => ListItem::new(Line::from(vec![
                    Span::styled("── ", Style::new().fg(app.theme.border)),
                    Span::styled(label.clone(), Style::new().fg(app.theme.muted).bold()),
                    Span::styled(" ", Style::new().fg(app.theme.border)),
                    Span::styled(
                        "─".repeat(
                            inner_w
                                .saturating_sub(label.len().saturating_add(4))
                                .min(40),
                        ),
                        Style::new().fg(app.theme.border),
                    ),
                ])),
                HomeRow::Continue(i) => {
                    let item = &app.continue_watching[*i];
                    continue_list_item(item, &app.theme, inner_w)
                }
                // No leading arrow — list highlight_symbol ("▶ ") owns selection.
                HomeRow::OpenLibrary => action_list_item(
                    "Library",
                    "browse all saved titles",
                    "2",
                    &app.theme,
                    inner_w,
                ),
                HomeRow::Search => action_list_item(
                    "Search",
                    "find movies, series, anime",
                    "/",
                    &app.theme,
                    inner_w,
                ),
            })
            .collect()
    };

    let title = if app.continue_watching.is_empty() {
        "Home".to_string()
    } else {
        format!("Home · Continue Watching ({})", app.continue_watching.len())
    };
    render_list(f, &app.theme, area, &title, items, &app.home_state, true);
}

fn continue_list_item(item: &LibraryItem, theme: &Theme, inner_w: usize) -> ListItem<'static> {
    media_list_item(
        item,
        theme,
        inner_w,
        MediaListRowOpts {
            status: false,
            kind: true,
        },
    )
}

/// Home quick-action row: kind-badge-sized chip + dim hint + shortcut key.
/// No baked-in `→` — the list's `▶ ` highlight_symbol owns selection, so
/// actions share the same left edge / chip rhythm as continue-watching rows.
fn action_list_item(
    label: &str,
    hint: &str,
    shortcut: &str,
    theme: &Theme,
    inner_w: usize,
) -> ListItem<'static> {
    // Fixed 9-cell chip (matches `kind_badge_span` outer width).
    const BADGE_W: usize = 9;
    let badge = Span::styled(
        format!(" {label:^7} "),
        Style::new().fg(theme.accent).bg(theme.surface).bold(),
    );

    // "  hint · sc" after the badge — compact, no stretch-to-edge gap.
    let sc = shortcut.to_string();
    let fixed = BADGE_W + 2 + 3 + sc.chars().count(); // badge + gap + " · " + key
    let hint_budget = inner_w.saturating_sub(fixed).max(4);
    let hint_shown = truncate(hint, hint_budget);

    ListItem::new(Line::from(vec![
        badge,
        Span::raw("  "),
        Span::styled(hint_shown, Style::new().fg(theme.dim)),
        Span::styled(" · ", Style::new().fg(theme.border)),
        Span::styled(sc, Style::new().fg(theme.muted)),
    ]))
}

/// Relative age from unix epoch seconds (`just now`, `5m ago`, `4d ago`, …).
fn relative_age(updated_at: i64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(updated_at);
    let secs = (now - updated_at).max(0);
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 86_400 * 14 {
        format!("{}d ago", secs / 86_400)
    } else {
        format!("{}w ago", secs / (86_400 * 7))
    }
}

fn draw_library(f: &mut Frame, app: &App, area: Rect) {
    // Filter chips line + list
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    draw_library_filters(f, app, chunks[0]);

    let list_area = chunks[1];
    let inner_w = list_area.width.saturating_sub(4) as usize;

    let items: Vec<ListItem> = if app.library.is_empty() {
        vec![ListItem::new(Line::from(vec![Span::styled(
            "No saved items in this filter. Press / to search.",
            Style::new().fg(app.theme.dim),
        )]))]
    } else {
        app.library
            .iter()
            .map(|item| library_list_item(item, &app.theme, inner_w))
            .collect()
    };
    let title = format!(
        "Library · {} ({})",
        app.library_filter.label(),
        app.library.len()
    );
    render_list(
        f,
        &app.theme,
        list_area,
        &title,
        items,
        &app.library_state,
        true,
    );
}

fn draw_library_filters(f: &mut Frame, app: &App, area: Rect) {
    use super::state::LibraryFilter;
    let mut spans = Vec::new();
    for filt in [
        LibraryFilter::All,
        LibraryFilter::Watching,
        LibraryFilter::Planned,
        LibraryFilter::Completed,
        LibraryFilter::Dropped,
    ] {
        let active = app.library_filter == filt;
        let style = if active {
            Style::new()
                .fg(app.theme.selection_fg)
                .bg(app.theme.selection_bg)
                .bold()
        } else {
            Style::new().fg(app.theme.dim)
        };
        spans.push(Span::styled(format!(" {} ", filt.label()), style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        "  h/l cycle",
        Style::new().fg(app.theme.muted),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn library_list_item(item: &LibraryItem, theme: &Theme, inner_w: usize) -> ListItem<'static> {
    media_list_item(
        item,
        theme,
        inner_w,
        MediaListRowOpts {
            status: true,
            kind: true,
        },
    )
}

struct MediaListRowOpts {
    status: bool,
    kind: bool,
}

/// Compact one-line progress: 10 small dots, **10% each**.
const DOT_CELLS: usize = 10;

// Fixed metadata columns (aligned across rows).
const YEAR_COL: usize = 4; // "2013"
const SEASON_COL: usize = 3; // "S09"
const EP_COL: usize = 3; // "E07"
const COL_GAP: usize = 2;
/// Dense strip with no gaps between dots (`▪▪▪▫▫…` = 10 cells).
const DOT_STRIP_W: usize = DOT_CELLS;

/// Shared Home/Library list item — always **one line**.
///
/// ```text
/// Tv  Title…  2013  S09  E07  ▪▪▪▪▪▪▪▫▫▫  3d ago  70%
/// ```
fn media_list_item(
    item: &LibraryItem,
    theme: &Theme,
    inner_w: usize,
    opts: MediaListRowOpts,
) -> ListItem<'static> {
    let year = item
        .year
        .map(|y| format!("{y:<YEAR_COL$}"))
        .unwrap_or_else(|| format!("{:YEAR_COL$}", "—"));
    let season = item
        .last_season
        .map(|s| format!("S{s:02}"))
        .unwrap_or_else(|| "—".to_string());
    let season = format!("{season:<SEASON_COL$}");
    let episode = item
        .last_episode
        .map(|e| format!("E{e:02}"))
        .unwrap_or_else(|| "—".to_string());
    let episode = format!("{episode:<EP_COL$}");

    let frac = item.progress_fraction();
    let show_progress = item.duration_secs > 0;
    let age = relative_age(item.updated_at);
    let pct = if show_progress {
        format!("{:>3.0}%", frac * 100.0)
    } else {
        String::new()
    };

    let meta_cols_w = YEAR_COL + COL_GAP + SEASON_COL + COL_GAP + EP_COL;
    let trail_w = {
        let mut w = 0usize;
        if show_progress {
            w += DOT_STRIP_W;
        }
        if !age.is_empty() {
            if w > 0 {
                w += COL_GAP;
            }
            w += age.chars().count();
        }
        if !pct.is_empty() {
            if w > 0 {
                w += COL_GAP;
            }
            w += pct.chars().count();
        }
        w
    };

    const KIND_BADGE_W: usize = 7;
    let status_badge_w = if opts.status { 10 } else { 0 };
    let status_w = if opts.status { status_badge_w + 1 } else { 0 };
    let kind_w = if opts.kind { KIND_BADGE_W + 1 } else { 0 };
    let fixed = status_w + kind_w + meta_cols_w + COL_GAP + trail_w + COL_GAP;
    let title_budget = inner_w.saturating_sub(fixed).max(8);
    let title = truncate(&item.title, title_budget);
    let title_pad = title_budget.saturating_sub(title.chars().count());

    let col_style = Style::new().fg(theme.dim);
    let mut spans = Vec::new();
    if opts.status {
        spans.push(status_badge(item.status, theme));
        spans.push(Span::raw(" "));
    }
    if opts.kind {
        spans.push(kind_badge_span(item.kind, theme));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(title, Style::new().fg(theme.text)));
    if title_pad > 0 {
        spans.push(Span::raw(" ".repeat(title_pad)));
    }
    spans.push(Span::raw(" ".repeat(COL_GAP)));
    spans.push(Span::styled(year, col_style));
    spans.push(Span::raw(" ".repeat(COL_GAP)));
    spans.push(Span::styled(season, col_style));
    spans.push(Span::raw(" ".repeat(COL_GAP)));
    spans.push(Span::styled(episode, col_style));

    if show_progress {
        spans.push(Span::raw(" ".repeat(COL_GAP)));
        spans.extend(progress_dots_spans(frac, theme));
    }
    if !age.is_empty() {
        spans.push(Span::raw(" ".repeat(COL_GAP)));
        spans.push(Span::styled(age, Style::new().fg(theme.muted)));
    }
    if !pct.is_empty() {
        spans.push(Span::raw(" ".repeat(COL_GAP)));
        spans.push(Span::styled(pct, Style::new().fg(theme.dim)));
    }

    ListItem::new(Line::from(spans))
}

/// Small dense progress dots on one line: filled `▪`, empty `▫` (no gaps).
/// 10 cells → 10% each.
fn progress_dots_spans(frac: f32, theme: &Theme) -> Vec<Span<'static>> {
    let frac = frac.clamp(0.0, 1.0);
    let filled = ((frac * DOT_CELLS as f32).round() as usize).min(DOT_CELLS);
    let empty = DOT_CELLS.saturating_sub(filled);
    let mut spans = Vec::with_capacity(2);
    if filled > 0 {
        spans.push(Span::styled(
            "▪".repeat(filled),
            Style::new().fg(theme.accent),
        ));
    }
    if empty > 0 {
        spans.push(Span::styled(
            "▫".repeat(empty),
            Style::new().fg(theme.border),
        ));
    }
    spans
}

/// How many dots are filled for `frac` (for tests).
#[cfg(test)]
pub(super) fn progress_cells_filled(frac: f32) -> usize {
    ((frac.clamp(0.0, 1.0) * DOT_CELLS as f32).round() as usize).min(DOT_CELLS)
}

/// Display height of a home list row (for mouse hit-testing).
pub(super) fn home_row_height(_row: &HomeRow, _continue_watching: &[LibraryItem]) -> u16 {
    1
}

/// Display height of a library list row.
pub(super) fn library_row_height(_item: &LibraryItem) -> u16 {
    1
}

fn status_badge(status: ListStatus, theme: &Theme) -> Span<'static> {
    let (label, color) = match status {
        ListStatus::Watching => ("Watching", theme.success),
        ListStatus::Completed => ("Done", theme.accent),
        ListStatus::Planning => ("Planned", theme.warn),
        ListStatus::Paused => ("Paused", theme.dim),
        ListStatus::Dropped => ("Dropped", theme.danger),
        ListStatus::Repeating => ("Repeat", theme.badge_anime),
    };
    // Fixed width slot (10) so Library columns stay aligned.
    Span::styled(
        format!(" {label:<8} "),
        Style::new().fg(color).bg(theme.surface).bold(),
    )
}

fn kind_badge_span(kind: MediaKind, theme: &Theme) -> Span<'static> {
    let (label, color) = match kind {
        MediaKind::Movie => ("Movie", theme.badge_movie),
        MediaKind::Series => ("Tv", theme.badge_series),
        MediaKind::Anime => ("Anime", theme.badge_anime),
    };
    // Fixed 7-cell chip: outer pads + centered 5-wide label so short "Tv"
    // sits in the middle of the surface (not left-aligned empty purple).
    Span::styled(
        format!(" {label:^5} "),
        Style::new().fg(color).bg(theme.surface).bold(),
    )
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(app.theme.border_focus))
        .title(Span::styled(
            " Search ",
            Style::new().fg(app.theme.accent).bold(),
        ))
        .style(Style::new().bg(app.theme.bg));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Find ", Style::new().fg(app.theme.dim)),
            Span::styled("movies, series, and anime", Style::new().fg(app.theme.text)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("❯ ", Style::new().fg(app.theme.accent).bold()),
            Span::styled(app.query.clone(), Style::new().fg(app.theme.text)),
            Span::styled("█", Style::new().fg(app.theme.accent)),
        ]),
    ];
    if app.busy {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "{} Searching configured metadata providers…",
                app.spinner_frame()
            ),
            Style::new().fg(app.theme.dim),
        )));
    } else if app.query.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Type a title and press Enter. Esc returns to Home.",
            Style::new().fg(app.theme.dim),
        )));
        let suggestions: Vec<&str> = app
            .all_library
            .iter()
            .take(3)
            .map(|i| i.title.as_str())
            .collect();
        if !suggestions.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("From library: {}", suggestions.join(" · ")),
                Style::new().fg(app.theme.muted),
            )));
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_results(f: &mut Frame, app: &App, area: Rect) {
    let layout = results_layout(area);
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|m| {
            let year = m.year.map(|y| format!(" ({y})")).unwrap_or_default();
            ListItem::new(Line::from(vec![
                kind_badge_span(m.kind, &app.theme),
                Span::raw(" "),
                Span::styled(
                    format!("{}{year}", m.display_title()),
                    Style::new().fg(app.theme.text),
                ),
            ]))
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
    let block = panel_block("Details", &app.theme, false);
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
        Paragraph::new(
            media
                .map(|m| media_detail_lines(m, &app.theme))
                .unwrap_or_else(|| {
                    vec![Line::from(Span::styled(
                        "No result selected.",
                        Style::new().fg(app.theme.dim),
                    ))]
                }),
        )
        .wrap(Wrap { trim: true }),
        text_area,
    );
}

fn media_detail_lines(m: &Media, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        m.display_title().to_string(),
        Style::new().fg(theme.accent).bold(),
    ))];
    let mut facts = vec![m.kind.label().to_string()];
    if let Some(y) = m.year {
        facts.push(y.to_string());
    }
    if let Some(score) = m.score.filter(|s| *s > 0.0) {
        facts.push(format!("★ {score:.1}"));
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
        Style::new().fg(theme.dim),
    )));
    if !m.genres.is_empty() {
        lines.push(Line::from(Span::styled(
            m.genres.join(", "),
            Style::new().fg(theme.muted),
        )));
    }
    lines.push(Line::from(""));
    match &m.overview {
        Some(o) => push_multiline(&mut lines, o),
        None => lines.push(Line::from(Span::styled(
            "No overview available.",
            Style::new().fg(theme.dim),
        ))),
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
            ListItem::new(Line::from(vec![
                Span::styled(name, Style::new().fg(app.theme.text)),
                Span::styled(
                    format!("  · {} episodes", s.episode_count),
                    Style::new().fg(app.theme.dim),
                ),
            ]))
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
    let layout = episodes_layout(area);

    let items: Vec<ListItem> = app
        .episodes
        .iter()
        .map(|ep| ListItem::new(episode_row(ep, &app.theme)))
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

fn episode_row(ep: &Episode, theme: &Theme) -> Line<'static> {
    let num = Span::styled(
        format!("E{:02}", ep.number),
        Style::new().fg(theme.accent).bold(),
    );
    match &ep.title {
        Some(t) if !t.is_empty() => Line::from(vec![
            num,
            Span::styled(" · ", Style::new().fg(theme.muted)),
            Span::styled(t.clone(), Style::new().fg(theme.text)),
        ]),
        _ => match &ep.air_date {
            Some(d) if !d.is_empty() => Line::from(vec![
                num,
                Span::styled(format!("   ({d})"), Style::new().fg(theme.dim)),
            ]),
            _ => Line::from(num),
        },
    }
}

fn draw_episode_panel(f: &mut Frame, app: &App, area: Rect) {
    let sel = app.episodes_state.selected().unwrap_or(0);
    let ep = app.episodes.get(sel);

    let mut lines: Vec<Line> = Vec::new();
    let label = |s: &str| Span::styled(s.to_string(), Style::new().fg(app.theme.dim));

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

    match ep {
        Some(ep) => {
            let title = match &ep.title {
                Some(t) if !t.is_empty() => format!("E{:02} · {t}", ep.number),
                _ => format!("E{:02}", ep.number),
            };
            lines.push(Line::from(Span::styled(
                title,
                Style::new().fg(app.theme.text).bold(),
            )));

            let mut facts: Vec<String> = Vec::new();
            if let Some(d) = &ep.air_date {
                if !d.is_empty() {
                    facts.push(format!("Aired {d}"));
                }
            }
            if let Some(r) = ep.runtime_minutes {
                facts.push(format!("{r} min"));
            }
            if let Some(rt) = ep.rating.filter(|r| *r > 0.0) {
                facts.push(format!("★ {rt:.1}"));
            }
            if !facts.is_empty() {
                lines.push(Line::from(label(&facts.join("   "))));
            }

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

    let block = panel_block("Episode", &app.theme, false);
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

fn draw_image(f: &mut Frame, app: &App, area: Rect, url: Option<&str>) {
    if let Some(protocol) = url.and_then(|u| app.stills.ready(u)) {
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
        .map(|c| ListItem::new(source_row_line(c, &app.theme)))
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

/// One scannable line per candidate (used by tests as plain text).
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

fn source_row_line(c: &SourceCandidate, theme: &Theme) -> Line<'static> {
    let style = match seed_health(c.seeders) {
        SeedHealth::Hot => Style::new().fg(theme.success),
        SeedHealth::Ok => Style::new().fg(theme.warn),
        SeedHealth::Cold => Style::new().fg(theme.danger),
        SeedHealth::Unknown => Style::new().fg(theme.dim),
    };
    let name = c
        .title
        .lines()
        .next()
        .unwrap_or(&c.title)
        .trim()
        .to_string();
    let cache_style = match c.cache {
        CacheState::Cached => Style::new().fg(theme.success),
        CacheState::Uncached => Style::new().fg(theme.warn),
        CacheState::Unknown => Style::new().fg(theme.muted),
    };
    Line::from(vec![
        Span::styled(format!("{:<8} ", cache_badge(c.cache)), cache_style),
        Span::styled(
            format!("[{:<5}] ", c.quality.label()),
            Style::new().fg(theme.text).bold(),
        ),
        Span::styled(format!("{:<10} ", seed_badge(c.seeders)), style),
        Span::styled(
            format!("{:<12} ", truncate(&c.provider, 12)),
            Style::new().fg(theme.dim),
        ),
        Span::styled(
            format!("{:>9}  ", c.human_size()),
            Style::new().fg(theme.muted),
        ),
        Span::styled(name, Style::new().fg(theme.text)),
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

fn draw_help(f: &mut Frame, app: &App) {
    let area = help_layout(f.area());
    f.render_widget(Clear, area);
    let text = help_lines(app);
    f.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(app.theme.accent))
                    .title(Span::styled(
                        " Help ",
                        Style::new().fg(app.theme.accent).bold(),
                    ))
                    .style(Style::new().bg(app.theme.surface)),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn help_lines(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines = vec![
        Line::from(Span::styled(
            breadcrumbs(app),
            Style::new().fg(t.accent).bold(),
        )),
        Line::from(""),
        Line::from(Span::styled("Global", Style::new().fg(t.accent).bold())),
        help_kv("j/k · arrows", "move selection", t),
        help_kv("1 / 2 / 3", "Home / Library / Search tabs", t),
        help_kv("/", "jump to Search", t),
        help_kv("?", "toggle this help", t),
        help_kv("Esc", "go back (never quits)", t),
        help_kv("q · Ctrl-C", "quit", t),
        Line::from(""),
    ];
    match app.screen() {
        Screen::Home => {
            lines.push(Line::from(Span::styled(
                "Home",
                Style::new().fg(t.accent).bold(),
            )));
            lines.push(help_kv("Enter", "resume a title or run a quick action", t));
        }
        Screen::Library => {
            lines.push(Line::from(Span::styled(
                "Library",
                Style::new().fg(t.accent).bold(),
            )));
            lines.push(help_kv("h/l · ←/→", "cycle status filters", t));
            lines.push(help_kv("Enter", "resume when possible, else open title", t));
        }
        Screen::Sources => {
            lines.push(Line::from(Span::styled(
                "Sources",
                Style::new().fg(t.accent).bold(),
            )));
            lines.push(help_kv("Tab", "toggle list ↔ filter panel", t));
            lines.push(help_kv("h/l in filters", "change value · Enter applies", t));
        }
        _ => {
            lines.push(Line::from(Span::styled(
                "Browse",
                Style::new().fg(t.accent).bold(),
            )));
            lines.push(help_kv(
                "Enter",
                "follow result / season / episode / source",
                t,
            ));
        }
    }
    lines
}

fn help_kv(key: &str, desc: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {key:<18}"), Style::new().fg(theme.text).bold()),
        Span::styled(desc.to_string(), Style::new().fg(theme.dim)),
    ])
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
                .border_type(if panel_focused {
                    BorderType::Rounded
                } else {
                    BorderType::Plain
                })
                .border_style(Style::new().fg(border))
                .title(" Filters / Sort ")
                .style(Style::new().bg(app.theme.bg)),
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
            .block(panel_block("Details", &app.theme, false))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn panel_block(title: &str, theme: &Theme, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Rounded
        } else {
            BorderType::Plain
        })
        .border_style(Style::new().fg(theme.border(focused)))
        .title(format!(" {title} "))
        .style(Style::new().bg(theme.bg))
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
                .border_type(if focused {
                    BorderType::Rounded
                } else {
                    BorderType::Plain
                })
                .border_style(Style::new().fg(border))
                .title(format!(" {title} "))
                .style(Style::new().bg(theme.bg)),
        )
        .highlight_style(theme.highlight())
        .highlight_symbol("▶ ");
    let mut s = *state;
    f.render_stateful_widget(list, area, &mut s);
}
