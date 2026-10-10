//! Turning the history into what `agent-usage status` prints: current meters, burn rates,
//! projections and local token windows, as JSON or as text.

use crate::burn::{self, Burn, Projection, WINDOWS};
use crate::history;
use crate::model::{Sample, Status, Tokens};
use crate::timefmt::{human_duration, human_local, rfc3339_utc};
use crate::transcripts::Index;
use serde::Serialize;

/// One plan meter with its burn.
#[derive(Debug, Clone, Serialize)]
pub struct MeterReport {
    /// Stable meter id.
    pub id: String,
    /// Provider's label.
    pub label: String,
    /// Percentage used.
    pub used_pct: f64,
    /// Percentage left (never below zero).
    pub remaining_pct: f64,
    /// Reset time, Unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// Reset time, RFC 3339 UTC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at_iso: Option<String>,
    /// Seconds until reset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_in_secs: Option<i64>,
    /// Burn per window, from the history (windows without enough samples are left out).
    pub burn: Vec<Burn>,
    /// Where the meter is heading at the most recent usable rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection: Option<Projection>,
}

/// Token use over one window.
#[derive(Debug, Clone, Serialize)]
pub struct TokenWindow {
    /// Window name.
    pub window: &'static str,
    /// Totals inside the window.
    pub tokens: Tokens,
    /// Seconds covered (equal to the window for transcript counts; may be less for counters).
    pub covered_secs: i64,
}

/// Local token burn for one provider.
#[derive(Debug, Clone, Serialize)]
pub struct TokenReport {
    /// `transcripts` (Claude Code) or `thread-totals` (Codex).
    pub source: &'static str,
    /// One entry per window with data.
    pub windows: Vec<TokenWindow>,
}

/// Everything known about one provider.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderReport {
    /// `claude` or `codex`.
    pub provider: String,
    /// Status of the newest plan reading.
    pub status: Status,
    /// Where it came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Why there are no meters, or what failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Plan name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// When the newest reading was taken (Unix seconds).
    pub sampled_at: i64,
    /// Its age in seconds.
    pub age_secs: i64,
    /// Whether this call took the reading (false: served from the history).
    pub fresh: bool,
    /// Plan meters.
    pub meters: Vec<MeterReport>,
    /// Local token burn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenReport>,
    /// Samples of this provider in the history.
    pub history_samples: usize,
}

/// The whole report.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Report time, Unix seconds.
    pub now: i64,
    /// Report time, RFC 3339 UTC.
    pub now_iso: String,
    /// One entry per provider asked for.
    pub providers: Vec<ProviderReport>,
}

fn meter_report(samples: &[Sample], latest: &Sample, now: i64) -> Vec<MeterReport> {
    latest
        .meters
        .iter()
        .map(|m| {
            let points = burn::meter_points(samples, &latest.provider, &m.id);
            let burns = burn::all_windows(&points, now);
            let projection = burn::project(&burns, m.used_pct, m.resets_at, now);
            MeterReport {
                id: m.id.clone(),
                label: m.label.clone(),
                used_pct: m.used_pct,
                remaining_pct: (100.0 - m.used_pct).max(0.0),
                resets_at: m.resets_at,
                resets_at_iso: m.resets_at.map(rfc3339_utc),
                resets_in_secs: m.resets_at.map(|r| r - now),
                burn: burns,
                projection,
            }
        })
        .collect()
}

fn claude_tokens(index: &Index, now: i64) -> TokenReport {
    TokenReport {
        source: "transcripts",
        windows: WINDOWS
            .iter()
            .map(|(name, secs)| TokenWindow {
                window: name,
                tokens: index.window(now, *secs),
                covered_secs: *secs,
            })
            .collect(),
    }
}

