//! The JSON payload sent to the webhook.
//!
//! Units are metric throughout: °C, km/h, hPa, mm, %.
//!
//! `hourly` and `daily` entries only use keys from Home Assistant's
//! `Forecast` dict, so both lists can be handed to a template weather entity
//! unchanged (it rejects forecasts containing unknown keys).

use chrono::{DateTime, SecondsFormat};
use chrono_tz::Tz;
use serde::{Serialize, Serializer};

use crate::condition::Condition;

#[derive(Debug, Serialize)]
pub struct Report {
    pub location: Location,
    /// The page the data was scraped from.
    pub source: String,
    /// When the page was fetched.
    #[serde(serialize_with = "rfc3339")]
    pub updated: DateTime<Tz>,
    pub current: Current,
    pub hourly: Vec<HourlyForecast>,
    pub daily: Vec<DailyForecast>,
}

#[derive(Debug, Serialize)]
pub struct Location {
    pub id: Option<String>,
    pub name: String,
    pub district: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Current {
    pub condition: Option<Condition>,
    /// The page's own (Polish) description of the condition.
    pub description: Option<String>,
    pub temperature: f64,
    pub apparent_temperature: Option<f64>,
    /// The page shows no observed humidity, so this is the value of the
    /// nearest forecast hour.
    pub humidity: Option<f64>,
    pub pressure: Option<f64>,
    /// As labelled by the page, e.g. "steady".
    pub pressure_trend: Option<String>,
    pub wind_speed: Option<f64>,
    /// Degrees.
    pub wind_bearing: Option<f64>,
    /// Compass point, e.g. "WSW".
    pub wind_direction: Option<String>,
    /// Local time, "HH:MM".
    pub sunrise: Option<String>,
    pub sunset: Option<String>,
    pub moonrise: Option<String>,
    pub moonset: Option<String>,
    pub air_quality: Option<AirQuality>,
}

#[derive(Debug, Serialize)]
pub struct AirQuality {
    /// Common Air Quality Index.
    pub caqi: Option<f64>,
    pub description: Option<String>,
    /// Percent of the norm.
    pub pm25_percent: Option<f64>,
    pub pm10_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HourlyForecast {
    #[serde(serialize_with = "rfc3339")]
    pub datetime: DateTime<Tz>,
    pub condition: Option<Condition>,
    pub native_temperature: f64,
    pub native_apparent_temperature: Option<f64>,
    /// Rain plus the liquid equivalent of snow, in mm.
    pub native_precipitation: Option<f64>,
    pub native_wind_speed: Option<f64>,
    pub native_wind_gust_speed: Option<f64>,
    pub wind_bearing: Option<f64>,
    pub humidity: Option<f64>,
    pub cloud_coverage: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailyForecast {
    /// Local midnight of the day.
    #[serde(serialize_with = "rfc3339")]
    pub datetime: DateTime<Tz>,
    pub condition: Option<Condition>,
    /// Daily high.
    pub native_temperature: f64,
    /// Daily low.
    pub native_templow: f64,
    pub native_precipitation: Option<f64>,
    pub native_wind_speed: Option<f64>,
    pub native_wind_gust_speed: Option<f64>,
    pub wind_bearing: Option<f64>,
    /// Not on the long-term page; only set when the day is derived from the
    /// hourly forecast.
    pub humidity: Option<f64>,
    pub cloud_coverage: Option<f64>,
    /// Only on the long-term page.
    pub uv_index: Option<f64>,
}

fn rfc3339<S: Serializer>(dt: &DateTime<Tz>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&dt.to_rfc3339_opts(SecondsFormat::Secs, false))
}
