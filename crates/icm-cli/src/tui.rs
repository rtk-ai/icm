//! Interactive TUI dashboard for ICM metrics and memory browsing.

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::CrosstermBackend,
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
        Tabs, Wrap,
    },
    Frame, Terminal,
};

use icm_core::{
    FeedbackStore, Importance, MemoirStore, Memory, MemoryStore, StoreStats, TopicHealth,
};
use icm_store::SqliteStore;

/// Tab indices
const TAB_OVERVIEW: usize = 0;
const TAB_TOPICS: usize = 1;
const TAB_MEMORIES: usize = 2;
const TAB_HEALTH: usize = 3;
const TAB_MEMOIRS: usize = 4;

/// Application state
struct App {
    /// Active tab
    tab: usize,
    /// Should quit
    quit: bool,
    /// Global stats (cached)
    stats: StoreStats,
    /// Topics with counts
    topics: Vec<(String, usize)>,
    /// Topic list selection state
    topic_state: ListState,
    /// Selected topic's memories
    memories: Vec<Memory>,
    /// Memory list selection state
    memory_state: ListState,
    /// Memory scroll offset for detail view
    memory_scroll: u16,
    /// Health reports
    health: Vec<TopicHealth>,
    /// Health table selection state
    health_state: TableState,
    /// Memoir names
    memoirs: Vec<(String, String, usize, usize)>, // (name, desc, concepts, links)
    /// Memoir table state
    memoir_state: TableState,
    /// DB file size in bytes
    db_size: u64,
    /// DB path for display
    db_path_display: String,
    /// Total feedback count
    feedback_count: usize,
    /// Search mode active
    search_mode: bool,
    /// Search input buffer
    search_input: String,
    /// Search results
    search_results: Vec<Memory>,
    /// Search result list state
    search_state: ListState,
    /// Last refresh time
    last_refresh: Instant,
}

impl App {
    fn new(store: &SqliteStore, db_path: Option<&str>) -> Result<Self> {
        let stats = store.stats()?;
        let topics = store.list_topics()?;
        let health = Self::load_health(store, &topics)?;
        let memoirs = Self::load_memoirs(store)?;
        let feedback_count = store.feedback_stats().map(|s| s.total).unwrap_or(0);
        let db_size = db_path
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0);

        let mut topic_state = ListState::default();
        if !topics.is_empty() {
            topic_state.select(Some(0));
        }

        let mut app = Self {
            tab: TAB_OVERVIEW,
            quit: false,
            stats,
            topics,
            topic_state,
            memories: Vec::new(),
            memory_state: ListState::default(),
            memory_scroll: 0,
            health,
            health_state: TableState::default(),
            memoirs,
            memoir_state: TableState::default(),
            db_size,
            db_path_display: db_path.unwrap_or("in-memory").to_string(),
            feedback_count,
            search_mode: false,
            search_input: String::new(),
            search_results: Vec::new(),
            search_state: ListState::default(),
            last_refresh: Instant::now(),
        };

        // Load memories for the first topic
        app.load_topic_memories(store);

