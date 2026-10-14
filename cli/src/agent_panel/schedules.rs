//! Profile-owned scheduled prompts. Due times are advanced before dispatch;
//! missed periods coalesce, and an unconfirmed run is never replayed.
use super::{
    config_transaction::{atomic_write, private_open},
    options::ModelOptions,
};
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Rule {
    Interval { minutes: u32 },
    Daily { time: String },
    Weekdays { time: String },
    Cron { expression: String },
}
fn time(value: &str) -> Result<(u32, u32), String> {
    let (h, m) = value
        .split_once(':')
        .ok_or("Use a local time in HH:MM format")?;
    let h: u32 = h.parse().map_err(|_| "Invalid hour")?;
    let m: u32 = m.parse().map_err(|_| "Invalid minute")?;
    if h > 23 || m > 59 {
        return Err("Time must be between 00:00 and 23:59".into());
    }
    Ok((h, m))
}
impl Rule {
    pub fn next(&self, after: u64) -> Result<u64, String> {
        if let Self::Interval { minutes } = self {
            if *minutes == 0 || *minutes > 525600 {
                return Err("Interval must be 1 through 525600 minutes".into());
            }
            return after
                .checked_add(*minutes as u64 * 60_000)
                .ok_or("Schedule time overflow".into());
        }
        let after_dt = Local
            .timestamp_millis_opt(i64::try_from(after).map_err(|_| "Invalid schedule timestamp")?)
            .single()
            .ok_or("Invalid schedule timestamp")?;
        let cron = match self {
            Self::Daily { time: value } => {
                let (h, m) = time(value)?;
                Cron::parse(&format!("{m} {h} * * *"))?
            }
            Self::Weekdays { time: value } => {
                let (h, m) = time(value)?;
                Cron::parse(&format!("{m} {h} * * 1-5"))?
            }
            Self::Cron { expression } => Cron::parse(expression)?,
            _ => unreachable!(),
        };
        let mut date = after_dt.date_naive();
        for _ in 0..366 * 8 {
            if cron.matches_day(date) {
                for hour in 0..24 {
                    if !cron.fields[1].contains(&hour) {
                        continue;
                    }
                    for minute in 0..60 {
                        if !cron.fields[0].contains(&minute) {
                            continue;
                        }
                        let Some(naive) = date.and_hms_opt(hour, minute, 0) else {
                            continue;
                        };
                        let local = Local.from_local_datetime(&naive);
                        // A nonexistent wall-clock time is skipped. On a fall
                        // back, use the first occurrence strictly after `after`.
                        let candidates = [local.earliest(), local.latest()];
                        let next = candidates
                            .into_iter()
                            .flatten()
                            .map(|d| d.timestamp_millis())
                            .filter(|n| *n > after as i64)
                            .min();
                        if let Some(next) = next {
                            return Ok(next as u64);
                        }
                    }
                }
            }
            date = date.succ_opt().ok_or("Schedule date overflow")?;
        }
        Err("Cron has no matching date within eight years".into())
    }
}
struct Cron {
    fields: Vec<Vec<u32>>,
    dom_any: bool,
    dow_any: bool,
}
impl Cron {
    fn parse(text: &str) -> Result<Self, String> {
        let pieces: Vec<_> = text.split_whitespace().collect();
        if pieces.len() != 5 || text.len() > 256 {
            return Err("Cron requires minute, hour, day, month and weekday fields".into());
        }
        let ranges = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
        let mut fields = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            let (min, max) = ranges[index];
            let mut values = Vec::new();
            let number = |value: &str| -> Result<u32, String> {
                let names: &[&str] = if index == 3 {
                    &[
                        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct",
                        "nov", "dec",
                    ]
                } else if index == 4 {
                    &["sun", "mon", "tue", "wed", "thu", "fri", "sat"]
                } else {
                    &[]
                };
                if let Some(n) = names
                    .iter()
                    .position(|name| *name == value.to_ascii_lowercase())
                {
                    return Ok(n as u32 + u32::from(index == 3));
                }
                value.parse().map_err(|_| "Invalid cron value".into())
            };
            for part in piece.split(',') {
                let (base, step) = if let Some((base, step)) = part.split_once('/') {
                    (base, number(step)?)
                } else {
                    (part, 1)
                };
                if step == 0 || step > max + 1 {
                    return Err("Invalid cron step".into());
                }
                let (lo, hi) = if base == "*" {
                    (min, max)
                } else if let Some((lo, hi)) = base.split_once('-') {
                    (number(lo)?, number(hi)?)
                } else {
                    let n = number(base)?;
                    (n, if part.contains('/') { max } else { n })
                };
                if lo < min || hi > max || lo > hi {
                    return Err("Cron value outside its field range".into());
                }
                for n in (lo..=hi).step_by(step as usize) {
                    values.push(if index == 4 && n == 7 { 0 } else { n });
                }
            }
            values.sort_unstable();
            values.dedup();
            if values.is_empty() {
                return Err("Cron field has no values".into());
            }
            fields.push(values);
        }
        Ok(Self {
            fields,
            dom_any: pieces[2].starts_with('*'),
            dow_any: pieces[4].starts_with('*'),
        })
    }
    fn matches_day(&self, date: NaiveDate) -> bool {
        if !self.fields[3].contains(&date.month()) {
            return false;
        }
        let dom = self.fields[2].contains(&date.day());
        let dow = self.fields[4].contains(&date.weekday().num_days_from_sunday());
        match (self.dom_any, self.dow_any) {
            (false, false) => dom || dow,
            _ => dom && dow,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Schedule {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub agent_id: String,
    pub model: Option<String>,
    #[serde(default)]
    pub options: ModelOptions,
    #[serde(default)]
    pub full_access: bool,
    pub rule: Rule,
    pub enabled: bool,
    pub next_run: u64,
    pub skill_id: Option<String>,
    pub active_chat: Option<String>,
    pub last_chat: Option<String>,
    pub last_result: Option<String>,
    pub last_run: Option<u64>,
    pub finished_at: Option<u64>,
}
impl Schedule {
    pub fn validate(&self) -> Result<(), String> {
        uuid::Uuid::parse_str(&self.id).map_err(|_| "Schedule needs a stable UUID")?;
        if self.name.trim().is_empty()
            || self.name.len() > 160
            || self.prompt.trim().is_empty()
            || self.prompt.len() > 32768
        {
            return Err("Schedule needs a name and 1 through 32768 bytes of prompt text".into());
        }
        if !super::CHAT_AGENTS.contains(&self.agent_id.as_str()) {
            return Err("Unsupported schedule agent".into());
        }
        if self
            .model
            .as_ref()
            .is_some_and(|m| m.is_empty() || m.len() > 256 || m.contains(['\r', '\n', '\0']))
        {
            return Err("Invalid schedule model".into());
        }
        self.options.validate(&self.agent_id)?;
        self.rule.next(super::conversation::now())?;
        if self.options.tool_access.is_none() {
            return Err("Schedules require an explicit allowed-tools mode".into());
        }
        Ok(())
    }
}
pub(super) fn load(directory: &Path) -> Result<BTreeMap<String, Schedule>, String> {
    let path = directory.join("schedules.json");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let mut data = Vec::new();
    private_open(&path, false)?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut data)
        .map_err(|_| "Cannot read schedules")?;
    if data.len() > 4 * 1024 * 1024 {
        return Err("Schedule store exceeds its limit".into());
    }
    let values: BTreeMap<String, Schedule> = serde_json::from_slice(&data)
        .map_err(|_| "Schedule store is corrupt; existing tasks were preserved")?;
    if values.len() > 64 {
        return Err("Schedule store has too many entries".into());
    }
    for (id, value) in &values {
        if id != &value.id {
            return Err("Schedule identity mismatch".into());
        }
        value.validate()?;
    }
    Ok(values)
}
pub(super) fn persist(
    directory: &Path,
    schedules: &BTreeMap<String, Schedule>,
) -> Result<(), String> {
    let data = serde_json::to_vec(schedules).map_err(|_| "Cannot encode schedules")?;
    atomic_write(&directory.join("schedules.json"), &data, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;
    #[test]
    fn cron_calendar_and_dom_dow_semantics_are_checked() {
        let cron = Cron::parse("*/15 9-17 13 jan mon").unwrap();
        assert_eq!(cron.fields[0], vec![0, 15, 30, 45]);
        assert!(cron.matches_day(NaiveDate::from_ymd_opt(2026, 1, 13).unwrap()));
        assert!(cron.matches_day(NaiveDate::from_ymd_opt(2026, 1, 12).unwrap()));
        assert!(!cron.matches_day(NaiveDate::from_ymd_opt(2026, 1, 14).unwrap()));
        for bad in ["* * * *", "*/0 * * * *", "61 * * * *", "* * * 13 *"] {
            assert!(Cron::parse(bad).is_err());
        }
    }
    #[test]
    fn rules_advance_strictly_after_now_and_missed_intervals_coalesce() {
        let now = super::super::conversation::now();
        assert_eq!(
            Rule::Interval { minutes: 15 }.next(now).unwrap(),
            now + 900_000
        );
        assert!(Rule::Interval { minutes: 0 }.next(now).is_err());
        let next = Rule::Cron {
            expression: "* * * * *".into(),
        }
        .next(now)
        .unwrap();
        assert!(next > now && next <= now + 60_000);
        assert_eq!(Local.timestamp_millis_opt(next as i64).unwrap().second(), 0);
        assert!(Rule::Daily {
            time: "24:00".into()
        }
        .next(now)
        .is_err());
    }
}
