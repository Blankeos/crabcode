use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const BOX_WIDTH: usize = 56;
const TOOL_BAR_WIDTH: usize = 20;

#[derive(Clone, Debug, Default)]
pub struct StatsOptions {
    pub days: Option<u64>,
    pub tools: Option<usize>,
    pub models: Option<Option<usize>>,
    pub project: Option<String>,
}

fn model_title_row() -> String {
    let text = "MODEL USAGE";
    let left = (BOX_WIDTH.saturating_sub(text.len())) / 2;
    let right = BOX_WIDTH.saturating_sub(text.len() + left);
    format!("│{}{}{}│", " ".repeat(left), text, " ".repeat(right))
}

#[derive(Clone, Debug, Default, PartialEq)]
struct UsageTotals {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost: f64,
}

impl UsageTotals {
    fn tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
}

#[derive(Clone, Debug)]
struct SessionRow {
    id: i64,
    workspace_path: Option<String>,
    total_cost: f64,
}

#[derive(Clone, Debug)]
struct MessageRow {
    session_id: i64,
    timestamp: i64,
    parts: String,
    model: Option<String>,
    provider: Option<String>,
    output_tokens: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ModelStats {
    messages: u64,
    usage: UsageTotals,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct StatsReport {
    sessions: usize,
    messages: usize,
    days: usize,
    usage: UsageTotals,
    average_tokens_per_session: u64,
    median_tokens_per_session: u64,
    tool_total: u64,
    tools: Vec<(String, u64)>,
    models: Vec<(String, ModelStats)>,
}

pub fn run(options: StatsOptions) -> Result<()> {
    let conn = crate::persistence::db::get_db_conn()?;
    let conn = conn.lock().unwrap();
    let report = collect(&conn, &options, now_timestamp())?;
    print!("{}", render(&report, &options));
    Ok(())
}

fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn collect(conn: &Connection, options: &StatsOptions, now: i64) -> Result<StatsReport> {
    let sessions = load_sessions(conn)?;
    let messages = load_messages(conn)?;
    let project_filter = resolve_project_filter(options.project.as_deref())?;
    let cutoff = options
        .days
        .map(|days| now.saturating_sub((days.saturating_mul(86_400)) as i64));

    let selected_sessions: HashMap<i64, &SessionRow> = sessions
        .iter()
        .filter(|session| project_matches(session, project_filter.as_deref()))
        .map(|session| (session.id, session))
        .collect();

    let filtered_messages: Vec<&MessageRow> = messages
        .iter()
        .filter(|message| selected_sessions.contains_key(&message.session_id))
        .filter(|message| cutoff.is_none_or(|cutoff| message.timestamp >= cutoff))
        .collect();

    let active_session_ids: HashSet<i64> = filtered_messages
        .iter()
        .map(|message| message.session_id)
        .collect();
    let report_sessions: HashSet<i64> = if options.days.is_some() {
        active_session_ids
    } else {
        selected_sessions.keys().copied().collect()
    };

    let mut usage = UsageTotals::default();
    let mut session_tokens: HashMap<i64, u64> =
        report_sessions.iter().copied().map(|id| (id, 0)).collect();
    let mut tool_counts: HashMap<String, u64> = HashMap::new();
    let mut model_counts: HashMap<String, ModelStats> = HashMap::new();
    let mut active_days = HashSet::new();

    for message in &filtered_messages {
        usage.output = usage.output.saturating_add(message.output_tokens);
        *session_tokens.entry(message.session_id).or_default() = session_tokens
            .get(&message.session_id)
            .copied()
            .unwrap_or_default()
            .saturating_add(message.output_tokens);

        if let Some(day) = Local
            .timestamp_opt(message.timestamp, 0)
            .single()
            .map(|timestamp| timestamp.date_naive())
        {
            active_days.insert(day);
        }

        for tool in tool_names(&message.parts) {
            *tool_counts.entry(tool).or_default() += 1;
        }

        if let Some(model) = message.model.as_deref().filter(|model| !model.is_empty()) {
            let name = match message
                .provider
                .as_deref()
                .filter(|provider| !provider.is_empty())
            {
                Some(provider) if !model.starts_with(&format!("{provider}/")) => {
                    format!("{provider}/{model}")
                }
                _ => model.to_string(),
            };
            let stats = model_counts.entry(name).or_default();
            stats.messages += 1;
            stats.usage.output = stats.usage.output.saturating_add(message.output_tokens);
        }
    }

    usage.cost = report_sessions
        .iter()
        .filter_map(|id| selected_sessions.get(id))
        .map(|session| session.total_cost)
        .sum();

    let mut per_session: Vec<u64> = session_tokens.into_values().collect();
    per_session.sort_unstable();
    let average_tokens_per_session = if per_session.is_empty() {
        0
    } else {
        usage.tokens() / per_session.len() as u64
    };
    let median_tokens_per_session = median(&per_session);

    let mut tools: Vec<_> = tool_counts.into_iter().collect();
    tools.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_b.cmp(count_a).then_with(|| name_a.cmp(name_b))
    });
    let tool_total = tools.iter().map(|(_, count)| count).sum();
    if let Some(limit) = options.tools {
        tools.truncate(limit);
    }

