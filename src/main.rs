mod condition;
mod config;
mod daily;
mod model;
mod parser;

use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use ureq::Agent;

use crate::config::{Config, USAGE};
use crate::model::{DailyForecast, HourlyForecast, Report};

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Delay before the first retry after a failed refresh; doubles on every
/// further failure, up to the refresh interval.
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(30);

/// The daily forecast from the long-term page. It is fetched less often than
/// the hourly page, but every push has to carry a daily forecast, so the last
/// one is kept.
struct DailyCache {
    fetched: Instant,
    days: Vec<DailyForecast>,
}

#[derive(Default)]
struct Args {
    once: bool,
    dry_run: bool,
}

fn main() -> ExitCode {
    let mut args = Args::default();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--once" => args.once = true,
            "--dry-run" => args.dry_run = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument `{other}`\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    let config = match Config::from_env(!args.dry_run) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("configuration error: {error:#}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match run(&config, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: &Config, args: &Args) -> Result<()> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .user_agent(concat!("weatherd/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();

    let mut daily_cache = None;
    if args.once {
        return refresh(&agent, config, args.dry_run, &mut daily_cache);
    }

    // A container's PID 1 ignores SIGTERM unless it installs a handler, so
    // without this `docker stop` would hang until the kill timeout.
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = shutdown_tx.send(());
    })
    .context("installing the signal handler")?;

    eprintln!(
        "watching {} every {} (time zone {})",
        config.page_url,
        humantime::format_duration(config.refresh_interval),
        config.timezone
    );
    match &config.daily_page_url {
        Some(url) => eprintln!(
            "watching {url} every {}",
            humantime::format_duration(config.daily_refresh_interval)
        ),
        None => eprintln!(
            "no long-term page for this LOCATION; deriving the daily forecast from the hourly one"
        ),
    }

    let mut retry_delay = INITIAL_RETRY_DELAY;
    loop {
        let wait = match refresh(&agent, config, args.dry_run, &mut daily_cache) {
            Ok(()) => {
                retry_delay = INITIAL_RETRY_DELAY;
                config.refresh_interval
            }
            Err(error) => {
                let wait = retry_delay.min(config.refresh_interval);
                eprintln!(
                    "error: {error:#}; retrying in {}",
                    humantime::format_duration(wait)
                );
                retry_delay = retry_delay.saturating_mul(2);
                wait
            }
        };
        match shutdown_rx.recv_timeout(wait) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                eprintln!("shutting down");
                return Ok(());
            }
        }
    }
}

/// Fetches and parses the forecast, then delivers the result.
fn refresh(
    agent: &Agent,
    config: &Config,
    dry_run: bool,
    daily_cache: &mut Option<DailyCache>,
) -> Result<()> {
    let html = fetch(agent, &config.page_url)?;
    let updated = Utc::now().with_timezone(&config.timezone);
    let page = parser::parse(&html, config.timezone)
        .with_context(|| format!("parsing {}", config.page_url))?;

    let daily = daily_forecast(
        agent,
        config,
        daily_cache,
        updated.date_naive(),
        &page.hourly,
    );
    let report = Report {
        location: page.location,
        source: config.page_url.clone(),
        updated,
        current: page.current,
        hourly: page.hourly,
        daily,
    };

    let summary = format!(
        "{}: {}°C, {} hourly and {} daily forecast entries",
        report.location.name,
        report.current.temperature,
        report.hourly.len(),
        report.daily.len()
    );
    match (&config.webhook_url, dry_run) {
        (Some(webhook_url), false) => {
            // The webhook URL is a secret, so keep it out of the logs.
            agent
                .post(webhook_url)
                .send_json(&report)
                .context("posting to the webhook")?;
            eprintln!("pushed {summary}");
        }
        _ => {
            println!("{}", serde_json::to_string_pretty(&report)?);
            eprintln!("parsed {summary} (dry run, not pushed)");
        }
    }
    Ok(())
}

/// The daily forecast to send: the long-term page's, refetched once it is
/// older than the daily refresh interval.
///
/// A failure here never fails the refresh. The previous long-term forecast
/// is kept while it still covers today; otherwise the daily forecast is
/// derived from the hourly one.
fn daily_forecast(
    agent: &Agent,
    config: &Config,
    cache: &mut Option<DailyCache>,
    today: NaiveDate,
    hourly: &[HourlyForecast],
) -> Vec<DailyForecast> {
    if let Some(url) = &config.daily_page_url {
        let due = cache
            .as_ref()
            .is_none_or(|cache| cache.fetched.elapsed() >= config.daily_refresh_interval);
        if due {
            let fetched = fetch(agent, url).and_then(|html| {
                parser::parse_long_term(&html, config.timezone, today)
                    .with_context(|| format!("parsing {url}"))
            });
            match fetched {
                Ok(days) => {
                    eprintln!("fetched {} days of daily forecast", days.len());
                    *cache = Some(DailyCache {
                        fetched: Instant::now(),
                        days,
                    });
                }
                Err(error) => eprintln!("warning: daily forecast not updated: {error:#}"),
            }
        }
    }

    let days = cache
        .as_ref()
        .map(|cache| daily::upcoming(&cache.days, today, config.daily_days))
        .unwrap_or_default();
    if days.is_empty() {
        let mut days = daily::summarize(hourly);
        days.truncate(config.daily_days);
        days
    } else {
        days
    }
}

fn fetch(agent: &Agent, url: &str) -> Result<String> {
    agent
        .get(url)
        .call()
        .with_context(|| format!("fetching {url}"))?
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading {url}"))
}
