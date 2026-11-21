mod condition;
mod config;
mod daily;
mod model;
mod parser;

use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use ureq::Agent;

use crate::config::{Config, USAGE};
use crate::model::Report;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Delay before the first retry after a failed refresh; doubles on every
/// further failure, up to the refresh interval.
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(30);

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

    if args.once {
        return refresh(&agent, config, args.dry_run);
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

    let mut retry_delay = INITIAL_RETRY_DELAY;
    loop {
        let wait = match refresh(&agent, config, args.dry_run) {
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

/// Fetches the page, parses it and delivers the result.
fn refresh(agent: &Agent, config: &Config, dry_run: bool) -> Result<()> {
    let html = agent
        .get(&config.page_url)
        .call()
        .with_context(|| format!("fetching {}", config.page_url))?
        .body_mut()
        .read_to_string()
        .context("reading the forecast page")?;
    let updated = Utc::now().with_timezone(&config.timezone);

    let page = parser::parse(&html, config.timezone)
        .with_context(|| format!("parsing {}", config.page_url))?;
    let report = Report {
        location: page.location,
        source: config.page_url.clone(),
        updated,
        current: page.current,
        daily: daily::summarize(&page.hourly),
        hourly: page.hourly,
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
