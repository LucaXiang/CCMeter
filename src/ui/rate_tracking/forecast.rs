use chrono::Timelike;
use ratatui::style::Color;

use crate::data::oauth::OAuthCredential;
use crate::data::tokens::MinuteTokens;
use crate::ui::theme::Theme;

#[derive(Clone, Copy)]
pub(super) enum PaceStatus {
    Steady,
    Watch,
    SlowDown,
    Critical,
}

impl PaceStatus {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Steady => "Steady",
            Self::Watch => "Watch",
            Self::SlowDown => "Slow down",
            Self::Critical => "Critical",
        }
    }

    pub(super) fn color(self, t: &Theme) -> Color {
        match self {
            Self::Steady => t.lines_positive,
            Self::Watch => t.warning,
            Self::SlowDown => t.efficiency_accent,
            Self::Critical => t.error,
        }
    }
}

pub(super) struct ForecastSnapshot {
    pub(super) utilization_pct: f64,
    pub(super) session_start_local: chrono::NaiveDateTime,
    pub(super) session_end_local: chrono::NaiveDateTime,
    pub(super) elapsed_min: f64,
    pub(super) remaining_min: f64,
    pub(super) session_tokens: u64,
    pub(super) rate_per_min: f64,
    pub(super) safe_rate_per_min: f64,
    pub(super) minutes_to_limit: Option<f64>,
    pub(super) limit_time_local: Option<chrono::DateTime<chrono::Local>>,
    pub(super) projected_pct_at_reset: f64,
    pub(super) status: PaceStatus,
}

pub(super) fn build_forecast_snapshot(
    selected_cred: Option<&OAuthCredential>,
    minute_tokens: &MinuteTokens,
) -> Option<ForecastSnapshot> {
    let cred = selected_cred?;
    let usage = cred.usage.as_ref()?;
    let five_h = usage.five_hour.as_ref()?;

    let utilization = five_h.utilization / 100.0;
    let now_local = chrono::Local::now();
    let (session_start_local, session_end_local) = if let Some(resets_at_str) = &five_h.resets_at
        && let Ok(resets_at) = chrono::DateTime::parse_from_rfc3339(resets_at_str)
    {
        let end = resets_at.with_timezone(&chrono::Local);
        let start = end - chrono::Duration::hours(5);
        (start.naive_local(), end.naive_local())
    } else {
        let end = now_local.naive_local() + chrono::Duration::hours(5);
        let start = now_local.naive_local();
        (start, end)
    };

    let elapsed_min = (now_local.naive_local() - session_start_local)
        .num_seconds()
        .max(0) as f64
        / 60.0;
    let remaining_min = (session_end_local - now_local.naive_local())
        .num_seconds()
        .max(0) as f64
        / 60.0;

    let sample_minutes = elapsed_min.clamp(1.0, 30.0);
    let now_naive = now_local.naive_local();
    let sample_start_local = (now_naive
        - chrono::Duration::seconds((sample_minutes * 60.0).round() as i64))
    .max(session_start_local);
    let recent_tokens = token_sum_in_window(minute_tokens, sample_start_local, now_naive);
    let rate_per_min = recent_tokens as f64 / sample_minutes;
    let session_tokens = token_sum_in_window(minute_tokens, session_start_local, now_naive);

    let util_per_min =
        recent_utilization_per_min(utilization, session_tokens, recent_tokens, sample_minutes);
    let minutes_to_limit = if util_per_min > 0.0 {
        Some((1.0 - utilization) / util_per_min)
    } else {
        None
    };
    let limit_time_local =
        minutes_to_limit.map(|mins| now_local + chrono::Duration::seconds((mins * 60.0) as i64));

    let safe_util_per_min = if remaining_min > 0.0 {
        (1.0 - utilization) / remaining_min
    } else {
        0.0
    };
    let util_per_token = if session_tokens > 0 && utilization > 0.0 {
        utilization / session_tokens as f64
    } else {
        0.0
    };
    let safe_rate_per_min = if util_per_token > 0.0 {
        safe_util_per_min / util_per_token
    } else {
        0.0
    };

    let rate_ratio = if safe_util_per_min > 0.0 {
        util_per_min / safe_util_per_min
    } else if util_per_min > 0.0 {
        f64::INFINITY
    } else {
        0.0
    };
    let status = if rate_ratio < 0.70 {
        PaceStatus::Steady
    } else if rate_ratio < 0.85 {
        PaceStatus::Watch
    } else if rate_ratio < 0.95 {
        PaceStatus::SlowDown
    } else {
        PaceStatus::Critical
    };

    Some(ForecastSnapshot {
        utilization_pct: utilization * 100.0,
        session_start_local,
        session_end_local,
        elapsed_min,
        remaining_min,
        session_tokens,
        rate_per_min,
        safe_rate_per_min,
        minutes_to_limit,
        limit_time_local,
        projected_pct_at_reset: ((utilization + util_per_min * remaining_min) * 100.0)
            .clamp(0.0, 100.0),
        status,
    })
}

pub(super) fn token_sum_in_window(
    minute_tokens: &MinuteTokens,
    start_local: chrono::NaiveDateTime,
    end_local: chrono::NaiveDateTime,
) -> u64 {
    if end_local < start_local {
        return 0;
    }

    let start_key = (
        start_local.date(),
        start_local.hour() as u16 * 60 + start_local.minute() as u16,
    );
    let end_key = (
        end_local.date(),
        end_local.hour() as u16 * 60 + end_local.minute() as u16,
    );

    minute_tokens
        .input
        .iter()
        .chain(minute_tokens.output.iter())
        .filter(|&(&(date, minute), _)| (date, minute) >= start_key && (date, minute) <= end_key)
        .map(|(_, &val)| val)
        .sum()
}

pub(super) fn recent_utilization_per_min(
    utilization: f64,
    session_tokens: u64,
    recent_tokens: u64,
    sample_minutes: f64,
) -> f64 {
    if utilization <= 0.0 || session_tokens == 0 || sample_minutes <= 0.0 {
        return 0.0;
    }
    let util_per_token = utilization / session_tokens as f64;
    recent_tokens as f64 * util_per_token / sample_minutes
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn recent_window_keeps_minutes_from_previous_day() {
        let mut minute_tokens = MinuteTokens::default();
        let day1 = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();
        let day2 = NaiveDate::from_ymd_opt(2026, 4, 11).unwrap();
        minute_tokens.input.insert((day1, 23 * 60 + 50), 120);
        minute_tokens.output.insert((day2, 5), 30);

        let start = day1.and_hms_opt(23, 45, 0).unwrap();
        let end = day2.and_hms_opt(0, 5, 0).unwrap();

        assert_eq!(token_sum_in_window(&minute_tokens, start, end), 150);
    }

    #[test]
    fn recent_utilization_rate_uses_recent_pace() {
        let util = recent_utilization_per_min(0.5, 1_000, 200, 10.0);
        assert!((util - 0.01).abs() < 1e-9);
    }
}
