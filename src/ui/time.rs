use chrono::{DateTime, FixedOffset, Local, Offset, TimeZone};

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const WEEK: i64 = 7 * DAY;

pub(super) fn format_unix_local(value: Option<i64>, now: i64) -> String {
    value.map_or_else(
        || "Time unavailable".into(),
        |timestamp| {
            let offset = Local
                .timestamp_opt(timestamp, 0)
                .single()
                .map(|date| date.offset().fix())
                .unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC is valid"));
            format_unix_at(timestamp, now, offset)
        },
    )
}

pub(super) fn format_unix_at(timestamp: i64, now: i64, offset: FixedOffset) -> String {
    let age = now.saturating_sub(timestamp);
    if age < 0 {
        if age >= -MINUTE {
            return "Just now".into();
        }
        return absolute_local(timestamp, offset);
    }
    if (0..MINUTE).contains(&age) {
        return "Just now".into();
    }
    if age < HOUR {
        return format!("{} min ago", age / MINUTE);
    }
    if age < DAY {
        return format!("{} hr ago", age / HOUR);
    }
    if age < WEEK {
        return format!("{} days ago", age / DAY);
    }
    absolute_local(timestamp, offset)
}

fn absolute_local(timestamp: i64, offset: FixedOffset) -> String {
    offset
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|date: DateTime<FixedOffset>| date.format("%b %-d, %Y, %-I:%M %p").to_string())
        .unwrap_or_else(|| "Time unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_times_are_relative_and_boundaries_are_stable() {
        let offset = FixedOffset::east_opt(10 * 3600).unwrap();
        assert_eq!(format_unix_at(1_000, 1_030, offset), "Just now");
        assert_eq!(format_unix_at(1_000, 1_120, offset), "2 min ago");
        assert_eq!(format_unix_at(1_000, 8_200, offset), "2 hr ago");
        assert_eq!(format_unix_at(1_000, 173_800, offset), "2 days ago");
    }

    #[test]
    fn older_times_use_the_supplied_local_offset() {
        let offset = FixedOffset::east_opt(10 * 3600).unwrap();
        assert_eq!(format_unix_at(0, WEEK, offset), "Jan 1, 1970, 10:00 AM");
    }

    #[test]
    fn future_times_never_render_negative_ago_values() {
        let offset = FixedOffset::east_opt(10 * 3600).unwrap();
        assert_eq!(format_unix_at(1_030, 1_000, offset), "Just now");
        assert_eq!(format_unix_at(WEEK, 0, offset), "Jan 8, 1970, 10:00 AM");
    }
}