        Ok(app)
    }

    fn load_health(store: &SqliteStore, topics: &[(String, usize)]) -> Result<Vec<TopicHealth>> {
        let mut health = Vec::new();
        for (topic, _) in topics {
            if let Ok(h) = store.topic_health(topic) {
                health.push(h);
            }
        }
        Ok(health)
    }

    fn load_memoirs(store: &SqliteStore) -> Result<Vec<(String, String, usize, usize)>> {
        let memoirs = store.list_memoirs()?;
        let mut result = Vec::new();
        for m in memoirs {
            let stats = store.memoir_stats(&m.id).unwrap_or_default();
            result.push((
                m.name,
                m.description,
                stats.total_concepts,
                stats.total_links,
            ));
        }
        Ok(result)
    }

    fn refresh(&mut self, store: &SqliteStore, db_path: Option<&str>) {
        if let Ok(s) = store.stats() {
            self.stats = s;
        }
        if let Ok(t) = store.list_topics() {
            self.topics = t;
        }
        if let Ok(h) = Self::load_health(store, &self.topics) {
            self.health = h;
        }
        if let Ok(m) = Self::load_memoirs(store) {
            self.memoirs = m;
        }
        self.feedback_count = store.feedback_stats().map(|s| s.total).unwrap_or(0);
        self.db_size = db_path
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0);
        self.last_refresh = Instant::now();
    }

    fn load_topic_memories(&mut self, store: &SqliteStore) {
        if let Some(idx) = self.topic_state.selected() {
            if let Some((topic, _)) = self.topics.get(idx) {
                if let Ok(mut mems) = store.get_by_topic(topic) {
                    mems.sort_by(|a, b| {
                        b.weight
                            .partial_cmp(&a.weight)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    self.memories = mems;
                    self.memory_state = ListState::default();
                    if !self.memories.is_empty() {
                        self.memory_state.select(Some(0));
                    }
                }
            }
        }
    }

    fn selected_topic_name(&self) -> Option<&str> {
        self.topic_state
            .selected()
            .and_then(|i| self.topics.get(i))
            .map(|(name, _)| name.as_str())
    }

    fn next_tab(&mut self) {
        self.tab = (self.tab + 1) % 5;
    }

    fn prev_tab(&mut self) {
        self.tab = if self.tab == 0 { 4 } else { self.tab - 1 };
    }

    fn select_next(selected: Option<usize>, len: usize) -> Option<usize> {
        if len == 0 {
            return None;
        }
        Some(selected.map(|i| (i + 1).min(len - 1)).unwrap_or(0))
    }

    fn select_prev(selected: Option<usize>) -> Option<usize> {
        Some(selected.map(|i| i.saturating_sub(1)).unwrap_or(0))
    }
}

/// Entry point for the TUI dashboard.
pub fn run_dashboard(store: &SqliteStore, db_path: Option<&str>) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(store, db_path)?;

    let result = run_loop(&mut terminal, &mut app, store, db_path);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    store: &SqliteStore,
    db_path: Option<&str>,
) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, app))?;

        // Poll events with 250ms timeout for auto-refresh
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                // Search mode input handling
                if app.search_mode {
                    match key.code {
                        KeyCode::Esc => {
                            app.search_mode = false;
                            app.search_input.clear();
                            app.search_results.clear();
                        }
                        KeyCode::Enter => {
                            if !app.search_input.is_empty() {
                                if let Ok(results) = store.search_fts(&app.search_input, 20) {
                                    app.search_results = results;
                                    app.search_state = ListState::default();
                                    if !app.search_results.is_empty() {
                                        app.search_state.select(Some(0));
                                    }
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            app.search_input.pop();
                        }
                        KeyCode::Down => {
                            let sel = App::select_next(
                                app.search_state.selected(),
                                app.search_results.len(),
                            );
                            app.search_state.select(sel);
                        }
                        KeyCode::Up => {
                            let sel = App::select_prev(app.search_state.selected());
                            app.search_state.select(sel);
                        }
                        KeyCode::Char(c) => {
                            app.search_input.push(c);
                        }
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.quit = true
                    }
                    // Tab navigation
                    KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => app.next_tab(),
                    KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => app.prev_tab(),
                    KeyCode::Char('1') => app.tab = TAB_OVERVIEW,
                    KeyCode::Char('2') => app.tab = TAB_TOPICS,
                    KeyCode::Char('3') => app.tab = TAB_MEMORIES,
                    KeyCode::Char('4') => app.tab = TAB_HEALTH,
                    KeyCode::Char('5') => app.tab = TAB_MEMOIRS,
                    // List navigation
                    KeyCode::Down | KeyCode::Char('j') => match app.tab {
                        TAB_TOPICS => {
                            let sel =
                                App::select_next(app.topic_state.selected(), app.topics.len());
                            app.topic_state.select(sel);
                            app.load_topic_memories(store);
                        }
                        TAB_MEMORIES => {
                            let sel =
                                App::select_next(app.memory_state.selected(), app.memories.len());
                            app.memory_state.select(sel);
                            app.memory_scroll = 0;
                        }
                        TAB_HEALTH => {
                            let sel =
                                App::select_next(app.health_state.selected(), app.health.len());
                            app.health_state.select(sel);
                        }
                        TAB_MEMOIRS => {
                            let sel =
                                App::select_next(app.memoir_state.selected(), app.memoirs.len());
                            app.memoir_state.select(sel);
                        }
                        _ => {}
                    },
                    KeyCode::Up | KeyCode::Char('k') => match app.tab {
                        TAB_TOPICS => {
                            let sel = App::select_prev(app.topic_state.selected());
                            app.topic_state.select(sel);
                            app.load_topic_memories(store);
                        }
                        TAB_MEMORIES => {
                            let sel = App::select_prev(app.memory_state.selected());
                            app.memory_state.select(sel);
                            app.memory_scroll = 0;
                        }
                        TAB_HEALTH => {
                            let sel = App::select_prev(app.health_state.selected());
                            app.health_state.select(sel);
                        }
                        TAB_MEMOIRS => {
                            let sel = App::select_prev(app.memoir_state.selected());
                            app.memoir_state.select(sel);
                        }
                        _ => {}
                    },
                    // Page up/down for memory detail scroll
                    KeyCode::PageDown => app.memory_scroll = app.memory_scroll.saturating_add(5),
                    KeyCode::PageUp => app.memory_scroll = app.memory_scroll.saturating_sub(5),
                    // Jump to top/bottom
                    KeyCode::Char('g') => match app.tab {
                        TAB_TOPICS if !app.topics.is_empty() => {
                            app.topic_state.select(Some(0));
                            app.load_topic_memories(store);
                        }
                        TAB_MEMORIES if !app.memories.is_empty() => {
                            app.memory_state.select(Some(0));
                        }
                        TAB_HEALTH if !app.health.is_empty() => {
                            app.health_state.select(Some(0));
                        }
                        _ => {}
                    },
                    KeyCode::Char('G') => match app.tab {
                        TAB_TOPICS if !app.topics.is_empty() => {
                            app.topic_state.select(Some(app.topics.len() - 1));
                            app.load_topic_memories(store);
                        }
                        TAB_MEMORIES if !app.memories.is_empty() => {
                            app.memory_state.select(Some(app.memories.len() - 1));
                        }
                        TAB_HEALTH if !app.health.is_empty() => {
                            app.health_state.select(Some(app.health.len() - 1));
                        }
                        _ => {}
                    },
                    // Enter on topics tab → switch to memories for that topic
                    KeyCode::Enter => {
                        if app.tab == TAB_TOPICS {
                            app.load_topic_memories(store);
                            app.tab = TAB_MEMORIES;
                        }
                    }
                    // Search
                    KeyCode::Char('/') => {
                        app.search_mode = true;
                        app.search_input.clear();
                        app.search_results.clear();
                    }
                    // Refresh
                    KeyCode::Char('r') => app.refresh(store, db_path),
                    _ => {}
                }
            }
        }

        // Auto-refresh every 30s
        if app.last_refresh.elapsed() > Duration::from_secs(30) {
            app.refresh(store, db_path);
        }

        if app.quit {
            break;
        }
    }
    Ok(())
}

fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Tabs
            Constraint::Min(0),    // Content
            Constraint::Length(1), // Status bar
        ])
        .split(f.area());

    draw_tabs(f, app, chunks[0]);

    match app.tab {
        TAB_OVERVIEW => draw_overview(f, app, chunks[1]),
        TAB_TOPICS => draw_topics(f, app, chunks[1]),
        TAB_MEMORIES => draw_memories(f, app, chunks[1]),
        TAB_HEALTH => draw_health(f, app, chunks[1]),
        TAB_MEMOIRS => draw_memoirs(f, app, chunks[1]),
        _ => {}
    }

    draw_status_bar(f, app, chunks[2]);

    // Search overlay
    if app.search_mode {
        draw_search_overlay(f, app);
    }
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let titles = vec!["Overview", "Topics", "Memories", "Health", "Memoirs"];
    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" ICM Dashboard "),
        )
        .select(app.tab)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(Style::default().fg(Color::Yellow).bold())
        .divider(Span::raw(" | "));
    f.render_widget(tabs, area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let help = if app.search_mode {
        " ESC: cancel | ENTER: search | Type query..."
    } else {
        " q: quit | Tab/1-5: switch tab | j/k: navigate | /: search | r: refresh | Enter: select"
    };
    let bar = Paragraph::new(help).style(Style::default().fg(Color::DarkGray).bg(Color::Black));
    f.render_widget(bar, area);
}

