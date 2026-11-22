//! Selection of the daily forecast to send.
//!
//! The daily forecast normally comes from the long-term page. When that page
//! is unavailable it is derived from the hourly forecast instead; the hourly
//! page starts with the next full hour, so the first derived day usually
//! covers just the remainder of today.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Timelike};
use chrono_tz::Tz;

use crate::condition::Condition;
use crate::model::{DailyForecast, HourlyForecast};
use crate::parser::round1;

/// Hours (local time) that decide a day's condition, so that a clear night
/// does not outvote a rainy day.
const DAYTIME_HOURS: std::ops::RangeInclusive<u32> = 7..=18;

/// The days from `today` on, at most `limit` of them.
///
/// The long-term forecast is fetched less often than it is sent, so it can
/// still start with a day that has passed.
pub fn upcoming(days: &[DailyForecast], today: NaiveDate, limit: usize) -> Vec<DailyForecast> {
    days.iter()
        .filter(|day| day.datetime.date_naive() >= today)
        .take(limit)
        .cloned()
        .collect()
}

/// Derives a daily forecast by grouping consecutive hourly entries by local
/// date.
pub fn summarize(hourly: &[HourlyForecast]) -> Vec<DailyForecast> {
    hourly
        .chunk_by(|a, b| a.datetime.date_naive() == b.datetime.date_naive())
        .map(summarize_day)
        .collect()
}

fn summarize_day(hours: &[HourlyForecast]) -> DailyForecast {
    let first = hours[0].datetime;
    let temperatures = || hours.iter().map(|h| h.native_temperature);
    let windiest = hours
        .iter()
        .filter(|h| h.native_wind_speed.is_some())
        .max_by(|a, b| {
            a.native_wind_speed
                .partial_cmp(&b.native_wind_speed)
                .unwrap()
        });

    DailyForecast {
        datetime: local_midnight(first),
        condition: day_condition(hours),
        native_temperature: temperatures().fold(f64::MIN, f64::max),
        native_templow: temperatures().fold(f64::MAX, f64::min),
        native_precipitation: sum(hours.iter().filter_map(|h| h.native_precipitation)).map(round1),
        native_wind_speed: windiest.and_then(|h| h.native_wind_speed),
        native_wind_gust_speed: hours
            .iter()
            .filter_map(|h| h.native_wind_gust_speed)
            .reduce(f64::max),
        wind_bearing: windiest.and_then(|h| h.wind_bearing),
        humidity: mean(hours.iter().filter_map(|h| h.humidity)),
        cloud_coverage: mean(hours.iter().filter_map(|h| h.cloud_coverage)),
        uv_index: None,
    }
}

/// The most frequent daytime condition; precipitation takes over once it
/// covers at least a quarter of those hours. Ties go to the more severe
/// condition.
fn day_condition(hours: &[HourlyForecast]) -> Option<Condition> {
    let conditions = |daytime_only: bool| -> Vec<Condition> {
        hours
            .iter()
            .filter(|h| !daytime_only || DAYTIME_HOURS.contains(&h.datetime.hour()))
            .filter_map(|h| h.condition)
            .collect()
    };
    let mut considered = conditions(true);
    if considered.is_empty() {
        considered = conditions(false);
    }

    let wet = considered.iter().filter(|c| c.is_precipitation()).count();
    if wet > 0 && wet * 4 >= considered.len() {
        considered.retain(|c| c.is_precipitation());
    }

    let mut counts: HashMap<Condition, usize> = HashMap::new();
    for condition in considered {
        *counts.entry(condition).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|&(condition, count)| (count, condition))
        .map(|(condition, _)| condition)
}

fn local_midnight(dt: DateTime<Tz>) -> DateTime<Tz> {
    dt.timezone()
        .from_local_datetime(&dt.date_naive().and_time(NaiveTime::MIN))
        .earliest()
        .unwrap_or(dt)
}

fn sum(values: impl Iterator<Item = f64>) -> Option<f64> {
    values.fold(None, |total, v| Some(total.unwrap_or(0.0) + v))
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (total, count) = values.fold((0.0, 0u32), |(total, count), v| (total + v, count + 1));
    (count > 0).then(|| (total / f64::from(count)).round())
}

#[cfg(test)]
mod tests {
    use chrono::SecondsFormat;

    use super::*;

    const TZ: Tz = chrono_tz::Europe::Warsaw;

