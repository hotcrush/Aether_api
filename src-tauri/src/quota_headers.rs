use chrono::{DateTime, Utc};
use reqwest::header::HeaderMap;
use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct CodexQuotaWindowSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_window_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_after_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CodexQuotaHeaderSnapshot {
    pub primary_window: Option<CodexQuotaWindowSnapshot>,
    pub secondary_window: Option<CodexQuotaWindowSnapshot>,
    pub primary_over_secondary_limit_percent: Option<f64>,
    pub fetched_at: i64,
}

#[cfg(test)]
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NormalizedCodexQuotaWindows {
    pub five_hour: Option<CodexQuotaWindowSnapshot>,
    pub seven_day: Option<CodexQuotaWindowSnapshot>,
}

#[cfg(test)]
impl CodexQuotaHeaderSnapshot {
    /// Convert the unstable primary/secondary labels to canonical 5h/7d slots.
    /// Window duration wins; the legacy primary=7d/secondary=5h mapping is only
    /// used when the upstream omits both durations.
    pub(crate) fn normalized_windows(&self) -> NormalizedCodexQuotaWindows {
        normalize_windows(self.primary_window.clone(), self.secondary_window.clone())
    }
}

pub(crate) fn parse_codex_quota_headers(headers: &HeaderMap) -> Option<CodexQuotaHeaderSnapshot> {
    parse_codex_quota_headers_at(headers, Utc::now().timestamp())
}

fn parse_codex_quota_headers_at(
    headers: &HeaderMap,
    fetched_at: i64,
) -> Option<CodexQuotaHeaderSnapshot> {
    let primary_window = parse_window(headers, "primary", fetched_at);
    let secondary_window = parse_window(headers, "secondary", fetched_at);
    let primary_over_secondary_limit_percent =
        parse_percent(headers, "x-codex-primary-over-secondary-limit-percent");
    if primary_window.is_none()
        && secondary_window.is_none()
        && primary_over_secondary_limit_percent.is_none()
    {
        return None;
    }
    Some(CodexQuotaHeaderSnapshot {
        primary_window,
        secondary_window,
        primary_over_secondary_limit_percent,
        fetched_at,
    })
}

fn parse_window(
    headers: &HeaderMap,
    slot: &str,
    fetched_at: i64,
) -> Option<CodexQuotaWindowSnapshot> {
    let used_percent = parse_percent(headers, &format!("x-codex-{slot}-used-percent"));
    let window_minutes = parse_positive_i64(headers, &format!("x-codex-{slot}-window-minutes"));
    let reset_after_seconds =
        parse_nonnegative_i64(headers, &format!("x-codex-{slot}-reset-after-seconds"));
    let explicit_reset_at =
        header_text(headers, &format!("x-codex-{slot}-reset-at")).and_then(parse_reset_at);
    if used_percent.is_none()
        && window_minutes.is_none()
        && reset_after_seconds.is_none()
        && explicit_reset_at.is_none()
    {
        return None;
    }
    let limit_window_seconds = window_minutes.and_then(|minutes| minutes.checked_mul(60));
    let reset_at = explicit_reset_at
        .or_else(|| reset_after_seconds.map(|seconds| fetched_at.saturating_add(seconds)));
    Some(CodexQuotaWindowSnapshot {
        used_percent,
        remaining_percent: used_percent.map(|used| (100.0 - used).clamp(0.0, 100.0)),
        limit_window_seconds,
        reset_after_seconds,
        reset_at,
    })
}

#[cfg(test)]
fn normalize_windows(
    primary: Option<CodexQuotaWindowSnapshot>,
    secondary: Option<CodexQuotaWindowSnapshot>,
) -> NormalizedCodexQuotaWindows {
    match (primary, secondary) {
        (Some(primary), Some(secondary)) => {
            let primary_seconds = primary.limit_window_seconds;
            let secondary_seconds = secondary.limit_window_seconds;
            match (primary_seconds, secondary_seconds) {
                (Some(primary_seconds), Some(secondary_seconds))
                    if primary_seconds < secondary_seconds =>
                {
                    NormalizedCodexQuotaWindows {
                        five_hour: Some(primary),
                        seven_day: Some(secondary),
                    }
                }
                (Some(primary_seconds), Some(secondary_seconds))
                    if secondary_seconds < primary_seconds =>
                {
                    NormalizedCodexQuotaWindows {
                        five_hour: Some(secondary),
                        seven_day: Some(primary),
                    }
                }
                (Some(primary_seconds), _) if is_short_window(primary_seconds) => {
                    NormalizedCodexQuotaWindows {
                        five_hour: Some(primary),
                        seven_day: Some(secondary),
                    }
                }
                (_, Some(secondary_seconds)) if !is_short_window(secondary_seconds) => {
                    NormalizedCodexQuotaWindows {
                        five_hour: Some(primary),
                        seven_day: Some(secondary),
                    }
                }
                _ => NormalizedCodexQuotaWindows {
                    five_hour: Some(secondary),
                    seven_day: Some(primary),
                },
            }
        }
        (Some(primary), None) => normalize_single(primary, true),
        (None, Some(secondary)) => normalize_single(secondary, false),
        (None, None) => NormalizedCodexQuotaWindows::default(),
    }
}

