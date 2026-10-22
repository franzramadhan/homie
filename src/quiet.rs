//! Quiet hours: a daily window (may cross midnight) in which scheduled-off
//! integrations show as "scheduled off" instead of down.

use jiff::{Timestamp, tz::TimeZone};

use crate::Result;

pub struct Quiet {
    pub window: String,
    pub timezone: String,
    start: u16,
    end: u16,
    tz: TimeZone,
}

fn minutes(hhmm: &str) -> Result<u16> {
    let (h, m) = hhmm.trim().split_once(':').ok_or("expected HH:MM")?;
    let (h, m): (u16, u16) = (h.parse()?, m.parse()?);
    if h > 23 || m > 59 {
        return Err(format!("{hhmm} is not a time of day").into());
    }
    Ok(h * 60 + m)
}

impl Quiet {
    pub fn new(window: &str, timezone: Option<&str>) -> Result<Self> {
        let (a, b) = window
            .split_once('-')
            .ok_or("quiet_hours.window must look like 23:30-06:00")?;
        let (start, end) = (minutes(a)?, minutes(b)?);
        if start == end {
            return Err("quiet_hours.window start and end are equal".into());
        }
        let tz = match timezone {
            Some(name) => TimeZone::get(name).map_err(|e| format!("quiet_hours.timezone: {e}"))?,
            None => TimeZone::system(),
        };
        Ok(Self {
            window: window.to_string(),
            timezone: tz.iana_name().unwrap_or("local").to_string(),
            start,
            end,
            tz,
        })
    }

    pub fn active_at(&self, ts: Timestamp) -> bool {
        let t = ts.to_zoned(self.tz.clone());
        let now = t.hour() as u16 * 60 + t.minute() as u16;
        if self.start < self.end {
            self.start <= now && now < self.end
        } else {
            now >= self.start || now < self.end
        }
    }

    pub fn active(&self) -> bool {
        self.active_at(Timestamp::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(q: &Quiet, local: &str) -> bool {
        let ts = jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M", local)
            .unwrap()
            .to_zoned(q.tz.clone())
            .unwrap()
            .timestamp();
        q.active_at(ts)
    }

    #[test]
    fn window_crossing_midnight() {
        let q = Quiet::new("23:30-06:00", Some("Asia/Jakarta")).unwrap();
        assert!(at(&q, "2026-10-22 23:30"));
        assert!(at(&q, "2026-10-23 02:00"));
        assert!(!at(&q, "2026-10-23 06:00"));
        assert!(!at(&q, "2026-10-22 12:00"));
        // 17:00 UTC is 00:00 in Jakarta
        assert!(q.active_at("2026-10-22T17:00:00Z".parse().unwrap()));
    }

    #[test]
    fn window_same_day_and_errors() {
        let q = Quiet::new("01:00-05:00", Some("UTC")).unwrap();
        assert!(at(&q, "2026-10-22 01:00"));
        assert!(!at(&q, "2026-10-22 05:00"));
        assert!(Quiet::new("10:00-10:00", None).is_err());
        assert!(Quiet::new("25:00-01:00", None).is_err());
        assert!(Quiet::new("23:30-06:00", Some("Not/AZone")).is_err());
    }

    #[test]
    fn parse_errors_and_system_zone() {
        let err = |w: &str| Quiet::new(w, Some("UTC")).err().unwrap().to_string();
        assert_eq!(
            err("23:30"),
            "quiet_hours.window must look like 23:30-06:00"
        );
        assert_eq!(err("2330-06:00"), "expected HH:MM");
        assert_eq!(err("10:60-11:00"), "10:60 is not a time of day");
        assert!(Quiet::new("aa:00-01:00", None).is_err());
        assert!(Quiet::new("01:bb-02:00", None).is_err());

        let q = Quiet::new(" 01:00 - 02:00 ", None).unwrap();
        assert_eq!(q.window, " 01:00 - 02:00 ");
        assert!(!q.timezone.is_empty());
        let always = Quiet::new("00:00-23:59", Some("UTC")).unwrap();
        assert_eq!(always.timezone, "UTC");
        let now = Timestamp::now().to_zoned(TimeZone::UTC);
        assert_eq!(always.active(), (now.hour(), now.minute()) != (23, 59));
    }
}
