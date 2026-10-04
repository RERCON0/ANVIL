//! What the quota block shows, decided without egui so it can be tested:
//! which providers and windows are visible, what is dimmed, the hover text,
//! and whether the block fits expanded, collapsed or not at all.

use super::model::{ProviderId, ProviderSnapshot, ProviderState, Snapshot};
use super::time::{format_clock, format_left};
use crate::config::QuotaConfig;
use crate::strings;

/// Data older than this is dimmed even when the last fetch succeeded.
pub const STALE_AFTER: i64 = 1_800;
/// From this share on, the reset time is written on the line itself.
pub const RESET_INLINE_FROM: f64 = 60.0;
pub const HEADER_HEIGHT: f32 = 20.0;
pub const LINE_HEIGHT: f32 = 15.0;
pub const PADDING: f32 = 6.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub label: String,
    pub used_pct: f64,
    /// `2ч5м` when the window is at least 60 % used.
    pub reset_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: ProviderId,
    pub name: &'static str,
    pub lines: Vec<Line>,
    pub dim: bool,
    pub note: Option<&'static str>,
    pub tooltip: String,
}

/// `42%` with JavaScript rounding, as the Claude line prints percentages.
pub fn percent(pct: f64) -> String {
    format!("{}%", (pct + 0.5).floor() as i64)
}

pub fn rows(snapshot: &Snapshot, config: &QuotaConfig, now: i64) -> Vec<Row> {
    ProviderId::ALL
        .into_iter()
        .filter_map(|id| {
            let provider = snapshot.get(id)?;
            if config.provider_enabled(id.key()) == Some(false) {
                return None;
            }
            let note = match provider.state {
                ProviderState::AuthExpired => Some(strings::QUOTA_AUTH_EXPIRED),
                ProviderState::FormatError { .. } => Some(strings::QUOTA_FORMAT_ERROR),
                _ => None,
            };
            let lines: Vec<Line> = if matches!(provider.state, ProviderState::FormatError { .. }) {
                Vec::new()
            } else {
                provider
                    .windows
                    .iter()
                    .filter(|w| config.window_visible(id.key(), &w.key))
                    .map(|w| Line {
                        label: w.label.clone(),
                        used_pct: w.used_pct,
                        reset_hint: w
                            .resets_at
                            .filter(|_| w.used_pct >= RESET_INLINE_FROM)
                            .map(|at| format_left(at - now)),
                    })
                    .collect()
            };
            if lines.is_empty() && note.is_none() {
                return None;
            }
            let stale = provider.fetched_at.is_none_or(|at| now - at > STALE_AFTER);
            let dim = stale || provider.state != ProviderState::Ok;
            Some(Row { id, name: id.label(), lines, dim, note, tooltip: tooltip(provider, config, now) })
        })
        .collect()
}

fn tooltip(provider: &ProviderSnapshot, config: &QuotaConfig, now: i64) -> String {
    let name = match &provider.plan {
        Some(plan) => format!("{} {plan}", provider.id.label()),
        None => provider.id.label().to_owned(),
    };
    let mut out = vec![format!("{name} · {} {}", strings::QUOTA_LOGIN, provider.source)];
    for window in provider.windows.iter().filter(|w| config.window_visible(provider.id.key(), &w.key)) {
        let mut line = format!("{} {}", window.label, percent(window.used_pct));
        if let Some(at) = window.resets_at {
            line.push_str(&format!(" — {} {}", strings::QUOTA_RESET_IN, format_left(at - now)));
            if let Some(clock) = format_clock(at, now) {
                line.push_str(&format!(" ({clock})"));
            }
        }
        out.push(line);
    }
    if let Some(clock) = provider.fetched_at.and_then(|at| format_clock(at, now)) {
        out.push(format!("{} {clock}", strings::QUOTA_DATA_AT));
    }
    match &provider.state {
        ProviderState::UpdateFailed { reason } => out.push(format!("{} {reason}", strings::QUOTA_UPDATE_FAILED)),
        ProviderState::RateLimited { retry_at } => {
            if let Some(clock) = format_clock(*retry_at, now) {
                out.push(format!("{} {clock}", strings::QUOTA_RETRY_AT));
            }
        }
        ProviderState::AuthExpired => out.push(strings::QUOTA_AUTH_EXPIRED.to_owned()),
        ProviderState::FormatError { reason } => out.push(format!("{}: {reason}", strings::QUOTA_FORMAT_ERROR)),
        ProviderState::Idle | ProviderState::Ok => {}
    }
    out.join("\n")
}

/// The most used visible window, for the collapsed one-line block:
/// `ChatGPT 5ч 42%`.
pub fn summary(rows: &[Row]) -> Option<(String, f64)> {
    rows.iter()
        .flat_map(|row| row.lines.iter().map(move |line| (row, line)))
        .max_by(|a, b| a.1.used_pct.total_cmp(&b.1.used_pct))
        .map(|(row, line)| (format!("{} {} {}", row.name, line.label, percent(line.used_pct)), line.used_pct))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Hidden,
    Collapsed,
    Expanded,
}

