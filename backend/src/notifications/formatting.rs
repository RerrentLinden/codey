use chrono::{DateTime, Local, TimeZone};

pub(crate) const DISPLAY_TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

pub(crate) fn format_duration(duration_ms: u128) -> String {
    if duration_ms == 0 {
        return "0 秒".to_string();
    }
    if duration_ms < 1_000 {
        return format!("{duration_ms} 毫秒");
    }
    if duration_ms < 60_000 {
        if duration_ms.is_multiple_of(1_000) {
            return format!("{} 秒", duration_ms / 1_000);
        }
        return format!("{:.1} 秒", duration_ms as f64 / 1_000.0);
    }
    let total_seconds = duration_ms / 1_000;
    let seconds = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    if total_minutes < 60 {
        return format!("{total_minutes} 分 {seconds} 秒");
    }
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    format!("{hours} 小时 {minutes} 分 {seconds} 秒")
}

pub(crate) fn format_timestamp(timestamp: &str) -> String {
    let timestamp = timestamp.trim();
    if let Some(datetime) = timestamp
        .strip_prefix("unix-ms:")
        .and_then(|millis| millis.trim().parse::<i64>().ok())
        .and_then(|millis| Local.timestamp_millis_opt(millis).single())
    {
        return datetime.format(DISPLAY_TIMESTAMP_FORMAT).to_string();
    }
    if let Ok(datetime) = DateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S %:z") {
        return datetime.format(DISPLAY_TIMESTAMP_FORMAT).to_string();
    }
    if let Ok(datetime) = DateTime::parse_from_rfc3339(timestamp) {
        return datetime.format(DISPLAY_TIMESTAMP_FORMAT).to_string();
    }
    timestamp.to_string()
}

pub(crate) fn plain_text_value(value: &str, fallback: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        fallback.to_string()
    } else {
        normalized
    }
}

pub(crate) fn markdown_text_value(value: &str, fallback: &str) -> String {
    let normalized = plain_text_value(value, fallback);
    let mut escaped = String::with_capacity(normalized.len());
    for character in normalized.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '*' => escaped.push_str("\\*"),
            '_' => escaped.push_str("\\_"),
            '`' => escaped.push_str("\\`"),
            '[' => escaped.push_str("\\["),
            ']' => escaped.push_str("\\]"),
            '<' => escaped.push('＜'),
            '>' => escaped.push('＞'),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_is_normalized_for_display() {
        assert_eq!(
            format_timestamp("2026-07-21 20:30:00 +08:00"),
            "2026-07-21 20:30:00"
        );
        assert_eq!(
            format_timestamp("2026-07-21T20:30:00+08:00"),
            "2026-07-21 20:30:00"
        );
        let legacy = format_timestamp("unix-ms:1784646600000");
        assert_eq!(legacy.len(), 19);
        assert!(!legacy.contains("unix-ms"));
        assert_eq!(&legacy[4..5], "-");
        assert_eq!(&legacy[7..8], "-");
        assert_eq!(&legacy[10..11], " ");
    }

    #[test]
    fn markdown_text_is_normalized_and_escaped() {
        assert_eq!(
            markdown_text_value(
                "  release <@all> **Codey** [docs] `now` \\ _ok_  ",
                "fallback"
            ),
            "release ＜@all＞ \\*\\*Codey\\*\\* \\[docs\\] \\`now\\` \\\\ \\_ok\\_"
        );
        assert_eq!(markdown_text_value("  ", "fallback"), "fallback");
    }
}