fn counter_tokens(samples: &[Sample], provider: &str, now: i64) -> Option<TokenReport> {
    let points = burn::token_points(samples, provider);
    if points.is_empty() {
        return None;
    }
    let windows = burn::all_windows(&points, now)
        .into_iter()
        .map(|b| TokenWindow {
            window: b.window,
            tokens: Tokens {
                total: b.used.round() as u64,
                ..Tokens::default()
            },
            covered_secs: b.covered_secs,
        })
        .collect();
    Some(TokenReport {
        source: "thread-totals",
        windows,
    })
}

/// Build the report for `providers` from the history (and the Claude transcript index, if any).
/// `fresh` names the providers whose newest sample this call took.
pub fn build(
    samples: &[Sample],
    index: Option<&Index>,
    providers: &[&str],
    fresh: &[&str],
    now: i64,
) -> Report {
    let mut out = Vec::new();
    for &provider in providers {
        let Some(latest) = history::latest(samples, provider) else {
            continue;
        };
        let tokens = match (provider, index) {
            ("claude", Some(index)) => Some(claude_tokens(index, now)),
            ("claude", None) => None,
            _ => counter_tokens(samples, provider, now),
        };
        out.push(ProviderReport {
            provider: provider.to_string(),
            status: latest.status,
            source: latest.source.clone(),
            detail: latest.detail.clone(),
            plan: latest.plan.clone(),
            sampled_at: latest.ts,
            age_secs: now - latest.ts,
            fresh: fresh.contains(&provider),
            meters: meter_report(samples, latest, now),
            tokens,
            history_samples: samples.iter().filter(|s| s.provider == provider).count(),
        });
    }
    Report {
        now,
        now_iso: rfc3339_utc(now),
        providers: out,
    }
}

/// `1.2G`, `34M`, `5.6k`, `12`.
pub fn compact(n: u64) -> String {
    let f = n as f64;
    let (value, unit) = if n >= 1_000_000_000 {
        (f / 1e9, "G")
    } else if n >= 1_000_000 {
        (f / 1e6, "M")
    } else if n >= 1_000 {
        (f / 1e3, "k")
    } else {
        return n.to_string();
    };
    if value < 10.0 {
        format!("{value:.1}{unit}")
    } else {
        format!("{value:.0}{unit}")
    }
}

fn pct(v: f64) -> String {
    if v >= 10.0 || v == v.round() {
        format!("{v:.0}%")
    } else {
        format!("{v:.1}%")
    }
}

fn burn_cell(window: &str, b: Option<&Burn>) -> String {
    match b {
        Some(b) if b.covered_secs * 10 < b.window_secs * 9 => format!(
            "{window} +{:.1} ({} seen)",
            b.used,
            human_duration(b.covered_secs)
        ),
        Some(b) => format!("{window} +{:.1}", b.used),
        None => format!("{window} n/a"),
    }
}

