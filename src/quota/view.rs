//! What the quota line at the bottom of the window shows, decided without
//! egui so it can be tested: one segment per provider, what is dimmed or
//! marked, the hover text, and how much detail fits the width.

use super::model::{format_amount, Balance, BalanceKind, ProviderId, ProviderSnapshot, ProviderState, Snapshot, Unit};
use super::time::{format_clock, format_left};
use crate::config::QuotaConfig;
use crate::strings;

/// Data older than this is dimmed even when the last fetch succeeded.
pub const STALE_AFTER: i64 = 1_800;
/// From this share on, the reset time is written next to the value.
pub const RESET_INLINE_FROM: f64 = 60.0;
/// Login expired: the segment keeps its last values, dimmed, after this mark.
pub const MARK_EXPIRED: &str = "⚠";
/// The provider answered with an error and there is nothing to show.
pub const MARK_NO_DATA: &str = "—";

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// `5ч 34%`, `¥12.40`, `траты $3.10/$10`, `120 кр.`
    pub text: String,
    /// The used share that colours the item; None: plain text colour.
    pub pct: Option<f64>,
    /// A prepaid balance at or below zero: drawn as an alarm.
    pub exhausted: bool,
    /// `2д16ч`, drawn after the item while the width allows.
    pub reset_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub id: ProviderId,
    pub name: &'static str,
    pub items: Vec<Item>,
    pub mark: Option<&'static str>,
    pub dim: bool,
    pub tooltip: String,
}

/// `42%` with JavaScript rounding, as the Claude line prints percentages.
pub fn percent(pct: f64) -> String {
    format!("{}%", (pct + 0.5).floor() as i64)
}

/// A limit without cents prints without them: `$10`, `$12.50`.
fn format_limit(amount: f64, unit: Unit) -> String {
    let text = format_amount(amount, unit);
    if unit != Unit::Credits && amount.fract() == 0.0 {
        text.trim_end_matches(".00").to_owned()
    } else {
        text
    }
}

/// `¥12.40`, `$4.20/$20`, `траты $3.10`, `траты $3.10/$10`, `120 кр.`
pub fn balance_text(balance: &Balance) -> String {
    let value = match balance.limit {
        Some(limit) => format!("{}/{}", format_amount(balance.amount, balance.unit), format_limit(limit, balance.unit)),
        None => format_amount(balance.amount, balance.unit),
    };
    match balance.kind {
        BalanceKind::Spent => format!("{} {value}", balance.label),
        BalanceKind::Remaining => value,
    }
}

fn hint(pct: Option<f64>, resets_at: Option<i64>, now: i64) -> Option<String> {
    pct.filter(|p| *p >= RESET_INLINE_FROM).and(resets_at).map(|at| format_left(at - now))
}

pub fn segments(snapshot: &Snapshot, config: &QuotaConfig, now: i64) -> Vec<Segment> {
    ProviderId::ALL
        .into_iter()
        .filter_map(|id| {
            let provider = snapshot.get(id)?;
            if config.provider_enabled(id.key()) == Some(false) {
                return None;
            }
            let visible = |key: &str| config.window_visible(id.key(), key);
            let mut items: Vec<Item> = provider
                .windows
                .iter()
                .filter(|w| visible(&w.key))
                .map(|w| Item {
                    text: format!("{} {}", w.label, percent(w.used_pct)),
                    pct: Some(w.used_pct),
                    exhausted: false,
                    reset_hint: hint(Some(w.used_pct), w.resets_at, now),
                })
                .collect();
            items.extend(provider.balances.iter().filter(|b| visible(&b.key)).map(|b| Item {
                text: balance_text(b),
                pct: b.used_pct(),
                exhausted: b.exhausted(),
                reset_hint: hint(b.used_pct(), b.resets_at, now),
            }));
            let failed = matches!(
                provider.state,
                ProviderState::UpdateFailed { .. }
                    | ProviderState::RateLimited { .. }
                    | ProviderState::FormatError { .. }
            );
            if matches!(provider.state, ProviderState::FormatError { .. }) {
                items.clear();
            }
            let mark = match &provider.state {
                ProviderState::AuthExpired => Some(MARK_EXPIRED),
                _ if failed && items.is_empty() => Some(MARK_NO_DATA),
                _ => None,
            };
            if items.is_empty() && mark.is_none() {
                return None;
            }
            let stale = provider.fetched_at.is_none_or(|at| now - at > STALE_AFTER);
            let dim = stale || provider.state != ProviderState::Ok;
            Some(Segment { id, name: id.label(), items, mark, dim, tooltip: tooltip(provider, config, now) })
        })
        .collect()
}

