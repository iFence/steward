//! Mainland China holiday and adjusted-workday schedules from Timor's free API.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

const HOLIDAY_API_ROOT: &str = "https://timor.tech/api/holiday/year";
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const CACHE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const USER_AGENT: &str = concat!(
    "Steward/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/iFence/steward)"
);

pub(crate) type HolidaySchedule = BTreeMap<String, String>;

#[derive(Debug, Deserialize)]
struct YearResponse {
    code: i32,
    #[serde(default)]
    holiday: BTreeMap<String, HolidayInfo>,
}

#[derive(Debug, Deserialize)]
struct HolidayInfo {
    holiday: bool,
    date: String,
}

pub(crate) fn load_year(year: u16, cache_dir: &Path) -> Result<HolidaySchedule> {
    if !(1900..=2100).contains(&year) {
        bail!("holiday year must be between 1900 and 2100");
    }

    let cache_path = cache_path(cache_dir, year);
    let cached = read_cache(&cache_path);
    if let Some(schedule) = cached.as_ref().filter(|_| cache_is_fresh(&cache_path)) {
        return Ok(schedule.clone());
    }

    match fetch_year(year) {
        Ok(schedule) => {
            if let Err(error) = write_cache(&cache_path, &schedule) {
                tracing::warn!(
                    year,
                    path = %cache_path.display(),
                    %error,
                    "failed to cache calendar holiday schedule"
                );
            }
            Ok(schedule)
        }
        Err(error) => {
            if let Some(schedule) = cached {
                tracing::warn!(
                    year,
                    path = %cache_path.display(),
                    %error,
                    "using stale calendar holiday schedule after refresh failure"
                );
                Ok(schedule)
            } else {
                Err(error)
            }
        }
    }
}

fn fetch_year(year: u16) -> Result<HolidaySchedule> {
    let url = format!("{HOLIDAY_API_ROOT}/{year}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let mut response = agent
        .get(&url)
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("request holiday schedule from {url}"))?;
    let mut body = String::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_string(&mut body)
        .with_context(|| format!("read holiday schedule from {url}"))?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        bail!("holiday schedule response exceeded the size limit");
    }
    parse_year_response(year, &body)
}

fn parse_year_response(year: u16, body: &str) -> Result<HolidaySchedule> {
    let response: YearResponse =
        serde_json::from_str(body).context("parse holiday schedule response")?;
    if response.code != 0 {
        bail!("holiday API returned error code {}", response.code);
    }

    let mut schedule = BTreeMap::new();
    for (month_day, info) in response.holiday {
        if !valid_month_day(&month_day) {
            bail!("holiday API returned an invalid month/day key {month_day:?}");
        }
        let expected_date = format!("{year:04}-{month_day}");
        if info.date != expected_date {
            bail!(
                "holiday API returned date {:?} for {month_day:?}, expected {expected_date:?}",
                info.date
            );
        }
        schedule.insert(
            info.date,
            if info.holiday {
                "holiday".to_string()
            } else {
                "workday".to_string()
            },
        );
    }
    Ok(schedule)
}

fn valid_month_day(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 5 || bytes[2] != b'-' {
        return false;
    }
    let Ok(month) = value[..2].parse::<u8>() else {
        return false;
    };
    let Ok(day) = value[3..].parse::<u8>() else {
        return false;
    };
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

fn cache_path(cache_dir: &Path, year: u16) -> PathBuf {
    cache_dir.join(format!("holiday-schedule-{year}.json"))
}

fn read_cache(path: &Path) -> Option<HolidaySchedule> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn cache_is_fresh(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(cache_age_is_fresh)
}

fn cache_age_is_fresh(age: Duration) -> bool {
    age < CACHE_MAX_AGE
}

fn write_cache(path: &Path, schedule: &HolidaySchedule) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create holiday cache directory {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec(schedule).context("serialize holiday schedule cache")?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes)
        .with_context(|| format!("write holiday cache {}", temporary.display()))?;
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("replace holiday cache {}", path.display()))?;
    }
    fs::rename(&temporary, path).with_context(|| format!("commit holiday cache {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_holidays_and_adjusted_workdays() {
        let body = r#"{
          "code": 0,
          "holiday": {
            "02-10": {
              "holiday": true,
              "name": "春节",
              "date": "2024-02-10"
            },
            "02-04": {
              "holiday": false,
              "name": "春节前补班",
              "date": "2024-02-04"
            }
          }
        }"#;

        let schedule = parse_year_response(2024, body).unwrap();
        assert_eq!(
            schedule.get("2024-02-10").map(String::as_str),
            Some("holiday")
        );
        assert_eq!(
            schedule.get("2024-02-04").map(String::as_str),
            Some("workday")
        );
    }

    #[test]
    fn rejects_a_mismatched_date() {
        let body = r#"{
          "code": 0,
          "holiday": {
            "02-10": {
              "holiday": true,
              "date": "2025-02-10"
            }
          }
        }"#;

        assert!(parse_year_response(2024, body).is_err());
    }

    #[test]
    fn cached_year_is_loaded_without_a_request() {
        let dir = std::env::temp_dir().join(format!(
            "steward-holiday-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let schedule = BTreeMap::from([("2024-02-10".into(), "holiday".into())]);
        write_cache(&cache_path(&dir, 2024), &schedule).unwrap();

        assert_eq!(load_year(2024, &dir).unwrap(), schedule);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn holiday_cache_expires_after_one_week() {
        assert!(cache_age_is_fresh(CACHE_MAX_AGE - Duration::from_secs(1)));
        assert!(!cache_age_is_fresh(CACHE_MAX_AGE));
    }
}