    fn hour(day: u32, hour: u32, temperature: f64, condition: Condition) -> HourlyForecast {
        HourlyForecast {
            datetime: TZ.with_ymd_and_hms(2026, 11, day, hour, 0, 0).unwrap(),
            condition: Some(condition),
            native_temperature: temperature,
            native_apparent_temperature: None,
            native_precipitation: Some(0.0),
            native_wind_speed: Some(10.0),
            native_wind_gust_speed: Some(20.0),
            wind_bearing: Some(90.0),
            humidity: Some(80.0),
            cloud_coverage: Some(50.0),
        }
    }

    #[test]
    fn groups_hours_by_local_date() {
        let hourly = [
            hour(22, 22, 3.0, Condition::Cloudy),
            hour(22, 23, 2.0, Condition::Cloudy),
            hour(23, 0, 1.0, Condition::Cloudy),
            hour(23, 12, 6.0, Condition::Sunny),
        ];
        let daily = summarize(&hourly);
        assert_eq!(daily.len(), 2);

        let iso = |dt: DateTime<Tz>| dt.to_rfc3339_opts(SecondsFormat::Secs, false);
        assert_eq!(iso(daily[0].datetime), "2026-11-22T00:00:00+01:00");
        assert_eq!(daily[0].native_temperature, 3.0);
        assert_eq!(daily[0].native_templow, 2.0);
        // Only night hours are left, so they decide.
        assert_eq!(daily[0].condition, Some(Condition::Cloudy));

        assert_eq!(iso(daily[1].datetime), "2026-11-23T00:00:00+01:00");
        assert_eq!(daily[1].native_temperature, 6.0);
        assert_eq!(daily[1].native_templow, 1.0);
        // The night hour is outside the daytime window.
        assert_eq!(daily[1].condition, Some(Condition::Sunny));
    }

    #[test]
    fn aggregates_wind_precipitation_and_averages() {
        let mut calm = hour(22, 9, 1.0, Condition::Cloudy);
        calm.native_precipitation = Some(0.3);
        calm.humidity = Some(71.0);
        let mut gale = hour(22, 10, 1.0, Condition::Cloudy);
        gale.native_precipitation = Some(1.2);
        gale.native_wind_speed = Some(40.0);
        gale.native_wind_gust_speed = Some(75.0);
        gale.wind_bearing = Some(270.0);
        gale.humidity = Some(90.0);

        let day = &summarize(&[calm, gale])[0];
        assert_eq!(day.native_precipitation, Some(1.5));
        assert_eq!(day.native_wind_speed, Some(40.0));
        assert_eq!(day.native_wind_gust_speed, Some(75.0));
        assert_eq!(day.wind_bearing, Some(270.0));
        assert_eq!(day.humidity, Some(81.0));
        assert_eq!(day.cloud_coverage, Some(50.0));
    }

    #[test]
    fn precipitation_wins_from_a_quarter_of_the_day() {
        let day = |wet_hours: u32| {
            let hourly: Vec<_> = (7..=18)
                .map(|h| {
                    let condition = if h < 7 + wet_hours {
                        Condition::Snowy
                    } else {
                        Condition::Cloudy
                    };
                    hour(22, h, 0.0, condition)
                })
                .collect();
            summarize(&hourly)[0].condition
        };
        assert_eq!(day(2), Some(Condition::Cloudy));
        assert_eq!(day(3), Some(Condition::Snowy));
    }

    #[test]
    fn ties_go_to_the_more_severe_condition() {
        let hourly = [
            hour(22, 9, 0.0, Condition::Sunny),
            hour(22, 10, 0.0, Condition::Cloudy),
        ];
        assert_eq!(summarize(&hourly)[0].condition, Some(Condition::Cloudy));
    }

    #[test]
    fn selects_upcoming_days() {
        let days = summarize(&[
            hour(22, 12, 1.0, Condition::Cloudy),
            hour(23, 12, 2.0, Condition::Cloudy),
            hour(24, 12, 3.0, Condition::Cloudy),
            hour(25, 12, 4.0, Condition::Cloudy),
        ]);
        let highs = |today: u32, limit| -> Vec<f64> {
            let today = NaiveDate::from_ymd_opt(2026, 11, today).unwrap();
            upcoming(&days, today, limit)
                .iter()
                .map(|day| day.native_temperature)
                .collect()
        };
        assert_eq!(highs(22, 10), [1.0, 2.0, 3.0, 4.0]);
        // A forecast fetched yesterday no longer starts with today.
        assert_eq!(highs(23, 10), [2.0, 3.0, 4.0]);
        assert_eq!(highs(23, 2), [2.0, 3.0]);
        assert_eq!(highs(26, 10), [] as [f64; 0]);
    }
}
