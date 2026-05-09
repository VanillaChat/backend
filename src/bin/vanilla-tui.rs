use std::{
    collections::HashMap,
    io,
    time::{Duration, Instant},
};

use anyhow::Context;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table, Tabs, Wrap},
};
use redis::AsyncCommands;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::{net::TcpStream, time::timeout};
use tokio_postgres::NoTls;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Overview,
    Gateway,
    Guilds,
    Users,
    Events,
    Env,
    Actions,
}

impl View {
    const ALL: [View; 7] = [
        View::Overview,
        View::Gateway,
        View::Guilds,
        View::Users,
        View::Events,
        View::Env,
        View::Actions,
    ];

    fn title(self) -> &'static str {
        match self {
            View::Overview => "Overview",
            View::Gateway => "Gateway",
            View::Guilds => "Guilds",
            View::Users => "Users",
            View::Events => "Events",
            View::Env => "Env",
            View::Actions => "Actions",
        }
    }
}

#[derive(Clone, Debug)]
struct ServiceStatus {
    name: &'static str,
    state: String,
    detail: String,
    ok: bool,
}

#[derive(Clone, Debug, Default)]
struct Counts {
    users: i64,
    guilds: i64,
    channels: i64,
    guild_members: i64,
    messages: i64,
    invites: i64,
}

#[derive(Clone, Debug, Default)]
struct GuildRow {
    id: String,
    name: String,
    members: i64,
    channels: i64,
    messages: i64,
}

#[derive(Clone, Debug, Default)]
struct UserRow {
    id: String,
    username: String,
    tag: String,
    status: String,
    guilds: i64,
    messages: i64,
}

