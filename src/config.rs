//! Configuration from environment variables.

use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use chrono_tz::Tz;

const SITE: &str = "https://pogoda.interia.pl/";
/// Path prefix of the hour-by-hour forecast page.
const HOURLY_PAGE: &str = "prognoza-szczegolowa-";
/// Path prefix of the 45-day forecast page.
const DAILY_PAGE: &str = "prognoza-dlugoterminowa-";
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_hours(1);
const DEFAULT_DAILY_REFRESH_INTERVAL: Duration = Duration::from_hours(6);
/// The site refreshes its data every 30 minutes; polling much faster than
/// that only loads their servers.
const MIN_REFRESH_INTERVAL: Duration = Duration::from_mins(1);
const DEFAULT_DAILY_DAYS: usize = 14;
/// The long-term page lists 45 days.
const MAX_DAILY_DAYS: usize = 45;
const DEFAULT_TIMEZONE: Tz = chrono_tz::Europe::Warsaw;

pub const USAGE: &str = "\
Usage: weatherd [--once] [--dry-run]

Scrapes the pogoda.interia.pl hourly and daily forecasts and POSTs them as
JSON to a Home Assistant webhook.

Options:
  --once      fetch and push a single time, then exit
  --dry-run   print the JSON payload to stdout instead of pushing it
  -h, --help  show this help

Environment:
  LOCATION          required; the location part of the forecast URL, e.g.
                    `warszawa,cId,36917`
  WEBHOOK_URL       required unless --dry-run; e.g.
                    `http://homeassistant/api/webhook/<webhook_id>`
  REFRESH_INTERVAL  optional; how often to fetch the current conditions and
                    hourly forecast and push, e.g. `30m`, `1h`, `1h 30m`
                    (default: 1h)
  DAILY_REFRESH_INTERVAL
                    optional; how often to fetch the daily (45-day) forecast;
                    it is checked on every refresh (default: 6h)
  DAILY_DAYS        optional; number of days of daily forecast to push,
                    1-45 (default: 14)
  TZ                optional; time zone of the times shown on the page
                    (default: Europe/Warsaw)";

#[derive(Debug, PartialEq)]
pub struct Config {
    /// The hour-by-hour forecast page.
    pub page_url: String,
    /// The 45-day forecast page. `None` when `LOCATION` is a URL it cannot
    /// be derived from.
    pub daily_page_url: Option<String>,
    /// `None` only in dry-run mode.
    pub webhook_url: Option<String>,
    pub refresh_interval: Duration,
    pub daily_refresh_interval: Duration,
    pub daily_days: usize,
    pub timezone: Tz,
}

impl Config {
    pub fn from_env(require_webhook: bool) -> Result<Self> {
        Self::load(|name| std::env::var(name).ok(), require_webhook)
    }

