//! `stratum dashboard` command — live terminal dashboard for metrics.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};
use ratatui::Terminal;

use stratum_adapters::collector::{
    METRIC_COMPACTION_COUNT, METRIC_HITL_QUEUE_DEPTH, METRIC_KV_CACHE_HIT_RATE,
    METRIC_LLM_CALLS_TOTAL, METRIC_LLM_INPUT_TOKENS_TOTAL, METRIC_LLM_OUTPUT_TOKENS_TOTAL,
    METRIC_TOOL_CALLS_TOTAL, METRIC_TOOL_ERRORS_TOTAL,
};
use stratum_adapters::{InMemoryMetrics, MetricsCollector};
use stratum_core::{MetricsExporter, SessionManager};

use crate::config::StratumConfig;
use crate::wiring::QueryContext;

/// Parsed metrics for display.
struct DashboardData {
    runs: Vec<RunRow>,
    global_hitl_depth: u64,
    raw_metrics: String,
}

struct RunRow {
    run_id: String,
    state: String,
    llm_calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_hit_rate: f64,
    tool_calls: u64,
    tool_errors: u64,
    compactions: u64,
}

pub async fn execute(config: &StratumConfig) -> anyhow::Result<()> {
    let ctx = QueryContext::build(config)?;

    // Set up terminal
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = run_dashboard(&mut terminal, &ctx).await;

    // Restore terminal
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    result
}

async fn run_dashboard(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ctx: &QueryContext,
) -> anyhow::Result<()> {
    let metrics = Arc::new(InMemoryMetrics::new());
    let collector = MetricsCollector::new(
        Arc::clone(&metrics),
        Arc::clone(&ctx.trajectory),
        Arc::clone(&ctx.hitl),
    );

    loop {
        // Refresh metrics
        let data = refresh_data(&collector, &metrics, ctx).await?;

        // Draw UI
        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Title
                    Constraint::Min(8),    // Runs table
                    Constraint::Length(5), // Global metrics
                    Constraint::Length(2), // Footer
                ])
                .split(frame.area());

            // Title
            let title = Paragraph::new(Line::from(vec![
                Span::styled(
                    " Stratum Dashboard ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  (press 'q' to quit, 'r' to refresh)"),
            ]))
            .block(Block::default().borders(Borders::BOTTOM));
            frame.render_widget(title, chunks[0]);

            // Runs table
            let header = Row::new(vec![
                "Run ID",
                "State",
                "LLM Calls",
                "Input Tok",
                "Output Tok",
                "Cache Hit",
                "Tools",
                "Errors",
                "Compacts",
            ])
            .style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );

            let rows: Vec<Row> = data
                .runs
                .iter()
                .map(|r| {
                    Row::new(vec![
                        truncate_id(&r.run_id),
                        r.state.clone(),
                        r.llm_calls.to_string(),
                        format_tokens(r.input_tokens),
                        format_tokens(r.output_tokens),
                        format!("{:.1}%", r.cache_hit_rate * 100.0),
                        r.tool_calls.to_string(),
                        r.tool_errors.to_string(),
                        r.compactions.to_string(),
                    ])
                    .style(state_style(&r.state))
                })
                .collect();

            let table = Table::new(
                rows,
                [
                    Constraint::Length(12),
                    Constraint::Length(14),
                    Constraint::Length(10),
                    Constraint::Length(11),
                    Constraint::Length(11),
                    Constraint::Length(10),
                    Constraint::Length(7),
                    Constraint::Length(7),
                    Constraint::Length(9),
                ],
            )
            .header(header)
            .block(Block::default().title(" Runs ").borders(Borders::ALL));
            frame.render_widget(table, chunks[1]);

            // Global metrics
            let global = Paragraph::new(vec![
                Line::from(format!("  HITL Queue Depth: {}", data.global_hitl_depth)),
                Line::from(format!(
                    "  Active Runs: {}",
                    data.runs.iter().filter(|r| r.state == "Running").count()
                )),
            ])
            .block(Block::default().title(" Global ").borders(Borders::ALL));
            frame.render_widget(global, chunks[2]);

            // Footer
            let footer = Paragraph::new(Line::from(vec![
                Span::styled(" q ", Style::default().fg(Color::Red)),
                Span::raw("quit  "),
                Span::styled(" r ", Style::default().fg(Color::Green)),
                Span::raw("refresh  "),
                Span::styled(" m ", Style::default().fg(Color::Blue)),
                Span::raw("raw metrics"),
            ]));
            frame.render_widget(footer, chunks[3]);
        })?;

        // Handle input (poll with timeout for auto-refresh)
        if event::poll(Duration::from_secs(2))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('m') => {
                            show_raw_metrics(terminal, &data.raw_metrics)?;
                        }
                        KeyCode::Char('r') => continue,
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}