#[derive(Clone, Debug, Default)]
struct EventRow {
    name: String,
    volume: String,
    source: String,
    risk: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GatewayMetrics {
    connected_sockets: u64,
    identified_users: usize,
    subscribed_rooms: usize,
    total_inbound: u64,
    total_dispatches: u64,
    total_broadcasts: u64,
    inbound_by_op: Vec<GatewayMetricEntry>,
    dispatch_by_type: Vec<GatewayMetricEntry>,
    broadcasts_by_room: Vec<GatewayMetricEntry>,
    recent: Vec<GatewayRecentEvent>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct GatewayMetricEntry {
    key: String,
    count: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct GatewayRecentEvent {
    at_ms: u128,
    kind: String,
    detail: String,
}

#[derive(Clone, Debug)]
struct Snapshot {
    refreshed_at: Instant,
    services: Vec<ServiceStatus>,
    counts: Counts,
    guilds: Vec<GuildRow>,
    users: Vec<UserRow>,
    events: Vec<EventRow>,
    gateway_metrics: Option<GatewayMetrics>,
    env: Vec<(String, String)>,
    last_error: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            refreshed_at: Instant::now(),
            services: Vec::new(),
            counts: Counts::default(),
            guilds: Vec::new(),
            users: Vec::new(),
            events: default_events(),
            gateway_metrics: None,
            env: Vec::new(),
            last_error: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputMode {
    Normal,
    Search,
    Command,
}

struct App {
    view: View,
    snapshot: Snapshot,
    search: String,
    command: String,
    mode: InputMode,
    status: String,
    selected: usize,
    should_quit: bool,
}

impl App {
    fn new(snapshot: Snapshot) -> Self {
        Self {
            view: View::Overview,
            snapshot,
            search: String::new(),
            command: String::new(),
            mode: InputMode::Normal,
            status: "r refresh | / search | : command | tab switch | q quit".to_string(),
            selected: 0,
            should_quit: false,
        }
    }

    fn next_view(&mut self) {
        let idx = View::ALL.iter().position(|v| *v == self.view).unwrap_or(0);
        self.view = View::ALL[(idx + 1) % View::ALL.len()];
        self.selected = 0;
    }

    fn prev_view(&mut self) {
        let idx = View::ALL.iter().position(|v| *v == self.view).unwrap_or(0);
        self.view = View::ALL[(idx + View::ALL.len() - 1) % View::ALL.len()];
        self.selected = 0;
    }

    fn apply_command(&mut self) {
        let command = self.command.trim().to_string();
        self.command.clear();
        self.mode = InputMode::Normal;

        match command.as_str() {
            "" => {}
            "q" | "quit" | "exit" => self.should_quit = true,
            "r" | "refresh" => self.status = "refresh requested".to_string(),
            "overview" | "o" => self.view = View::Overview,
            "gateway" | "gw" => self.view = View::Gateway,
            "guilds" | "g" => self.view = View::Guilds,
            "users" | "u" => self.view = View::Users,
            "events" | "e" => self.view = View::Events,
            "env" => self.view = View::Env,
            "actions" | "a" => self.view = View::Actions,
            _ if command.starts_with("find ") => {
                self.search = command["find ".len()..].trim().to_string();
                self.status = format!("searching for '{}'", self.search);
            }
            _ => {
                self.status = format!("unknown command: {}", command);
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let snapshot = collect_snapshot().await.unwrap_or_else(|e| {
        let mut snapshot = Snapshot::default();
        snapshot.last_error = Some(e.to_string());
        snapshot.env = read_env();
        snapshot
    });

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_app(&mut terminal, App::new(snapshot)).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut app: App,
) -> anyhow::Result<()> {
    let mut last_refresh = Instant::now();

    loop {
        terminal.draw(|frame| render(frame, &app))?;

        if app.should_quit {
            return Ok(());
        }

        if last_refresh.elapsed() >= Duration::from_secs(5) {
            match collect_snapshot().await {
                Ok(snapshot) => {
                    app.snapshot = snapshot;
                    app.status = "auto-refreshed".to_string();
                }
                Err(e) => app.status = format!("refresh failed: {}", e),
            }
            last_refresh = Instant::now();
        }

        if event::poll(Duration::from_millis(120))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(&mut app, key).await;
                    if app.status == "refresh requested" {
                        match collect_snapshot().await {
                            Ok(snapshot) => {
                                app.snapshot = snapshot;
                                app.status = "refreshed".to_string();
                            }
                            Err(e) => app.status = format!("refresh failed: {}", e),
                        }
                        last_refresh = Instant::now();
                    }
                }
            }
        }
    }
}

async fn handle_key(app: &mut App, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }

    let code = key.code;

    match app.mode {
        InputMode::Search => match code {
            KeyCode::Esc | KeyCode::Enter => app.mode = InputMode::Normal,
            KeyCode::Backspace => {
                app.search.pop();
            }
            KeyCode::Char(c) => app.search.push(c),
            _ => {}
        },
        InputMode::Command => match code {
            KeyCode::Esc => {
                app.command.clear();
                app.mode = InputMode::Normal;
            }
            KeyCode::Enter => app.apply_command(),
            KeyCode::Backspace => {
                app.command.pop();
            }
            KeyCode::Char(c) => app.command.push(c),
            _ => {}
        },
        InputMode::Normal => match code {
            KeyCode::Char('q') => app.should_quit = true,
            KeyCode::Char('/') => app.mode = InputMode::Search,
            KeyCode::Char(':') => app.mode = InputMode::Command,
            KeyCode::Char('r') => app.status = "refresh requested".to_string(),
            KeyCode::Tab | KeyCode::Right => app.next_view(),
            KeyCode::BackTab | KeyCode::Left => app.prev_view(),
            KeyCode::Char('1') => app.view = View::Overview,
            KeyCode::Char('2') => app.view = View::Gateway,
            KeyCode::Char('3') => app.view = View::Guilds,
            KeyCode::Char('4') => app.view = View::Users,
            KeyCode::Char('5') => app.view = View::Events,
            KeyCode::Char('6') => app.view = View::Env,
            KeyCode::Char('7') => app.view = View::Actions,
            KeyCode::Down => app.selected = app.selected.saturating_add(1),
            KeyCode::Up => app.selected = app.selected.saturating_sub(1),
            _ => {}
        },
    }
}

fn render(frame: &mut Frame, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(frame.area());

    render_header(frame, root[0], app);

    match app.view {
        View::Overview => render_overview(frame, root[1], app),
        View::Gateway => render_gateway(frame, root[1], app),
        View::Guilds => render_guilds(frame, root[1], app),
        View::Users => render_users(frame, root[1], app),
        View::Events => render_events(frame, root[1], app),
        View::Env => render_env(frame, root[1], app),
        View::Actions => render_actions(frame, root[1], app),
    }

    render_footer(frame, root[2], app);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let titles: Vec<Line> = View::ALL
        .iter()
        .enumerate()
        .map(|(i, view)| Line::from(format!(" {} {}", i + 1, view.title())))
        .collect();

    let selected = View::ALL.iter().position(|v| *v == app.view).unwrap_or(0);
    let tabs = Tabs::new(titles)
        .select(selected)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Vanilla Ops TUI"),
        )
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let input = match app.mode {
        InputMode::Normal => format!(
            "{} | search='{}' | updated {}s ago",
            app.status,
            app.search,
            app.snapshot.refreshed_at.elapsed().as_secs()
        ),
        InputMode::Search => format!("/{}", app.search),
        InputMode::Command => format!(":{}", app.command),
    };

    let color = match app.mode {
        InputMode::Normal => Color::Gray,
        InputMode::Search => Color::Yellow,
        InputMode::Command => Color::Green,
    };

    let paragraph = Paragraph::new(input)
        .style(Style::default().fg(color))
        .block(Block::default().borders(Borders::ALL).title("Input"));
    frame.render_widget(paragraph, area);
}

fn render_overview(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(area);

    let services = app
        .snapshot
        .services
        .iter()
        .map(|service| {
            let color = if service.ok { Color::Green } else { Color::Red };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<10}", service.name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(format!("{:<8}", service.state), Style::default().fg(color)),
                Span::raw(service.detail.clone()),
            ]))
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        List::new(services).block(Block::default().borders(Borders::ALL).title("Services")),
        chunks[0],
    );

    let counts = &app.snapshot.counts;
    let rows = vec![
        Row::new(vec!["Users".to_string(), counts.users.to_string()]),
        Row::new(vec!["Guilds".to_string(), counts.guilds.to_string()]),
        Row::new(vec!["Channels".to_string(), counts.channels.to_string()]),
        Row::new(vec![
            "Members".to_string(),
            counts.guild_members.to_string(),
        ]),
        Row::new(vec!["Messages".to_string(), counts.messages.to_string()]),
        Row::new(vec!["Invites".to_string(), counts.invites.to_string()]),
    ];
    frame.render_widget(simple_table(rows, "Database Counts"), chunks[1]);
}