/// The state as one line for the hover text and the settings page; None
/// while everything is fine.
pub fn state_note(state: &ProviderState, now: i64) -> Option<String> {
    match state {
        ProviderState::Idle | ProviderState::Ok => None,
        ProviderState::UpdateFailed { reason } => Some(format!("{} {reason}", strings::QUOTA_PROVIDER_SAID)),
        ProviderState::AuthExpired => Some(strings::QUOTA_AUTH_EXPIRED.to_owned()),
        ProviderState::RateLimited { retry_at } => {
            Some(format!("{} {}", strings::QUOTA_RATE_LIMITED, format_clock(*retry_at, now).unwrap_or_default()))
        }
        ProviderState::FormatError { reason } => Some(format!("{}: {reason}", strings::QUOTA_FORMAT_ERROR)),
    }
}

fn reset_suffix(resets_at: Option<i64>, now: i64) -> String {
    let Some(at) = resets_at else { return String::new() };
    let mut text = format!(" — {} {}", strings::QUOTA_RESET_IN, format_left(at - now));
    if let Some(clock) = format_clock(at, now) {
        text.push_str(&format!(" ({clock})"));
    }
    text
}

fn tooltip(provider: &ProviderSnapshot, config: &QuotaConfig, now: i64) -> String {
    let name = match &provider.plan {
        Some(plan) => format!("{} {plan}", provider.id.label()),
        None => provider.id.label().to_owned(),
    };
    let visible = |key: &str| config.window_visible(provider.id.key(), key);
    let mut out = vec![format!("{name} · {} {}", strings::QUOTA_LOGIN, provider.source)];
    for window in provider.windows.iter().filter(|w| visible(&w.key)) {
        out.push(format!("{} {}{}", window.label, percent(window.used_pct), reset_suffix(window.resets_at, now)));
    }
    for balance in provider.balances.iter().filter(|b| visible(&b.key)) {
        let mut line = format!("{} {}{}", balance.label, balance_text(balance), reset_suffix(balance.resets_at, now));
        if let Some(detail) = &balance.detail {
            line.push_str(&format!(" · {detail}"));
        }
        out.push(line);
    }
    if let Some(clock) = provider.fetched_at.and_then(|at| format_clock(at, now)) {
        out.push(format!("{} {clock}", strings::QUOTA_DATA_AT));
    }
    out.extend(state_note(&provider.state, now));
    out.join("\n")
}

/// How much of each segment is drawn; the line steps down until it fits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Detail {
    /// Every visible item, reset hints included.
    Full,
    /// Every visible item, no reset hints.
    NoHints,
    /// Each provider's single most used item (the first when none has a share).
    Hottest,
}

/// How a run of text in a segment is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    Name,
    Mark,
    Separator,
    Value { pct: Option<f64>, exhausted: bool },
    Hint,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub text: String,
    pub style: Style,
}