async fn refresh_data(
    collector: &MetricsCollector<
        stratum_adapters::SqliteTrajectoryStore,
        stratum_adapters::SqliteHitlController,
    >,
    metrics: &Arc<InMemoryMetrics>,
    ctx: &QueryContext,
) -> anyhow::Result<DashboardData> {
    collector.refresh_global().await?;

    let run_ids = super::discover_run_ids(ctx.trajectory.as_ref()).await?;

    let mut runs = Vec::new();
    for rid in &run_ids {
        if let Some(run) = ctx.session.get_run(*rid).await? {
            collector.refresh_run(*rid).await?;
            let rid_str = rid.to_string();
            let labels = &[("run_id", rid_str.as_str())];

            let llm_calls = gauge_u64(metrics, METRIC_LLM_CALLS_TOTAL, labels);
            let input_tokens = gauge_u64(metrics, METRIC_LLM_INPUT_TOKENS_TOTAL, labels);
            let output_tokens = gauge_u64(metrics, METRIC_LLM_OUTPUT_TOKENS_TOTAL, labels);
            let cache_hit_rate = metrics
                .get_gauge(METRIC_KV_CACHE_HIT_RATE, labels)
                .unwrap_or(0.0);
            let tool_calls = gauge_u64(metrics, METRIC_TOOL_CALLS_TOTAL, labels);
            let tool_errors = gauge_u64(metrics, METRIC_TOOL_ERRORS_TOTAL, labels);
            let compactions = gauge_u64(metrics, METRIC_COMPACTION_COUNT, labels);

            runs.push(RunRow {
                run_id: rid_str,
                state: format!("{:?}", run.state),
                llm_calls,
                input_tokens,
                output_tokens,
                cache_hit_rate,
                tool_calls,
                tool_errors,
                compactions,
            });
        }
    }

    let global_hitl_depth = gauge_u64(metrics, METRIC_HITL_QUEUE_DEPTH, &[]);

    Ok(DashboardData {
        runs,
        global_hitl_depth,
        raw_metrics: metrics.export_metrics(),
    })
}

/// Query a gauge as u64 (defaults to 0).
fn gauge_u64(metrics: &InMemoryMetrics, name: &str, labels: &[(&str, &str)]) -> u64 {
    metrics.get_gauge(name, labels).unwrap_or(0.0) as u64
}

fn show_raw_metrics(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    raw: &str,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|frame| {
            let text = Paragraph::new(raw.to_string())
                .block(
                    Block::default()
                        .title(" Raw Prometheus Metrics (press any key to go back) ")
                        .borders(Borders::ALL),
                )
                .style(Style::default().fg(Color::White));
            frame.render_widget(text, frame.area());
        })?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn truncate_id(id: &str) -> String {
    if id.len() > 8 {
        format!("{}...", &id[..8])
    } else {
        id.to_string()
    }
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn state_style(state: &str) -> Style {
    match state {
        "Running" => Style::default().fg(Color::Green),
        "Completed" => Style::default().fg(Color::Blue),
        "Failed" | "Aborted" => Style::default().fg(Color::Red),
        "Paused" => Style::default().fg(Color::Yellow),
        _ => Style::default(),
    }
}