fn draw_overview(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10), // Stats box
            Constraint::Length(10), // Top topics
            Constraint::Min(0),     // Recent activity
        ])
        .split(area);

    // Stats panel
    let db_str = format_size(app.db_size);
    let oldest = app
        .stats
        .oldest_memory
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "—".into());
    let newest = app
        .stats
        .newest_memory
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "—".into());

    let memoir_count = app.memoirs.len();
    let total_concepts: usize = app.memoirs.iter().map(|(_, _, c, _)| c).sum();
    let total_links: usize = app.memoirs.iter().map(|(_, _, _, l)| l).sum();

    let stats_text = vec![
        Line::from(vec![
            Span::styled("  Memories:  ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", app.stats.total_memories)),
            Span::raw("    "),
            Span::styled("Topics:  ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", app.stats.total_topics)),
            Span::raw("    "),
            Span::styled("Avg weight:  ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{:.2}", app.stats.avg_weight)),
        ]),
        Line::from(vec![
            Span::styled("  Memoirs:   ", Style::default().fg(Color::Magenta)),
            Span::raw(format!("{memoir_count}")),
            Span::raw("    "),
            Span::styled("Concepts: ", Style::default().fg(Color::Magenta)),
            Span::raw(format!("{total_concepts}")),
            Span::raw("    "),
            Span::styled("Links: ", Style::default().fg(Color::Magenta)),
            Span::raw(format!("{total_links}")),
        ]),
        Line::from(vec![
            Span::styled("  Feedback:  ", Style::default().fg(Color::Green)),
            Span::raw(format!("{}", app.feedback_count)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  DB size:   ", Style::default().fg(Color::DarkGray)),
            Span::raw(db_str),
            Span::raw("    "),
            Span::styled("Range: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{oldest} → {newest}")),
        ]),
        Line::from(vec![
            Span::styled("  DB path:   ", Style::default().fg(Color::DarkGray)),
            Span::raw(app.db_path_display.clone()),
        ]),
    ];

    let stats_block = Paragraph::new(stats_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Global Stats ")
            .title_style(Style::default().fg(Color::Yellow).bold()),
    );
    f.render_widget(stats_block, chunks[0]);

    // Top topics by count
    let mut sorted_topics = app.topics.clone();
    sorted_topics.sort_by(|a, b| b.1.cmp(&a.1));
    sorted_topics.truncate(8);

    let topic_rows: Vec<Row> = sorted_topics
        .iter()
        .map(|(name, count)| {
            let bar = "█".repeat((*count).min(30));
            Row::new(vec![
                Cell::from(name.as_str()).style(Style::default().fg(Color::Cyan)),
                Cell::from(format!("{count:>4}")),
                Cell::from(bar).style(Style::default().fg(Color::Blue)),
            ])
        })
        .collect();

    let topic_table = Table::new(
        topic_rows,
        [
            Constraint::Length(30),
            Constraint::Length(5),
            Constraint::Min(10),
        ],
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Top Topics ")
            .title_style(Style::default().fg(Color::Yellow).bold()),
    );
    f.render_widget(topic_table, chunks[1]);

    // Importance distribution
    let health_summary = importance_distribution(&app.health);
    let dist_block = Paragraph::new(health_summary).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Health Summary ")
            .title_style(Style::default().fg(Color::Yellow).bold()),
    );
    f.render_widget(dist_block, chunks[2]);
}

fn draw_topics(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    // Topics list
    let items: Vec<ListItem> = app
        .topics
        .iter()
        .map(|(name, count)| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{name:<30}"), Style::default().fg(Color::Cyan)),
                Span::raw(format!(" ({count})")),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Topics ({}) ", app.topics.len()))
                .title_style(Style::default().fg(Color::Yellow).bold()),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, chunks[0], &mut app.topic_state);

    // Topic detail (right panel)
    let detail = if let Some(idx) = app.topic_state.selected() {
        if let Some(health) = app.health.get(idx) {
            topic_detail_text(health)
        } else {
            vec![Line::from("  No health data")]
        }
    } else {
        vec![Line::from("  Select a topic")]
    };

    let detail_block = Paragraph::new(detail).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Topic Detail ")
            .title_style(Style::default().fg(Color::Yellow).bold()),
    );
    f.render_widget(detail_block, chunks[1]);
}