/// The report as text for people and agents.
pub fn render(report: &Report) -> String {
    let mut out = String::new();
    let now = report.now;
    if report.providers.is_empty() {
        out.push_str("no readings yet\n");
    }
    for p in &report.providers {
        let age = if p.fresh {
            "read now".to_string()
        } else {
            format!("read {} ago", human_duration(p.age_secs))
        };
        let plan = p
            .plan
            .as_deref()
            .map(|s| format!(" [{s}]"))
            .unwrap_or_default();
        match p.status {
            Status::Ok => out.push_str(&format!(
                "{}{plan}: plan usage ({}, {age})\n",
                p.provider,
                p.source.as_deref().unwrap_or("?")
            )),
            Status::Unavailable => out.push_str(&format!(
                "{}{plan}: no plan limits to read ({age}): {}\n",
                p.provider,
                p.detail.as_deref().unwrap_or("")
            )),
            Status::Error => out.push_str(&format!(
                "{}{plan}: plan usage UNKNOWN ({age}): {}\n",
                p.provider,
                p.detail.as_deref().unwrap_or("")
            )),
        }
        let width = p.meters.iter().map(|m| m.label.len()).max().unwrap_or(0);
        for m in &p.meters {
            let reset = match m.resets_at {
                Some(r) => format!(
                    ", resets {} (in {})",
                    human_local(r, now),
                    human_duration(r - now)
                ),
                None => String::new(),
            };
            out.push_str(&format!(
                "  {:width$}  {:>4} used, {} left{reset}\n",
                m.label,
                pct(m.used_pct),
                pct(m.remaining_pct),
            ));
            let cells: Vec<String> = WINDOWS
                .iter()
                .map(|(w, _)| burn_cell(w, m.burn.iter().find(|b| b.window == *w)))
                .collect();
            out.push_str(&format!("    burn (points): {}\n", cells.join(", ")));
            if let Some(pr) = &m.projection {
                let line = match (pr.secs_to_full, pr.full_before_reset) {
                    (None, _) => format!("idle at the {} rate", pr.basis),
                    (Some(s), Some(true)) => format!(
                        "at {:.1} points/h ({} rate): 100% in {}, BEFORE the reset",
                        pr.per_hour,
                        pr.basis,
                        human_duration(s)
                    ),
                    (Some(s), _) => format!(
                        "at {:.1} points/h ({} rate): 100% in {}, after the reset",
                        pr.per_hour,
                        pr.basis,
                        human_duration(s)
                    ),
                };
                out.push_str(&format!("    {line}\n"));
            }
        }
        if p.status == Status::Ok && p.history_samples < 2 {
            out.push_str(
                "  (burn needs at least two readings: run `agent-usage daemon --detach`)\n",
            );
        }
        if let Some(t) = &p.tokens {
            let what = if t.source == "transcripts" {
                "local tokens (transcripts on this host)"
            } else {
                "local tokens (thread totals, from history)"
            };
            if t.windows.is_empty() {
                out.push_str(&format!("  {what}: need two readings\n"));
            } else {
                out.push_str(&format!("  {what}:\n"));
                for w in &t.windows {
                    let tk = &w.tokens;
                    let seen = if w.covered_secs * 10 < (window_secs(w.window) * 9) {
                        format!(" ({} seen)", human_duration(w.covered_secs))
                    } else {
                        String::new()
                    };
                    if t.source == "transcripts" {
                        out.push_str(&format!(
                            "    {:>3}: {:>5} requests, {:>6} tokens (output {}, cache read {}, cache write {}, input {}){seen}\n",
                            w.window,
                            tk.requests,
                            compact(tk.total),
                            compact(tk.output),
                            compact(tk.cache_read),
                            compact(tk.cache_write),
                            compact(tk.input),
                        ));
                    } else {
                        out.push_str(&format!(
                            "    {:>3}: {:>6} tokens{seen}\n",
                            w.window,
                            compact(tk.total)
                        ));
                    }
                }
            }
        }
    }
    out
}

fn short_id(provider: &str, id: &str) -> String {
    let id = id.strip_prefix(&format!("{provider}:")).unwrap_or(id);
    match id {
        "session" => "5h".into(),
        "weekly_all" | "weekly" => "wk".into(),
        other => other.replace("weekly:", "wk:"),
    }
}