    fn load(get: impl Fn(&str) -> Option<String>, require_webhook: bool) -> Result<Self> {
        // Compose passes unset `${VAR}` references as empty strings.
        let get = |name: &str| {
            get(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };

        let location = get("LOCATION").context("LOCATION is not set")?;
        let page_url = page_url(&location)?;

        let webhook_url = get("WEBHOOK_URL");
        if let Some(url) = &webhook_url {
            ensure!(
                is_http_url(url),
                "WEBHOOK_URL must start with http:// or https://"
            );
        } else if require_webhook {
            bail!("WEBHOOK_URL is not set");
        }

        let interval = |name: &str, default: Duration| -> Result<Duration> {
            let interval = match get(name) {
                Some(value) => humantime::parse_duration(&value).with_context(|| {
                    format!("{name} `{value}` is not a duration like `30m` or `1h`")
                })?,
                None => default,
            };
            ensure!(
                interval >= MIN_REFRESH_INTERVAL,
                "{name} must be at least {}",
                humantime::format_duration(MIN_REFRESH_INTERVAL)
            );
            Ok(interval)
        };
        let refresh_interval = interval("REFRESH_INTERVAL", DEFAULT_REFRESH_INTERVAL)?;
        let daily_refresh_interval =
            interval("DAILY_REFRESH_INTERVAL", DEFAULT_DAILY_REFRESH_INTERVAL)?;

        let daily_days = match get("DAILY_DAYS") {
            Some(value) => value
                .parse()
                .ok()
                .filter(|days| (1..=MAX_DAILY_DAYS).contains(days))
                .with_context(|| {
                    format!("DAILY_DAYS `{value}` is not a number from 1 to {MAX_DAILY_DAYS}")
                })?,
            None => DEFAULT_DAILY_DAYS,
        };

        let timezone = match get("TZ") {
            Some(value) => value
                .parse()
                .ok()
                .with_context(|| format!("TZ `{value}` is not a known time zone"))?,
            None => DEFAULT_TIMEZONE,
        };

        Ok(Self {
            daily_page_url: daily_page_url(&page_url),
            page_url,
            webhook_url,
            refresh_interval,
            daily_refresh_interval,
            daily_days,
            timezone,
        })
    }
}

/// Accepts the bare location (`warszawa,cId,36917`), the URL path
/// (`/prognoza-szczegolowa-warszawa,cId,36917`) or the full URL.
fn page_url(location: &str) -> Result<String> {
    if is_http_url(location) {
        return Ok(location.to_owned());
    }
    let location = location.trim_start_matches('/');
    let location = location.strip_prefix(HOURLY_PAGE).unwrap_or(location);
    ensure!(
        !location.is_empty()
            && !location.contains(|c: char| c.is_whitespace() || "/?#".contains(c)),
        "LOCATION `{location}` does not look like `warszawa,cId,36917`"
    );
    Ok(format!("{SITE}{HOURLY_PAGE}{location}"))
}

/// The 45-day page has the same address as the hour-by-hour one, apart from
/// the path prefix.
fn daily_page_url(page_url: &str) -> Option<String> {
    page_url
        .contains(HOURLY_PAGE)
        .then(|| page_url.replacen(HOURLY_PAGE, DAILY_PAGE, 1))
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEBHOOK: &str = "http://homeassistant/api/webhook/abc";

    fn load(vars: &[(&str, &str)], require_webhook: bool) -> Result<Config> {
        Config::load(
            |name| {
                vars.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            },
            require_webhook,
        )
    }

    #[test]
    fn applies_defaults() {
        let config = load(
            &[("LOCATION", "warszawa,cId,36917"), ("WEBHOOK_URL", WEBHOOK)],
            true,
        )
        .unwrap();
        assert_eq!(
            config,
            Config {
                page_url: "https://pogoda.interia.pl/prognoza-szczegolowa-warszawa,cId,36917"
                    .to_owned(),
                daily_page_url: Some(
                    "https://pogoda.interia.pl/prognoza-dlugoterminowa-warszawa,cId,36917"
                        .to_owned()
                ),
                webhook_url: Some(WEBHOOK.to_owned()),
                refresh_interval: Duration::from_hours(1),
                daily_refresh_interval: Duration::from_hours(6),
                daily_days: 14,
                timezone: chrono_tz::Europe::Warsaw,
            }
        );
    }

    #[test]
    fn reads_optional_settings() {
        let config = load(
            &[
                ("LOCATION", "londyn,cId,39613"),
                ("WEBHOOK_URL", WEBHOOK),
                ("REFRESH_INTERVAL", "1h 30m"),
                ("DAILY_REFRESH_INTERVAL", "12h"),
                ("DAILY_DAYS", "45"),
                ("TZ", "Europe/London"),
            ],
            true,
        )
        .unwrap();
        assert_eq!(config.refresh_interval, Duration::from_secs(5400));
        assert_eq!(config.daily_refresh_interval, Duration::from_hours(12));
        assert_eq!(config.daily_days, 45);
        assert_eq!(config.timezone, chrono_tz::Europe::London);
    }

    #[test]
    fn accepts_location_as_path_or_url() {
        let expected =
            "https://pogoda.interia.pl/prognoza-szczegolowa-warszawa-srodmiescie,cId,30704";
        for location in [
            "warszawa-srodmiescie,cId,30704",
            "/prognoza-szczegolowa-warszawa-srodmiescie,cId,30704",
            expected,
        ] {
            assert_eq!(page_url(location).unwrap(), expected);
        }
        assert!(page_url("warszawa srodmiescie srodmiescie").is_err());
        assert!(page_url("a/b").is_err());
    }

    #[test]
    fn derives_the_daily_page_url() {
        assert_eq!(
            daily_page_url(
                "https://pogoda.interia.pl/prognoza-szczegolowa-warszawa-srodmiescie,cId,30704"
            )
            .as_deref(),
            Some(
                "https://pogoda.interia.pl/prognoza-dlugoterminowa-warszawa-srodmiescie,cId,30704"
            )
        );
        assert_eq!(daily_page_url("http://localhost/forecast.html"), None);
    }

    #[test]
    fn reports_missing_and_invalid_settings() {
        let error = |vars: &[(&str, &str)]| load(vars, true).unwrap_err().to_string();
        let location = ("LOCATION", "x,cId,1");
        let webhook = ("WEBHOOK_URL", WEBHOOK);

        assert!(error(&[webhook]).contains("LOCATION is not set"));
        assert!(error(&[("LOCATION", "  "), webhook]).contains("LOCATION is not set"));
        assert!(error(&[location]).contains("WEBHOOK_URL is not set"));
        assert!(error(&[location, ("WEBHOOK_URL", "homeassistant")]).contains("http://"));
        assert!(error(&[location, webhook, ("REFRESH_INTERVAL", "soon")]).contains("soon"));
        assert!(error(&[location, webhook, ("REFRESH_INTERVAL", "5s")]).contains("at least 1m"));
        assert!(
            error(&[location, webhook, ("DAILY_REFRESH_INTERVAL", "5s")])
                .contains("DAILY_REFRESH_INTERVAL must be at least 1m")
        );
        for days in ["0", "46", "many"] {
            assert!(error(&[location, webhook, ("DAILY_DAYS", days)]).contains("from 1 to 45"));
        }
        assert!(error(&[location, webhook, ("TZ", "Mars/Olympus")]).contains("Mars/Olympus"));
    }

    #[test]
    fn webhook_is_optional_for_dry_runs() {
        let config = load(&[("LOCATION", "x,cId,1")], false).unwrap();
        assert_eq!(config.webhook_url, None);
    }
}