fn draw_memories(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let topic_label = app.selected_topic_name().unwrap_or("all").to_string();

    // Memory list
    let items: Vec<ListItem> = app
        .memories
        .iter()
        .map(|m| {
            let imp_color = importance_color(&m.importance);
            let summary = if m.summary.len() > 50 {
                format!("{}...", &m.summary[..47])
            } else {
                m.summary.clone()
            };
            let bar = weight_bar(m.weight, 5);
            ListItem::new(Line::from(vec![
                Span::styled(bar, Style::default().fg(weight_color(m.weight))),
                Span::raw(" "),
                Span::styled("● ", Style::default().fg(imp_color)),
                Span::raw(summary),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " Memories — {} ({}) ",
                    topic_label,
                    app.memories.len()
                ))
                .title_style(Style::default().fg(Color::Yellow).bold()),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, chunks[0], &mut app.memory_state);

    // Memory detail
    let detail = if let Some(idx) = app.memory_state.selected() {
        if let Some(mem) = app.memories.get(idx) {
            memory_detail_text(mem)
        } else {
            vec![Line::from("  No memory selected")]
        }
    } else {
        vec![Line::from("  Select a memory (j/k to navigate)")]
    };

    let detail_block = Paragraph::new(detail)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Memory Detail ")
                .title_style(Style::default().fg(Color::Yellow).bold()),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.memory_scroll, 0));
    f.render_widget(detail_block, chunks[1]);
}