#[cfg(test)]
fn normalize_single(
    window: CodexQuotaWindowSnapshot,
    primary_slot: bool,
) -> NormalizedCodexQuotaWindows {
    let is_short = window
        .limit_window_seconds
        .map(is_short_window)
        .unwrap_or(!primary_slot);
    if is_short {
        NormalizedCodexQuotaWindows {
            five_hour: Some(window),
            seven_day: None,
        }
    } else {
        NormalizedCodexQuotaWindows {
            five_hour: None,
            seven_day: Some(window),
        }
    }
}

#[cfg(test)]
fn is_short_window(seconds: i64) -> bool {
    seconds <= 6 * 60 * 60
}

fn parse_percent(headers: &HeaderMap, name: &str) -> Option<f64> {
    let value = header_text(headers, name)?.parse::<f64>().ok()?;
    value.is_finite().then(|| value.clamp(0.0, 100.0))
}

fn parse_positive_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    header_text(headers, name)?
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
}

fn parse_nonnegative_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    header_text(headers, name)?
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim)
}

fn parse_reset_at(value: &str) -> Option<i64> {
    if let Ok(timestamp) = value.parse::<i64>() {
        return Some(if timestamp > 10_000_000_000 {
            timestamp / 1_000
        } else {
            timestamp
        });
    }
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    fn headers(values: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in values {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn parses_and_normalizes_swapped_windows() {
        let headers = headers(&[
            ("x-codex-primary-used-percent", "12.5"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-after-seconds", "90"),
            ("x-codex-secondary-used-percent", "72"),
            ("x-codex-secondary-window-minutes", "10080"),
            ("x-codex-secondary-reset-after-seconds", "3600"),
        ]);
        let snapshot = parse_codex_quota_headers_at(&headers, 1_000).unwrap();
        assert_eq!(
            snapshot
                .primary_window
                .as_ref()
                .and_then(|window| window.remaining_percent),
            Some(87.5)
        );
        assert_eq!(
            snapshot
                .primary_window
                .as_ref()
                .and_then(|window| window.reset_at),
            Some(1_090)
        );
        let normalized = snapshot.normalized_windows();
        assert_eq!(
            normalized.five_hour.and_then(|window| window.used_percent),
            Some(12.5)
        );
        assert_eq!(
            normalized.seven_day.and_then(|window| window.used_percent),
            Some(72.0)
        );
    }

    #[test]
    fn accepts_partial_headers_and_ignores_standard_api_rate_limits() {
        let mixed = headers(&[
            ("x-ratelimit-remaining-tokens", "149984"),
            ("x-codex-secondary-used-percent", "105"),
            ("x-codex-secondary-reset-at", "2026-07-31T10:00:00Z"),
        ]);
        let snapshot = parse_codex_quota_headers_at(&mixed, 1_000).unwrap();
        let secondary = snapshot.secondary_window.unwrap();
        assert_eq!(secondary.used_percent, Some(100.0));
        assert_eq!(secondary.remaining_percent, Some(0.0));
        assert_eq!(secondary.reset_at, Some(1_785_492_000));

        let only_standard = headers(&[("x-ratelimit-remaining-tokens", "149984")]);
        assert!(parse_codex_quota_headers_at(&only_standard, 1_000).is_none());
    }

    #[test]
    fn legacy_labels_fall_back_to_secondary_five_hour_primary_seven_day() {
        let headers = headers(&[
            ("x-codex-primary-used-percent", "80"),
            ("x-codex-secondary-used-percent", "20"),
        ]);
        let normalized = parse_codex_quota_headers_at(&headers, 1_000)
            .unwrap()
            .normalized_windows();
        assert_eq!(
            normalized.five_hour.and_then(|window| window.used_percent),
            Some(20.0)
        );
        assert_eq!(
            normalized.seven_day.and_then(|window| window.used_percent),
            Some(80.0)
        );
    }
}