/// The runs of one segment at a level of detail. `reset_icon`: the UI font
/// has `↺`; without it the hint is the bare time.
pub fn pieces(segment: &Segment, detail: Detail, reset_icon: bool) -> Vec<Piece> {
    let piece = |text: String, style: Style| Piece { text, style };
    let mut out = vec![piece(segment.name.to_owned(), Style::Name)];
    if let Some(mark) = segment.mark {
        out.push(piece(format!(" {mark}"), Style::Mark));
    }
    let items: Vec<&Item> = match detail {
        Detail::Full | Detail::NoHints => segment.items.iter().collect(),
        Detail::Hottest => segment
            .items
            .iter()
            // `max_by` returns the last of equal values, while the tie-break is
            // documented as the first. Keeping the incumbent unless the next is
            // strictly hotter gives that: balance-only providers all share
            // `None` and still show their first item.
            .reduce(|best, item| if item.pct.unwrap_or(-1.0) > best.pct.unwrap_or(-1.0) { item } else { best })
            .into_iter()
            .collect(),
    };
    for (index, item) in items.into_iter().enumerate() {
        out.push(piece(if index == 0 { " ".to_owned() } else { " · ".to_owned() }, Style::Separator));
        out.push(piece(item.text.clone(), Style::Value { pct: item.pct, exhausted: item.exhausted }));
        if let (Detail::Full, Some(hint)) = (detail, &item.reset_hint) {
            out.push(piece(if reset_icon { format!(" ↺ {hint}") } else { format!(" {hint}") }, Style::Hint));
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub detail: Detail,
    /// Segments drawn, from the first.
    pub shown: usize,
    /// Segments folded into `+N` at the end.
    pub hidden: usize,
}

/// `+3`, the fold of the segments that did not fit.
pub fn more_text(hidden: usize) -> String {
    format!("+{hidden}")
}

/// The most detail at which every segment fits `width`; at the least detail,
/// as many leading segments as fit next to a `+N` fold. `measure` is the
/// drawn width of a text run; `gap` the room between two segments.
pub fn layout(segments: &[Segment], width: f32, gap: f32, reset_icon: bool, measure: impl Fn(&str) -> f32) -> Layout {
    let segment_width = |segment: &Segment, detail: Detail| {
        pieces(segment, detail, reset_icon).iter().map(|p| measure(&p.text)).sum::<f32>()
    };
    let total = |detail: Detail, count: usize| {
        let sum: f32 = segments[..count].iter().map(|s| segment_width(s, detail)).sum();
        sum + gap * count.saturating_sub(1) as f32
    };
    for detail in [Detail::Full, Detail::NoHints, Detail::Hottest] {
        if total(detail, segments.len()) <= width {
            return Layout { detail, shown: segments.len(), hidden: 0 };
        }
    }
    let mut shown = segments.len();
    while shown > 0 {
        let hidden = segments.len() - shown;
        if total(Detail::Hottest, shown) + gap + measure(&more_text(hidden)) <= width {
            break;
        }
        shown -= 1;
    }
    Layout { detail: Detail::Hottest, shown, hidden: segments.len() - shown }
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
        balances: Vec<Balance>,
    ) -> ProviderSnapshot {
        ProviderSnapshot {
            id,
            plan: Some("Pro".into()),
            source: "Codex CLI".into(),
            state,
            windows,
            balances,
            fetched_at: Some(NOW - 60),
            checked_at: NOW,
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            version: Snapshot::VERSION,
            providers: vec![
                provider(
                    ProviderId::Claude,
                    ProviderState::AuthExpired,
                    vec![Window::new("5h", "5ч", 17.0, None)],
                    vec![],
                ),
                provider(
                    ProviderId::ChatGpt,
                    ProviderState::Ok,
                    vec![
                        Window::new("5h", "5ч", 42.0, Some(NOW + 3_900)),
                        Window::new("7d", "7д", 75.0, Some(NOW + 7_500)),
                        Window::new("review", "ревью", 3.0, None),
                    ],
                    vec![Balance::new("credits", "кредиты", 120.0, Unit::Credits, BalanceKind::Remaining)],
                ),
                provider(
                    ProviderId::Zai,
                    ProviderState::UpdateFailed { reason: "нет coding plan".into() },
                    vec![],
                    vec![],
                ),
                provider(ProviderId::Kimi, ProviderState::Idle, vec![], vec![]),
                provider(
                    ProviderId::DeepSeek,
                    ProviderState::Ok,
                    vec![],
                    vec![Balance::new("balance:cny", "баланс", 0.0, Unit::Cny, BalanceKind::Remaining)],
                ),
                provider(
                    ProviderId::OpenRouter,
                    ProviderState::Ok,
                    vec![],
                    vec![Balance::new("spend", "траты", 3.1, Unit::Usd, BalanceKind::Spent).with_limit(Some(10.0))],
                ),
            ],
        }
    }

    fn texts(segment: &Segment, detail: Detail) -> String {
        pieces(segment, detail, true).into_iter().map(|p| p.text).collect()
    }

    #[test]
    fn segments_marks_and_balances() {
        let segments = segments(&snapshot(), &QuotaConfig::default(), NOW);
        let lines: Vec<String> = segments.iter().map(|s| texts(s, Detail::Full)).collect();
        assert_eq!(
            lines,
            vec![
                "Claude ⚠ 5ч 17%",
                "ChatGPT 5ч 42% · 7д 75% ↺ 2ч5м · 120 кр.",
                "Z.ai —",
                "DeepSeek ¥0.00",
                "OpenRouter траты $3.10/$10",
            ],
            "review hidden by default, Idle without data skipped, order follows ProviderId::ALL"
        );
        assert!(segments[0].dim && segments[2].dim && !segments[1].dim);
        assert!(segments[3].items[0].exhausted, "an empty balance is an alarm");
        assert!((segments[4].items[0].pct.unwrap() - 31.0).abs() < 1e-9);
        assert!(segments[2].tooltip.ends_with("ответ провайдера: нет coding plan"), "{}", segments[2].tooltip);
        assert!(segments[1].tooltip.contains("кредиты 120 кр."), "{}", segments[1].tooltip);
    }

    #[test]
    fn user_choices_hide_providers_and_items() {
        let mut config = QuotaConfig::default();
        config.set_provider_enabled("claude", Some(false));
        config.set_window_visible("chatgpt", "review", true);
        config.set_window_visible("chatgpt", "credits", false);
        let segments = segments(&snapshot(), &config, NOW);
        assert_eq!(segments[0].id, ProviderId::ChatGpt);
        assert_eq!(texts(&segments[0], Detail::NoHints), "ChatGPT 5ч 42% · 7д 75% · ревью 3%");
    }

    #[test]
    fn detail_steps_down_then_folds_the_tail() {
        let segments = segments(&snapshot(), &QuotaConfig::default(), NOW);
        let measure = |text: &str| text.chars().count() as f32;
        let width = |detail: Detail| -> f32 {
            segments.iter().map(|s| texts(s, detail).chars().count() as f32).sum::<f32>()
                + 2.0 * (segments.len() - 1) as f32
        };
        let full = width(Detail::Full);
        assert_eq!(layout(&segments, full, 2.0, true, measure), Layout { detail: Detail::Full, shown: 5, hidden: 0 });
        let no_hints = width(Detail::NoHints);
        assert_eq!(layout(&segments, no_hints, 2.0, true, measure).detail, Detail::NoHints);
        let hottest = width(Detail::Hottest);
        assert_eq!(
            layout(&segments, hottest, 2.0, true, measure),
            Layout { detail: Detail::Hottest, shown: 5, hidden: 0 }
        );
        assert_eq!(texts(&segments[1], Detail::Hottest), "ChatGPT 7д 75%");
        // A balance-only provider has no percentage, so every item ties and the
        // documented tie-break (the first item) has to win.
        let balances = [Segment {
            id: ProviderId::Zai,
            name: "Z.ai",
            mark: None,
            dim: false,
            tooltip: String::new(),
            items: vec![
                Item { text: "первый".to_owned(), pct: None, exhausted: false, reset_hint: None },
                Item { text: "второй".to_owned(), pct: None, exhausted: false, reset_hint: None },
            ],
        }];
        assert_eq!(texts(&balances[0], Detail::Hottest), "Z.ai первый");
        let tight = layout(&segments, hottest - 1.0, 2.0, true, measure);
        assert_eq!((tight.detail, tight.shown + tight.hidden), (Detail::Hottest, 5));
        assert!(tight.hidden >= 1);
        let shown_width: f32 =
            segments[..tight.shown].iter().map(|s| texts(s, Detail::Hottest).chars().count() as f32).sum::<f32>()
                + 2.0 * tight.shown as f32
                + more_text(tight.hidden).len() as f32;
        assert!(shown_width <= hottest - 1.0, "the fold fits as well");
        assert_eq!(layout(&segments, 1.0, 2.0, true, measure).shown, 0);
        assert_eq!(layout(&[], 1.0, 2.0, true, measure), Layout { detail: Detail::Full, shown: 0, hidden: 0 });
    }

    #[test]
    fn hints_lose_their_icon_without_the_glyph() {
        let segments = segments(&snapshot(), &QuotaConfig::default(), NOW);
        assert_eq!(
            pieces(&segments[1], Detail::Full, false).iter().map(|p| p.text.as_str()).collect::<String>(),
            "ChatGPT 5ч 42% · 7д 75% 2ч5м · 120 кр."
        );
    }

    #[test]
    fn notes_for_the_settings_page() {
        assert_eq!(state_note(&ProviderState::Ok, NOW), None);
        assert_eq!(
            state_note(&ProviderState::UpdateFailed { reason: "x".into() }, NOW).as_deref(),
            Some("ответ провайдера: x")
        );
        assert_eq!(state_note(&ProviderState::AuthExpired, NOW).as_deref(), Some("вход устарел"));
        assert!(state_note(&ProviderState::RateLimited { retry_at: NOW + 60 }, NOW)
            .unwrap()
            .starts_with("лимит запросов, повтор в "));
    }
}