fn draw_health(f: &mut Frame, app: &mut App, area: Rect) {
    let header = Row::new(vec![
        Cell::from("Topic").style(Style::default().bold()),
        Cell::from("Count").style(Style::default().bold()),
        Cell::from("Avg Wt").style(Style::default().bold()),
        Cell::from("Stale").style(Style::default().bold()),
        Cell::from("Consol?").style(Style::default().bold()),
        Cell::from("Last Access").style(Style::default().bold()),
    ])
    .height(1)
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .health
        .iter()
        .map(|h| {
            let consol = if h.needs_consolidation {
                Span::styled("YES", Style::default().fg(Color::Red).bold())
            } else {
                Span::styled("no", Style::default().fg(Color::Green))
            };
            let stale_style = if h.stale_count > 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Green)
            };
            let last_access = h
                .last_accessed
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "—".into());

            Row::new(vec![
                Cell::from(h.topic.as_str()).style(Style::default().fg(Color::Cyan)),
                Cell::from(format!("{}", h.entry_count)),
                Cell::from(format!("{:.2}", h.avg_weight)),
                Cell::from(format!("{}", h.stale_count)).style(stale_style),
                Cell::from(consol),
                Cell::from(last_access),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(30),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Min(16),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Health Report ({} topics) ", app.health.len()))
            .title_style(Style::default().fg(Color::Yellow).bold()),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    f.render_stateful_widget(table, area, &mut app.health_state);
}

fn draw_memoirs(f: &mut Frame, app: &mut App, area: Rect) {
    if app.memoirs.is_empty() {
        let empty = Paragraph::new(
            "  No memoirs found. Create one with: icm memoir create -n <name> -d <description>",
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Memoirs ")
                .title_style(Style::default().fg(Color::Yellow).bold()),
        );
        f.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Name").style(Style::default().bold()),
        Cell::from("Description").style(Style::default().bold()),
        Cell::from("Concepts").style(Style::default().bold()),
        Cell::from("Links").style(Style::default().bold()),
    ])
    .height(1)
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .memoirs
        .iter()
        .map(|(name, desc, concepts, links)| {
            let desc_short = if desc.len() > 40 {
                format!("{}...", &desc[..37])
            } else {
                desc.clone()
            };
            Row::new(vec![
                Cell::from(name.as_str()).style(Style::default().fg(Color::Magenta)),
                Cell::from(desc_short),
                Cell::from(format!("{concepts}")),
                Cell::from(format!("{links}")),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(25),
            Constraint::Min(20),
            Constraint::Length(9),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Memoirs ({}) ", app.memoirs.len()))
            .title_style(Style::default().fg(Color::Yellow).bold()),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    f.render_stateful_widget(table, area, &mut app.memoir_state);
}

fn draw_search_overlay(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let overlay_height = (area.height / 2).max(10);
    let overlay = Rect {
        x: area.width / 6,
        y: area.height / 4,
        width: area.width * 2 / 3,
        height: overlay_height,
    };

    f.render_widget(Clear, overlay);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(overlay);

    // Search input
    let input = Paragraph::new(Line::from(vec![
        Span::styled(" > ", Style::default().fg(Color::Yellow)),
        Span::raw(&app.search_input),
        Span::styled("_", Style::default().fg(Color::Yellow)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Search (Enter to search, Esc to cancel) ")
            .title_style(Style::default().fg(Color::Yellow).bold()),
    );
    f.render_widget(input, chunks[0]);

    // Results
    let items: Vec<ListItem> = app
        .search_results
        .iter()
        .map(|m| {
            let summary = if m.summary.len() > 80 {
                format!("{}...", &m.summary[..77])
            } else {
                m.summary.clone()
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("[{}] ", m.topic), Style::default().fg(Color::Cyan)),
                Span::raw(summary),
            ]))
        })
        .collect();

    let results = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Results ({}) ", app.search_results.len())),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(results, chunks[1], &mut app.search_state);
}

// --- Helper functions ---

fn weight_bar(weight: f32, width: usize) -> String {
    let filled = ((weight.clamp(0.0, 1.0)) * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn weight_color(weight: f32) -> Color {
    if weight >= 0.8 {
        Color::Green
    } else if weight >= 0.5 {
        Color::Yellow
    } else if weight >= 0.3 {
        Color::Rgb(255, 165, 0) // Orange
    } else {
        Color::Red
    }
}

fn importance_color(imp: &Importance) -> Color {
    match imp {
        Importance::Critical => Color::Red,
        Importance::High => Color::Yellow,
        Importance::Medium => Color::Green,
        Importance::Low => Color::DarkGray,
    }
}

fn importance_label(imp: &Importance) -> &'static str {
    match imp {
        Importance::Critical => "CRITICAL",
        Importance::High => "HIGH",
        Importance::Medium => "MEDIUM",
        Importance::Low => "LOW",
    }
}

fn format_size(bytes: u64) -> String {
    if bytes == 0 {
        return "—".into();
    }
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn topic_detail_text(h: &TopicHealth) -> Vec<Line<'static>> {
    let consol_status = if h.needs_consolidation {
        Span::styled(
            "NEEDS CONSOLIDATION",
            Style::default().fg(Color::Red).bold(),
        )
    } else {
        Span::styled("OK", Style::default().fg(Color::Green))
    };

    let oldest = h
        .oldest
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "—".into());
    let newest = h
        .newest
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "—".into());
    let last_access = h
        .last_accessed
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "—".into());

    vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Topic:         ", Style::default().fg(Color::Cyan).bold()),
            Span::raw(h.topic.clone()),
        ]),
        Line::from(vec![
            Span::styled("  Entries:       ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", h.entry_count)),
        ]),
        Line::from(vec![
            Span::styled("  Avg weight:    ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{:.3}", h.avg_weight)),
        ]),
        Line::from(vec![
            Span::styled("  Avg accesses:  ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{:.1}", h.avg_access_count)),
        ]),
        Line::from(vec![
            Span::styled("  Stale entries: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", h.stale_count)),
        ]),
        Line::from(vec![
            Span::styled("  Consolidation: ", Style::default().fg(Color::Cyan)),
            consol_status,
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Oldest:        ", Style::default().fg(Color::DarkGray)),
            Span::raw(oldest),
        ]),
        Line::from(vec![
            Span::styled("  Newest:        ", Style::default().fg(Color::DarkGray)),
            Span::raw(newest),
        ]),
        Line::from(vec![
            Span::styled("  Last accessed: ", Style::default().fg(Color::DarkGray)),
            Span::raw(last_access),
        ]),
    ]
}