fn render_gateway(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Min(7),
        ])
        .split(area);

    let metrics = app.snapshot.gateway_metrics.as_ref();
    let pressure = if let Some(metrics) = metrics {
        vec![
            Row::new(vec![
                "Connected sockets".to_string(),
                metrics.connected_sockets.to_string(),
                "live".to_string(),
            ]),
            Row::new(vec![
                "Identified users".to_string(),
                metrics.identified_users.to_string(),
                "live".to_string(),
            ]),
            Row::new(vec![
                "Subscribed rooms".to_string(),
                metrics.subscribed_rooms.to_string(),
                "live".to_string(),
            ]),
            Row::new(vec![
                "Inbound / Dispatch / Broadcast".to_string(),
                format!(
                    "{} / {} / {}",
                    metrics.total_inbound, metrics.total_dispatches, metrics.total_broadcasts
                ),
                "since boot".to_string(),
            ]),
        ]
    } else {
        vec![
            Row::new(vec![
                "Connected sockets".to_string(),
                "backend metrics unavailable".to_string(),
                "start backend".to_string(),
            ]),
            Row::new(vec![
                "Identified users".to_string(),
                "backend metrics unavailable".to_string(),
                "start backend".to_string(),
            ]),
            Row::new(vec![
                "Rooms".to_string(),
                "guild count from DB".to_string(),
                app.snapshot.counts.guilds.to_string(),
            ]),
            Row::new(vec![
                "Likely noisy events".to_string(),
                "typing/presence/message".to_string(),
                "watch".to_string(),
            ]),
        ]
    };
    frame.render_widget(simple_table(pressure, "Gateway Pressure"), chunks[0]);

    let left = metrics
        .map(|metrics| metric_rows("Inbound ops", &metrics.inbound_by_op))
        .unwrap_or_else(|| vec![Row::new(vec!["no data".to_string(), "0".to_string()])]);
    frame.render_widget(
        Table::new(
            left,
            [Constraint::Percentage(70), Constraint::Percentage(30)],
        )
        .header(header(["op", "count"]))
        .block(Block::default().borders(Borders::ALL).title("Inbound")),
        chunks[1],
    );

    let mid_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);

    let dispatch = metrics
        .map(|metrics| metric_rows("Dispatch", &metrics.dispatch_by_type))
        .unwrap_or_else(|| vec![Row::new(vec!["no data".to_string(), "0".to_string()])]);
    frame.render_widget(
        Table::new(
            dispatch,
            [Constraint::Percentage(70), Constraint::Percentage(30)],
        )
        .header(header(["event", "count"]))
        .block(Block::default().borders(Borders::ALL).title("Dispatch")),
        mid_chunks[0],
    );

    let rooms = metrics
        .map(|metrics| metric_rows("Rooms", &metrics.broadcasts_by_room))
        .unwrap_or_else(|| vec![Row::new(vec!["no data".to_string(), "0".to_string()])]);
    frame.render_widget(
        Table::new(
            rooms,
            [Constraint::Percentage(70), Constraint::Percentage(30)],
        )
        .header(header(["room", "broadcasts"]))
        .block(Block::default().borders(Borders::ALL).title("Top Rooms")),
        mid_chunks[1],
    );

    let recent = metrics
        .map(|metrics| {
            metrics
                .recent
                .iter()
                .filter(|event| {
                    matches_search(&app.search, [event.kind.as_str(), event.detail.as_str()])
                })
                .take(10)
                .map(|event| {
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{:<10}", event.kind),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(format!("{}  @{}", event.detail, event.at_ms)),
                    ]))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![ListItem::new("No live gateway metrics yet.")]);
    frame.render_widget(
        List::new(recent).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Recent Gateway Events"),
        ),
        chunks[3],
    );
}