/// Text lines a row takes: one per window, plus one for a note under them
/// (a note alone takes the name line).
pub fn row_lines(row: &Row) -> usize {
    (row.lines.len() + usize::from(row.note.is_some() && !row.lines.is_empty())).max(1)
}

pub fn expanded_height(rows: &[Row]) -> f32 {
    HEADER_HEIGHT + rows.iter().map(row_lines).sum::<usize>() as f32 * LINE_HEIGHT + PADDING
}

/// Tabs get the space first: the block expands only when it fits whole.
pub fn fit(available: f32, rows: &[Row], collapsed: bool) -> Fit {
    if rows.is_empty() || available < HEADER_HEIGHT {
        Fit::Hidden
    } else if !collapsed && expanded_height(rows) <= available {
        Fit::Expanded
    } else {
        Fit::Collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::model::Window;

    const NOW: i64 = 1_791_210_000;

    fn provider(
        id: ProviderId,
        state: ProviderState,
        windows: Vec<Window>,
        fetched_at: Option<i64>,
    ) -> ProviderSnapshot {
        ProviderSnapshot {
            id,
            plan: Some("Pro".into()),
            source: "Codex CLI".into(),
            state,
            windows,
            fetched_at,
            checked_at: NOW,
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            version: Snapshot::VERSION,
            providers: vec![
                provider(
                    ProviderId::ChatGpt,
                    ProviderState::Ok,
                    vec![
                        Window::new("5h", "5ч", 42.0, Some(NOW + 3_900)),
                        Window::new("7d", "7д", 75.0, Some(NOW + 7_500)),
                        Window::new("review", "ревью", 3.0, None),
                    ],
                    Some(NOW - 60),
                ),
                provider(
                    ProviderId::Claude,
                    ProviderState::AuthExpired,
                    vec![Window::new("5h", "5ч", 17.0, None)],
                    Some(NOW - 7_200),
                ),
                provider(
                    ProviderId::Zai,
                    ProviderState::FormatError { reason: "x".into() },
                    vec![Window::new("5h", "5ч", 1.0, None)],
                    None,
                ),
                provider(ProviderId::Kimi, ProviderState::Idle, Vec::new(), None),
            ],
        }
    }

    #[test]
    fn visible_rows_lines_and_dimming() {
        let rows = rows(&snapshot(), &QuotaConfig::default(), NOW);
        let summary: Vec<(ProviderId, Vec<&str>, bool, Option<&str>)> =
            rows.iter().map(|r| (r.id, r.lines.iter().map(|l| l.label.as_str()).collect(), r.dim, r.note)).collect();
        assert_eq!(
            summary,
            vec![
                (ProviderId::Claude, vec!["5ч"], true, Some("вход устарел")),
                (ProviderId::ChatGpt, vec!["5ч", "7д"], false, None),
                (ProviderId::Zai, vec![], true, Some("ошибка формата")),
            ],
            "order follows ProviderId::ALL; review hidden by default; Idle rows without data are skipped"
        );
        let chatgpt = &rows[1];
        assert_eq!(chatgpt.lines[0].reset_hint, None, "below 60 %");
        assert_eq!(chatgpt.lines[1].reset_hint.as_deref(), Some("2ч5м"));
        assert!(
            chatgpt.tooltip.starts_with("ChatGPT Pro · вход: Codex CLI\n5ч 42% — сброс через 1ч5м ("),
            "{}",
            chatgpt.tooltip
        );
    }

    #[test]
    fn user_choices_hide_providers_and_show_windows() {
        let mut config = QuotaConfig::default();
        config.set_provider_enabled("claude", Some(false));
        config.set_window_visible("chatgpt", "review", true);
        config.set_window_visible("chatgpt", "5h", false);
        let rows = rows(&snapshot(), &config, NOW);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![ProviderId::ChatGpt, ProviderId::Zai]);
        assert_eq!(rows[0].lines.iter().map(|l| l.label.as_str()).collect::<Vec<_>>(), vec!["7д", "ревью"]);
    }

    #[test]
    fn summary_and_fit() {
        let rows = rows(&snapshot(), &QuotaConfig::default(), NOW);
        assert_eq!(rows.iter().map(row_lines).collect::<Vec<_>>(), vec![2, 2, 1], "a note gets its own line");
        assert_eq!(expanded_height(&rows), HEADER_HEIGHT + 5.0 * LINE_HEIGHT + PADDING);
        assert_eq!(summary(&rows), Some(("ChatGPT 7д 75%".to_owned(), 75.0)));
        let full = expanded_height(&rows);
        assert_eq!(fit(full, &rows, false), Fit::Expanded);
        assert_eq!(fit(full - 1.0, &rows, false), Fit::Collapsed);
        assert_eq!(fit(full, &rows, true), Fit::Collapsed);
        assert_eq!(fit(HEADER_HEIGHT - 1.0, &rows, false), Fit::Hidden);
        assert_eq!(fit(500.0, &[], false), Fit::Hidden);
    }
}