fn memory_detail_text(m: &Memory) -> Vec<Line<'static>> {
    let imp_color = importance_color(&m.importance);
    let keywords = if m.keywords.is_empty() {
        "—".to_string()
    } else {
        m.keywords.join(", ")
    };

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  ID:          ", Style::default().fg(Color::DarkGray)),
            Span::raw(m.id.clone()),
        ]),
        Line::from(vec![
            Span::styled("  Topic:       ", Style::default().fg(Color::Cyan)),
            Span::raw(m.topic.clone()),
        ]),
        Line::from(vec![
            Span::styled("  Importance:  ", Style::default().fg(Color::Cyan)),
            Span::styled(
                importance_label(&m.importance).to_string(),
                Style::default().fg(imp_color).bold(),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Weight:      ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{:.4}", m.weight)),
        ]),
        Line::from(vec![
            Span::styled("  Accesses:    ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", m.access_count)),
        ]),
        Line::from(vec![
            Span::styled("  Created:     ", Style::default().fg(Color::DarkGray)),
            Span::raw(m.created_at.format("%Y-%m-%d %H:%M").to_string()),
        ]),
        Line::from(vec![
            Span::styled("  Updated:     ", Style::default().fg(Color::DarkGray)),
            Span::raw(m.updated_at.format("%Y-%m-%d %H:%M").to_string()),
        ]),
        Line::from(vec![
            Span::styled("  Accessed:    ", Style::default().fg(Color::DarkGray)),
            Span::raw(m.last_accessed.format("%Y-%m-%d %H:%M").to_string()),
        ]),
        Line::from(vec![
            Span::styled("  Keywords:    ", Style::default().fg(Color::Cyan)),
            Span::raw(keywords),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Content:",
            Style::default().fg(Color::Yellow).bold(),
        )),
        Line::from(""),
    ];

    // Wrap summary text
    for line in m.summary.lines() {
        lines.push(Line::from(format!("  {line}")));
    }

    // Raw excerpt if present
    if let Some(ref raw) = m.raw_excerpt {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Raw excerpt:",
            Style::default().fg(Color::Yellow).bold(),
        )));
        lines.push(Line::from(""));
        for line in raw.lines() {
            lines.push(Line::from(format!("  {line}")));
        }
    }

    lines
}

fn importance_distribution(health: &[TopicHealth]) -> Vec<Line<'static>> {
    if health.is_empty() {
        return vec![Line::from("  No data")];
    }

    let total_entries: usize = health.iter().map(|h| h.entry_count).sum();
    let total_stale: usize = health.iter().map(|h| h.stale_count).sum();
    let needs_consol = health.iter().filter(|h| h.needs_consolidation).count();
    let avg_weight: f32 = health.iter().map(|h| h.avg_weight).sum::<f32>() / health.len() as f32;

    let healthy_pct = if total_entries > 0 {
        ((total_entries - total_stale) as f32 / total_entries as f32 * 100.0) as u32
    } else {
        100
    };

    let bar_width = 30;
    let filled = (healthy_pct as usize * bar_width) / 100;

    let health_color = if healthy_pct >= 80 {
        Color::Green
    } else if healthy_pct >= 50 {
        Color::Yellow
    } else {
        Color::Red
    };

    vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Total entries:     ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{total_entries}")),
            Span::raw("    "),
            Span::styled("Stale: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{total_stale}")),
            Span::raw("    "),
            Span::styled("Need consolidation: ", Style::default().fg(Color::Red)),
            Span::raw(format!("{needs_consol}")),
        ]),
        Line::from(vec![
            Span::styled("  Avg weight:        ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{avg_weight:.3}")),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Health: ", Style::default().fg(Color::Cyan).bold()),
            Span::styled(
                format!("[{}{}]", "█".repeat(filled), "░".repeat(bar_width - filled)),
                Style::default().fg(health_color),
            ),
            Span::raw(format!(" {healthy_pct}%")),
        ]),
    ]
}