/// The report as one line, for status lines and for agents that want the gist in few tokens:
/// `claude[max] 5h 34% +8.0/h reset 1h59m, wk 22% +1.0/h reset 1d03h | codex no-plan | 1h tokens:
/// claude 244M (823 req), codex 5.1M`.
pub fn render_line(report: &Report) -> String {
    let mut parts = Vec::new();
    let mut tokens = Vec::new();
    for p in &report.providers {
        let plan = p
            .plan
            .as_deref()
            .map(|s| format!("[{s}]"))
            .unwrap_or_default();
        let body = match p.status {
            Status::Ok => p
                .meters
                .iter()
                .map(|m| {
                    let mut cell = format!("{} {}", short_id(&p.provider, &m.id), pct(m.used_pct));
                    if let Some(pr) = &m.projection {
                        cell.push_str(&format!(" +{:.1}/h", pr.per_hour));
                        if pr.full_before_reset == Some(true) {
                            cell.push_str(" FULL-BEFORE-RESET");
                        }
                    }
                    if let Some(r) = m.resets_in_secs {
                        cell.push_str(&format!(" reset {}", human_duration(r)));
                    }
                    cell
                })
                .collect::<Vec<_>>()
                .join(", "),
            Status::Unavailable => "no-plan".into(),
            Status::Error => "UNKNOWN".into(),
        };
        parts.push(format!("{}{plan} {body}", p.provider));
        if let Some(t) = &p.tokens {
            if let Some(w) = t.windows.iter().find(|w| w.window == "1h") {
                let req = if t.source == "transcripts" {
                    format!(" ({} req)", w.tokens.requests)
                } else {
                    String::new()
                };
                tokens.push(format!("{} {}{req}", p.provider, compact(w.tokens.total)));
            }
        }
    }
    if !tokens.is_empty() {
        parts.push(format!("1h tokens: {}", tokens.join(", ")));
    }
    parts.join(" | ")
}

fn window_secs(name: &str) -> i64 {
    WINDOWS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Meter;

    fn sample(ts: i64, used: f64) -> Sample {
        let mut s = Sample::new("claude", ts, Status::Ok);
        s.source = Some("oauth-usage".into());
        s.plan = Some("max".into());
        s.meters.push(Meter {
            id: "session".into(),
            label: "Current session".into(),
            used_pct: used,
            resets_at: Some(ts - ts % 18_000 + 18_000),
            window_mins: Some(300),
        });
        s
    }

    #[test]
    fn report_has_burn_and_projection() {
        // 1_800_000_000 is a multiple of the 18,000 s window, so now is mid-window.
        let now = 1_800_000_000 + 9_000;
        let samples: Vec<_> = (0..=8)
            .map(|i| sample(now - (8 - i) * 900, 10.0 + i as f64))
            .collect();
        let r = build(&samples, None, &["claude", "codex"], &["claude"], now);
        assert_eq!(r.providers.len(), 1);
        let p = &r.providers[0];
        assert!(p.fresh);
        assert_eq!(p.history_samples, 9);
        let m = &p.meters[0];
        assert_eq!(m.used_pct, 18.0);
        assert_eq!(m.remaining_pct, 82.0);
        let b1h = m.burn.iter().find(|b| b.window == "1h").unwrap();
        assert_eq!(b1h.used, 4.0);
        let pr = m.projection.unwrap();
        assert_eq!(pr.basis, "1h");
        assert_eq!(pr.full_before_reset, Some(false));
        let text = render(&r);
        assert!(text.contains("Current session"), "{text}");
        assert!(text.contains("18% used"), "{text}");
        assert!(text.contains("1h +4.0"), "{text}");
        assert!(text.contains("after the reset"), "{text}");
        let line = render_line(&r);
        assert!(
            line.starts_with("claude[max] 5h 18% +4.0/h reset 2h30m"),
            "{line}"
        );
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["providers"][0]["meters"][0]["burn"][1]["window"], "1h");
    }

    #[test]
    fn short_ids() {
        assert_eq!(short_id("claude", "session"), "5h");
        assert_eq!(short_id("claude", "weekly_all"), "wk");
        assert_eq!(short_id("claude", "weekly:Fable"), "wk:Fable");
        assert_eq!(short_id("codex", "codex:5h"), "5h");
        assert_eq!(short_id("codex", "codex:weekly"), "wk");
        assert_eq!(
            short_id("codex", "codex_other:weekly"),
            "codex_other:weekly"
        );
    }

    #[test]
    fn compact_numbers() {
        assert_eq!(compact(12), "12");
        assert_eq!(compact(5_600), "5.6k");
        assert_eq!(compact(34_000_000), "34M");
        assert_eq!(compact(4_337_013_987), "4.3G");
    }
}