fn render_guilds(frame: &mut Frame, area: Rect, app: &App) {
    let rows = app
        .snapshot
        .guilds
        .iter()
        .filter(|guild| matches_search(&app.search, [guild.id.as_str(), guild.name.as_str()]))
        .map(|guild| {
            Row::new(vec![
                guild.id.clone(),
                guild.name.clone(),
                guild.members.to_string(),
                guild.channels.to_string(),
                guild.messages.to_string(),
            ])
        });
    let table = Table::new(
        rows,
        [
            Constraint::Length(22),
            Constraint::Percentage(35),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(header(["id", "name", "members", "channels", "messages"]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Guild Inspector"),
    );
    frame.render_widget(table, area);
}

fn render_users(frame: &mut Frame, area: Rect, app: &App) {
    let rows = app
        .snapshot
        .users
        .iter()
        .filter(|user| {
            matches_search(
                &app.search,
                [
                    user.id.as_str(),
                    user.username.as_str(),
                    user.tag.as_str(),
                    user.status.as_str(),
                ],
            )
        })
        .map(|user| {
            Row::new(vec![
                user.id.clone(),
                user.username.clone(),
                user.tag.clone(),
                user.status.clone(),
                user.guilds.to_string(),
                user.messages.to_string(),
            ])
        });
    let table = Table::new(
        rows,
        [
            Constraint::Length(22),
            Constraint::Percentage(25),
            Constraint::Length(10),
            Constraint::Length(18),
            Constraint::Length(8),
            Constraint::Length(9),
        ],
    )
    .header(header([
        "id", "username", "tag", "status", "guilds", "messages",
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("User Inspector"),
    );
    frame.render_widget(table, area);
}

fn render_events(frame: &mut Frame, area: Rect, app: &App) {
    let rows = app
        .snapshot
        .events
        .iter()
        .filter(|event| {
            matches_search(
                &app.search,
                [
                    event.name.as_str(),
                    event.source.as_str(),
                    event.risk.as_str(),
                ],
            )
        })
        .map(|event| {
            Row::new(vec![
                event.name.as_str(),
                event.volume.as_str(),
                event.source.as_str(),
                event.risk.as_str(),
            ])
        });
    let table = Table::new(
        rows,
        [
            Constraint::Length(22),
            Constraint::Length(14),
            Constraint::Percentage(35),
            Constraint::Percentage(35),
        ],
    )
    .header(header(["event", "volume", "source", "risk"]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Event Stream Map"),
    );
    frame.render_widget(table, area);
}

fn render_env(frame: &mut Frame, area: Rect, app: &App) {
    let rows = app
        .snapshot
        .env
        .iter()
        .filter(|(key, value)| matches_search(&app.search, [key.as_str(), value.as_str()]))
        .map(|(key, value)| Row::new(vec![key.as_str(), value.as_str()]));

    let table = Table::new(rows, [Constraint::Length(28), Constraint::Min(20)])
        .header(header(["key", "value"]))
        .block(Block::default().borders(Borders::ALL).title("Environment"));
    frame.render_widget(table, area);
}

fn render_actions(frame: &mut Frame, area: Rect, _app: &App) {
    let items = vec![
        ListItem::new("r / :refresh       refresh all probes"),
        ListItem::new("/text             filter current view"),
        ListItem::new(":find text        set global search"),
        ListItem::new(":guilds           jump to guild inspector"),
        ListItem::new(":users            jump to user inspector"),
        ListItem::new(":events           jump to event map"),
        ListItem::new("Gateway metrics are read live from /admin/gateway-metrics"),
    ];
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Actions")),
        area,
    );
}

fn simple_table<'a>(rows: Vec<Row<'a>>, title: &'a str) -> Table<'a> {
    Table::new(
        rows,
        [
            Constraint::Percentage(35),
            Constraint::Percentage(25),
            Constraint::Min(10),
        ],
    )
    .block(Block::default().borders(Borders::ALL).title(title))
}

fn header<const N: usize>(labels: [&str; N]) -> Row<'_> {
    Row::new(labels).style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
}

fn matches_search<'a, I>(search: &str, values: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    if search.trim().is_empty() {
        return true;
    }
    let needle = search.to_lowercase();
    values
        .into_iter()
        .any(|value| value.to_lowercase().contains(&needle))
}

async fn collect_snapshot() -> anyhow::Result<Snapshot> {
    let env = read_env();
    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is not set")?;
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse::<u16>()
        .unwrap_or(3000);

    let mut services = Vec::new();
    let backend = probe_backend(port).await;
    services.push(backend);

    let (db_service, counts, guilds, users) = probe_database(&database_url).await;
    services.push(db_service);

    let redis_service = probe_redis(&redis_url).await;
    services.push(redis_service);

    Ok(Snapshot {
        refreshed_at: Instant::now(),
        services,
        counts,
        guilds,
        users,
        events: default_events(),
        gateway_metrics: fetch_gateway_metrics(port).await.ok(),
        env,
        last_error: None,
    })
}

fn metric_rows(_title: &str, entries: &[GatewayMetricEntry]) -> Vec<Row<'static>> {
    if entries.is_empty() {
        return vec![Row::new(vec!["none".to_string(), "0".to_string()])];
    }

    entries
        .iter()
        .map(|entry| Row::new(vec![entry.key.clone(), entry.count.to_string()]))
        .collect()
}

async fn fetch_gateway_metrics(port: u16) -> anyhow::Result<GatewayMetrics> {
    let mut stream = timeout(
        Duration::from_millis(900),
        TcpStream::connect(("127.0.0.1", port)),
    )
    .await??;

    stream
        .write_all(
            b"GET /admin/gateway-metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await?;

    let mut response = Vec::new();
    timeout(
        Duration::from_millis(900),
        stream.read_to_end(&mut response),
    )
    .await??;
    let response = String::from_utf8(response)?;
    let (_, body) = response
        .split_once("\r\n\r\n")
        .context("gateway metrics response did not contain a body")?;

    Ok(serde_json::from_str(body)?)
}

async fn probe_backend(port: u16) -> ServiceStatus {
    let addr = format!("127.0.0.1:{port}");
    match timeout(Duration::from_millis(800), TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => ServiceStatus {
            name: "Backend",
            state: "ok".to_string(),
            detail: format!("http://localhost:{port}"),
            ok: true,
        },
        Ok(Err(e)) => ServiceStatus {
            name: "Backend",
            state: "down".to_string(),
            detail: e.to_string(),
            ok: false,
        },
        Err(_) => ServiceStatus {
            name: "Backend",
            state: "timeout".to_string(),
            detail: addr,
            ok: false,
        },
    }
}

async fn probe_redis(redis_url: &str) -> ServiceStatus {
    let result = async {
        let client = redis::Client::open(redis_url)?;
        let mut connection = client.get_multiplexed_async_connection().await?;
        let pong: String = redis::cmd("PING").query_async(&mut connection).await?;
        anyhow::Ok(pong)
    };

    match timeout(Duration::from_secs(1), result).await {
        Ok(Ok(pong)) => ServiceStatus {
            name: "Redis",
            state: "ok".to_string(),
            detail: pong,
            ok: true,
        },
        Ok(Err(e)) => ServiceStatus {
            name: "Redis",
            state: "error".to_string(),
            detail: e.to_string(),
            ok: false,
        },
        Err(_) => ServiceStatus {
            name: "Redis",
            state: "timeout".to_string(),
            detail: redis_url.to_string(),
            ok: false,
        },
    }
}

async fn probe_database(
    database_url: &str,
) -> (ServiceStatus, Counts, Vec<GuildRow>, Vec<UserRow>) {
    let result = async {
        let (client, connection) = tokio_postgres::connect(database_url, NoTls).await?;
        tokio::spawn(async move {
            let _ = connection.await;
        });

        let counts = Counts {
            users: count_table(&client, "users").await?,
            guilds: count_table(&client, "guilds").await?,
            channels: count_table(&client, "channels").await?,
            guild_members: count_table(&client, "guildmembers").await?,
            messages: count_table(&client, "messages").await?,
            invites: count_table(&client, "guildinvites").await?,
        };

        let guilds = guild_rows(&client).await.unwrap_or_default();
        let users = user_rows(&client).await.unwrap_or_default();
        anyhow::Ok((counts, guilds, users))
    };

    match timeout(Duration::from_secs(3), result).await {
        Ok(Ok((counts, guilds, users))) => (
            ServiceStatus {
                name: "Postgres",
                state: "ok".to_string(),
                detail: "connected".to_string(),
                ok: true,
            },
            counts,
            guilds,
            users,
        ),
        Ok(Err(e)) => (
            ServiceStatus {
                name: "Postgres",
                state: "error".to_string(),
                detail: e.to_string(),
                ok: false,
            },
            Counts::default(),
            Vec::new(),
            Vec::new(),
        ),
        Err(_) => (
            ServiceStatus {
                name: "Postgres",
                state: "timeout".to_string(),
                detail: "DATABASE_URL".to_string(),
                ok: false,
            },
            Counts::default(),
            Vec::new(),
            Vec::new(),
        ),
    }
}

async fn count_table(client: &tokio_postgres::Client, table: &str) -> anyhow::Result<i64> {
    let sql = format!("SELECT COUNT(*)::BIGINT FROM {table}");
    let row = client.query_one(&sql, &[]).await?;
    Ok(row.get(0))
}

async fn guild_rows(client: &tokio_postgres::Client) -> anyhow::Result<Vec<GuildRow>> {
    let rows = client
        .query(
            "SELECT g.id, g.name,
                    COUNT(DISTINCT gm.id)::BIGINT AS members,
                    COUNT(DISTINCT c.id)::BIGINT AS channels,
                    COUNT(DISTINCT m.id)::BIGINT AS messages
             FROM guilds g
             LEFT JOIN guildmembers gm ON gm.guild_id = g.id
             LEFT JOIN channels c ON c.guild_id = g.id
             LEFT JOIN messages m ON m.guild_id = g.id
             GROUP BY g.id, g.name
             ORDER BY messages DESC, members DESC
             LIMIT 100",
            &[],
        )
        .await?;

    Ok(rows
        .into_iter()
        .map(|row| GuildRow {
            id: row.get("id"),
            name: row.get("name"),
            members: row.get("members"),
            channels: row.get("channels"),
            messages: row.get("messages"),
        })
        .collect())
}

async fn user_rows(client: &tokio_postgres::Client) -> anyhow::Result<Vec<UserRow>> {
    let rows = client
        .query(
            "SELECT u.id, u.username, u.tag, u.status::TEXT AS status,
                    COUNT(DISTINCT gm.id)::BIGINT AS guilds,
                    COUNT(DISTINCT m.id)::BIGINT AS messages
             FROM users u
             LEFT JOIN guildmembers gm ON gm.user_id = u.id
             LEFT JOIN messages m ON m.author_id = u.id
             GROUP BY u.id, u.username, u.tag, u.status
             ORDER BY messages DESC, guilds DESC
             LIMIT 100",
            &[],
        )
        .await?;

    Ok(rows
        .into_iter()
        .map(|row| UserRow {
            id: row.get("id"),
            username: row.get("username"),
            tag: row.get("tag"),
            status: row.get("status"),
            guilds: row.get("guilds"),
            messages: row.get("messages"),
        })
        .collect())
}

fn default_events() -> Vec<EventRow> {
    vec![
        EventRow {
            name: "MESSAGE_CREATE".to_string(),
            volume: "high".to_string(),
            source: "channels::create_message".to_string(),
            risk: "broadcasts to guild room; watch busy channels".to_string(),
        },
        EventRow {
            name: "PRESENCE_UPDATE".to_string(),
            volume: "very high".to_string(),
            source: "gateway presence + identify + disconnect".to_string(),
            risk: "can spam all guild rooms for active users".to_string(),
        },
        EventRow {
            name: "TYPING_START".to_string(),
            volume: "burst".to_string(),
            source: "channels::typing_start".to_string(),
            risk: "needs debounce/rate limit per user/channel".to_string(),
        },
        EventRow {
            name: "GUILD_MEMBER_ADD".to_string(),
            volume: "medium".to_string(),
            source: "invites::use_invite".to_string(),
            risk: "fanout to guild room".to_string(),
        },
        EventRow {
            name: "USER_UPDATE".to_string(),
            volume: "low".to_string(),
            source: "users::update_user".to_string(),
            risk: "broadcasts once per joined guild".to_string(),
        },
        EventRow {
            name: "INVITE_CODE_DELETE".to_string(),
            volume: "low".to_string(),
            source: "admin::delete_invite_code".to_string(),
            risk: "admin room only".to_string(),
        },
    ]
}

fn read_env() -> Vec<(String, String)> {
    dotenvy::dotenv().ok();
    let keys = [
        "DATABASE_URL",
        "REDIS_URL",
        "PORT",
        "NODE_ENV",
        "REGISTRATION_CLOSED",
        "TOKEN_SECRET",
        "VERBOSE",
        "USER_GUILD_LIMIT",
        "MAX_AVATAR_SIZE",
        "MAX_BANNER_SIZE",
    ];

    let mut values = HashMap::new();
    for key in keys {
        let value = std::env::var(key).unwrap_or_else(|_| "<unset>".to_string());
        values.insert(key.to_string(), mask_env(key, &value));
    }
    values.into_iter().collect()
}

fn mask_env(key: &str, value: &str) -> String {
    let sensitive = ["TOKEN", "SECRET", "PASSWORD", "DATABASE_URL"];
    if sensitive.iter().any(|needle| key.contains(needle)) {
        if value.len() <= 8 {
            "***".to_string()
        } else {
            format!("{}...{}", &value[..4], &value[value.len() - 4..])
        }
    } else {
        value.to_string()
    }
}