    let mut models: Vec<_> = model_counts.into_iter().collect();
    models.sort_by(|(name_a, stats_a), (name_b, stats_b)| {
        stats_b
            .usage
            .tokens()
            .cmp(&stats_a.usage.tokens())
            .then_with(|| stats_b.messages.cmp(&stats_a.messages))
            .then_with(|| name_a.cmp(name_b))
    });
    if let Some(Some(limit)) = options.models {
        models.truncate(limit);
    }

    Ok(StatsReport {
        sessions: report_sessions.len(),
        messages: filtered_messages.len(),
        days: options
            .days
            .map(|days| days as usize)
            .unwrap_or(active_days.len()),
        usage,
        average_tokens_per_session,
        median_tokens_per_session,
        tool_total,
        tools,
        models,
    })
}

fn load_sessions(conn: &Connection) -> Result<Vec<SessionRow>> {
    let mut statement = conn.prepare(
        "SELECT s.id, w.root_path, s.total_cost
         FROM sessions s
         LEFT JOIN workspaces w ON w.id = s.workspace_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(SessionRow {
            id: row.get(0)?,
            workspace_path: row.get(1)?,
            total_cost: row.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load sessions for stats")
}

fn load_messages(conn: &Connection) -> Result<Vec<MessageRow>> {
    let mut statement = conn.prepare(
        "SELECT session_id, timestamp, parts, model, provider,
                COALESCE(output_tokens, tokens_used, 0)
         FROM messages",
    )?;
    let rows = statement.query_map([], |row| {
        let output_tokens: i64 = row.get(5)?;
        Ok(MessageRow {
            session_id: row.get(0)?,
            timestamp: row.get(1)?,
            parts: row.get(2)?,
            model: row.get(3)?,
            provider: row.get(4)?,
            output_tokens: output_tokens.max(0) as u64,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load messages for stats")
}

fn resolve_project_filter(project: Option<&str>) -> Result<Option<String>> {
    match project {
        None => Ok(None),
        Some("") => Ok(Some(
            std::env::current_dir()?
                .canonicalize()
                .unwrap_or(std::env::current_dir()?)
                .to_string_lossy()
                .into_owned(),
        )),
        Some(project) => Ok(Some(
            Path::new(project)
                .canonicalize()
                .unwrap_or_else(|_| Path::new(project).to_path_buf())
                .to_string_lossy()
                .into_owned(),
        )),
    }
}

fn project_matches(session: &SessionRow, project: Option<&str>) -> bool {
    let Some(project) = project else {
        return true;
    };
    session.workspace_path.as_deref().is_some_and(|workspace| {
        workspace == project
            || Path::new(workspace)
                .file_name()
                .is_some_and(|name| name.to_string_lossy() == project)
    })
}

fn tool_names(parts: &str) -> Vec<String> {
    serde_json::from_str::<Vec<Value>>(parts)
        .unwrap_or_default()
        .into_iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool_call"))
        .filter_map(|part| {
            part.get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        })
        .collect()
}

fn median(sorted: &[u64]) -> u64 {
    match sorted.len() {
        0 => 0,
        len if len % 2 == 1 => sorted[len / 2],
        len => sorted[len / 2 - 1].saturating_add(sorted[len / 2]) / 2,
    }
}

fn render(report: &StatsReport, options: &StatsOptions) -> String {
    let mut sections = vec![render_overview(report), render_cost_and_tokens(report)];
    if options.models.is_some() && !report.models.is_empty() {
        sections.push(render_models(&report.models));
    }
    if !report.tools.is_empty() {
        sections.push(render_tools(&report.tools, report.tool_total));
    }
    format!("{}\n", sections.join("\n\n"))
}

fn render_overview(report: &StatsReport) -> String {
    render_table(
        "OVERVIEW",
        &[
            ("Sessions", report.sessions.to_string()),
            ("Messages", report.messages.to_string()),
            ("Days", report.days.to_string()),
        ],
    )
}

fn render_cost_and_tokens(report: &StatsReport) -> String {
    let average_cost = if report.days == 0 {
        0.0
    } else {
        report.usage.cost / report.days as f64
    };
    render_table(
        "COST & TOKENS",
        &[
            ("Total Cost", format!("${:.2}", report.usage.cost)),
            ("Avg Cost/Day", format!("${average_cost:.2}")),
            (
                "Avg Tokens/Session",
                compact_number(report.average_tokens_per_session),
            ),
            (
                "Median Tokens/Session",
                compact_number(report.median_tokens_per_session),
            ),
            ("Input", compact_number(report.usage.input)),
            ("Output", compact_number(report.usage.output)),
            ("Cache Read", compact_number(report.usage.cache_read)),
            ("Cache Write", compact_number(report.usage.cache_write)),
        ],
    )
}

fn render_models(models: &[(String, ModelStats)]) -> String {
    let mut lines = vec![top_border(), model_title_row(), middle_border()];
    for (index, (name, stats)) in models.iter().enumerate() {
        lines.push(text_row(&format!(" {name}")));
        lines.push(metric_row("  Messages", &stats.messages.to_string()));
        lines.push(metric_row(
            "  Input Tokens",
            &compact_number(stats.usage.input),
        ));
        lines.push(metric_row(
            "  Output Tokens",
            &compact_number(stats.usage.output),
        ));
        lines.push(metric_row(
            "  Cache Read",
            &compact_number(stats.usage.cache_read),
        ));
        lines.push(metric_row(
            "  Cache Write",
            &compact_number(stats.usage.cache_write),
        ));
        lines.push(metric_row("  Cost", &format!("${:.4}", stats.usage.cost)));
        if index + 1 < models.len() {
            lines.push(middle_border());
        }
    }
    lines.push(bottom_border());
    lines.join("\n")
}

fn render_tools(tools: &[(String, u64)], total: u64) -> String {
    let max = tools.first().map(|(_, count)| *count).unwrap_or(0);
    let mut lines = vec![top_border(), centered_row("TOOL USAGE"), middle_border()];
    for (name, count) in tools {
        let percentage = if total == 0 {
            0.0
        } else {
            *count as f64 * 100.0 / total as f64
        };
        let bar_len = if max == 0 {
            0
        } else {
            ((*count as f64 / max as f64) * TOOL_BAR_WIDTH as f64)
                .round()
                .max(1.0) as usize
        };
        let name = truncate(name, 18);
        let body = format!(
            " {name:<18} {:<20} {count} ({percentage:>4.1}%)",
            "█".repeat(bar_len)
        );
        lines.push(text_row(&body));
    }
    lines.push(bottom_border());
    lines.join("\n")
}

fn render_table(title: &str, rows: &[(&str, String)]) -> String {
    let mut lines = vec![top_border(), centered_row(title), middle_border()];
    lines.extend(rows.iter().map(|(label, value)| metric_row(label, value)));
    lines.push(bottom_border());
    lines.join("\n")
}

fn top_border() -> String {
    format!("┌{}┐", "─".repeat(BOX_WIDTH))
}

fn middle_border() -> String {
    format!("├{}┤", "─".repeat(BOX_WIDTH))
}

fn bottom_border() -> String {
    format!("└{}┘", "─".repeat(BOX_WIDTH))
}

fn centered_row(text: &str) -> String {
    let left = (BOX_WIDTH.saturating_sub(text.chars().count()) / 2).saturating_sub(1);
    let right = BOX_WIDTH.saturating_sub(text.chars().count() + left);
    format!("│{}{}{}│", " ".repeat(left), text, " ".repeat(right))
}

fn metric_row(label: &str, value: &str) -> String {
    let usable = BOX_WIDTH - 1;
    let label = truncate(label, usable.saturating_sub(value.chars().count()));
    let spaces = usable.saturating_sub(label.chars().count() + value.chars().count());
    format!("│{label}{}{value} │", " ".repeat(spaces))
}

fn text_row(text: &str) -> String {
    let text = truncate(text, BOX_WIDTH);
    let padding = BOX_WIDTH.saturating_sub(text.chars().count());
    format!("│{text}{}│", " ".repeat(padding))
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    if width <= 2 {
        return value.chars().take(width).collect();
    }
    format!("{}..", value.chars().take(width - 2).collect::<String>())
}

fn compact_number(value: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1_000_000_000, "B"),
        (1_000_000, "M"),
        (1_000, "K"),
        (1, ""),
    ];
    for (divisor, suffix) in UNITS {
        if value >= divisor {
            return if divisor == 1 {
                value.to_string()
            } else {
                format!("{:.1}{suffix}", value as f64 / divisor as f64)
            };
        }
    }
    "0".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::migrations::run_migrations;
    use rusqlite::params;

    fn test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO workspaces (root_path, display_name) VALUES ('/tmp/one', 'one')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (session_identifier, name, workspace_id, total_cost)
             VALUES ('ses_1', 'One', 1, 1.25), ('ses_2', 'Two', 1, 0.75)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages
             (id, session_id, role, parts, timestamp, tokens_used, output_tokens, model, provider)
             VALUES
             ('m1', 1, 'assistant', ?1, 1000, 1200, 1200, 'gpt-test', 'openai'),
             ('m2', 1, 'user', '[]', 1001, 0, 0, NULL, NULL),
             ('m3', 2, 'assistant', ?2, 90000, 800, 800, 'openai/gpt-test', 'openai')",
            params![
                r#"[{"type":"tool_call","name":"read"},{"type":"tool_call","name":"bash"}]"#,
                r#"[{"type":"tool_call","name":"read"}]"#
            ],
        )
        .unwrap();
        conn
    }

    #[test]
    fn collects_totals_tools_models_and_median() {
        let report = collect(
            &test_db(),
            &StatsOptions {
                models: Some(None),
                ..StatsOptions::default()
            },
            100_000,
        )
        .unwrap();

        assert_eq!(report.sessions, 2);
        assert_eq!(report.messages, 3);
        assert_eq!(report.usage.output, 2_000);
        assert_eq!(report.average_tokens_per_session, 1_000);
        assert_eq!(report.median_tokens_per_session, 1_000);
        assert_eq!(report.tool_total, 3);
        assert_eq!(report.tools, vec![("read".into(), 2), ("bash".into(), 1)]);
        assert_eq!(report.models[0].0, "openai/gpt-test");
        assert_eq!(report.models[0].1.messages, 2);
        assert_eq!(report.models[0].1.usage.output, 2_000);
    }

    #[test]
    fn days_filter_counts_only_active_sessions_and_uses_requested_days() {
        let report = collect(
            &test_db(),
            &StatsOptions {
                days: Some(1),
                ..StatsOptions::default()
            },
            100_000,
        )
        .unwrap();

        assert_eq!(report.sessions, 1);
        assert_eq!(report.messages, 1);
        assert_eq!(report.days, 1);
        assert_eq!(report.usage.output, 800);
    }

    #[test]
    fn renders_opencode_style_sections() {
        let output = render(
            &StatsReport {
                sessions: 2,
                messages: 3,
                days: 2,
                usage: UsageTotals {
                    output: 2_000,
                    cost: 2.0,
                    ..UsageTotals::default()
                },
                average_tokens_per_session: 1_000,
                median_tokens_per_session: 1_000,
                tool_total: 3,
                tools: vec![("read".into(), 2), ("bash".into(), 1)],
                ..StatsReport::default()
            },
            &StatsOptions::default(),
        );

        assert!(output.contains("│                       OVERVIEW                         │"));
        assert!(output.contains("│Sessions                                              2 │"));
        assert!(output.contains("│Output                                             2.0K │"));
        assert!(output.contains("│                      TOOL USAGE                        │"));
        assert!(output
            .lines()
            .filter(|line| !line.is_empty())
            .all(|line| line.chars().count() == 58));
    }

    #[test]
    fn compact_numbers_match_stats_display() {
        assert_eq!(compact_number(0), "0");
        assert_eq!(compact_number(999), "999");
        assert_eq!(compact_number(1_000), "1.0K");
        assert_eq!(compact_number(10_600_000), "10.6M");
        assert_eq!(compact_number(1_310_600_000), "1.3B");
    }
}
