//! Extracts weather data from pogoda.interia.pl pages: the "prognoza
//! szczegółowa" (hour-by-hour forecast) page and the "prognoza
//! długoterminowa" (45-day forecast) page.

use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, LocalResult, NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Tz;
use scraper::{ElementRef, Html, Selector};

use crate::condition::{Condition, compass_to_degrees};
use crate::model::{AirQuality, Current, DailyForecast, HourlyForecast, Location};

/// Hourly rain (mm) from which a rainy hour is reported as `pouring`.
const POURING_MM_PER_HOUR: f64 = 4.0;

/// Everything the hour-by-hour page provides.
#[derive(Debug)]
pub struct Page {
    pub location: Location,
    pub current: Current,
    pub hourly: Vec<HourlyForecast>,
}

/// A CSS selector that is compiled once.
macro_rules! selector {
    ($css:literal) => {{
        static SELECTOR: LazyLock<Selector> =
            LazyLock::new(|| Selector::parse($css).expect("valid CSS selector"));
        &*SELECTOR
    }};
}

/// Parses the hour-by-hour page. Times on the page are local to `tz`.
///
/// Fails when the page lacks the current temperature or any hourly entries,
/// which means it is an error page or its markup changed; individual missing
/// details become `None` instead.
pub fn parse(html: &str, tz: Tz) -> Result<Page> {
    let doc = Html::parse_document(html);
    let root = doc.root_element();

    let hourly = parse_hourly(root, tz)?;
    let mut current = parse_current(root)?;
    current.humidity = hourly.first().and_then(|h| h.humidity);

    Ok(Page {
        location: parse_location(root)?,
        current,
        hourly,
    })
}

fn parse_location(root: ElementRef) -> Result<Location> {
    let city = root
        .select(selector!(".weather-currently-city"))
        .next()
        .context("location name not found on the page")?;
    Ok(Location {
        id: city.value().attr("data-city-id").map(str::to_owned),
        name: text(city),
        district: first_text(root, selector!(".weather-currently-city-district")),
    })
}

fn parse_current(root: ElementRef) -> Result<Current> {
    let section = root
        .select(selector!(".weather-currently"))
        .next()
        .context("current conditions not found on the page")?;

    let icon = section.select(selector!(".weather-currently-icon")).next();
    let description = first_text(section, selector!(".weather-currently-icon-description"))
        .or_else(|| {
            icon.and_then(|i| i.value().attr("title"))
                .map(str::to_owned)
        });

    let pressure = section
        .select(selector!(".pressure .weather-currently-details-value"))
        .next();
    let wind_direction = section
        .select(selector!(".weather-currently-wind-icon"))
        .next()
        .and_then(|img| img.value().attr("alt"))
        .map(|alt| alt.trim().to_owned())
        .filter(|alt| !alt.is_empty());

    Ok(Current {
        condition: Condition::resolve(icon.and_then(icon_code), description.as_deref()),
        description,
        temperature: first_number(section, selector!(".weather-currently-temp-strict"))
            .context("current temperature not found on the page")?,
        apparent_temperature: first_number(
            section,
            selector!(".feelTemperature .weather-currently-details-value"),
        ),
        humidity: None,
        pressure: pressure.and_then(|p| number(&text(p))),
        pressure_trend: pressure.and_then(|p| {
            p.value()
                .classes()
                .find(|c| *c != "weather-currently-details-value")
                .map(str::to_owned)
        }),
        wind_speed: first_number(section, selector!(".wind .weather-currently-details-value")),
        wind_bearing: wind_direction.as_deref().and_then(compass_to_degrees),
        wind_direction,
        sunrise: first_text(section, selector!(".weather-currently-info-sunrise")),
        sunset: first_text(section, selector!(".weather-currently-info-sunset")),
        moonrise: first_text(section, selector!(".weather-currently-info-moonrise")),
        moonset: first_text(section, selector!(".weather-currently-info-moonset")),
        air_quality: parse_air_quality(section),
    })
}

fn parse_air_quality(section: ElementRef) -> Option<AirQuality> {
    let caqi = first_number(
        section,
        selector!(".weather-currently-pollution-circle .number"),
    );
    let details = section
        .select(selector!(".weather-currently-middle-pollution-right"))
        .next();
    let air_quality = AirQuality {
        caqi,
        description: details.and_then(|d| first_text(d, selector!(".kind .value"))),
        pm25_percent: details.and_then(|d| first_number(d, selector!(".pm25 .value"))),
        pm10_percent: details.and_then(|d| first_number(d, selector!(".pm10 .value"))),
    };
    let present = air_quality.caqi.is_some()
        || air_quality.description.is_some()
        || air_quality.pm25_percent.is_some()
        || air_quality.pm10_percent.is_some();
    present.then_some(air_quality)
}

fn parse_hourly(root: ElementRef, tz: Tz) -> Result<Vec<HourlyForecast>> {
    let mut hourly: Vec<HourlyForecast> = Vec::new();

    // Each day is a `<div class="weather-forecast 2026-11-22 ...">` holding
    // one `.weather-entry` per hour.
    for day in root.select(selector!("div.weather-forecast")) {
        let Some(date) = day
            .value()
            .classes()
            .find_map(|c| NaiveDate::parse_from_str(c, "%Y-%m-%d").ok())
        else {
            continue;
        };
        for entry in day.select(selector!(".weather-entry")) {
            let previous = hourly.last().map(|h| h.datetime);
            if let Some(forecast) = parse_entry(entry, date, tz, previous) {
                hourly.push(forecast);
            }
        }
    }

    if hourly.is_empty() {
        bail!("no hourly forecast entries found on the page");
    }
    Ok(hourly)
}

/// Returns `None` for entries without a usable time or temperature.
fn parse_entry(
    entry: ElementRef,
    date: NaiveDate,
    tz: Tz,
    previous: Option<DateTime<Tz>>,
) -> Option<HourlyForecast> {
    let clock = |selector| first_text(entry, selector)?.parse::<u32>().ok();
    let hour = clock(selector!(".entry-hour .hour"))?;
    let minute = clock(selector!(".entry-hour .minutes")).unwrap_or(0);
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    let datetime = match tz.from_local_datetime(&date.and_time(time)) {
        LocalResult::Single(dt) => dt,
        // The hour repeated when clocks go back: first occurrence, then second.
        LocalResult::Ambiguous(first, second) => {
            if previous.is_some_and(|p| first <= p) {
                second
            } else {
                first
            }
        }
        // The hour skipped when clocks go forward.
        LocalResult::None => return None,
    };

    let icon = entry.select(selector!(".forecast-icon")).next();
    let phrase = first_text(entry, selector!(".forecast-phrase")).or_else(|| {
        icon.and_then(|i| i.value().attr("title"))
            .map(str::to_owned)
    });

    let mut cloud_coverage = None;
    let mut precipitation = None;
    for value in entry.select(selector!(".entry-precipitation-value")) {
        let Some(amount) = number(&text(value)) else {
            continue;
        };
        if value.value().classes().any(|c| c == "cloud-cover") {
            cloud_coverage = Some(amount);
        } else {
            let unit = first_text(value, selector!(".precipitation-unit")).unwrap_or_default();
            let is_snow = value.value().classes().any(|c| c == "snow");
            *precipitation.get_or_insert(0.0) += liquid_mm(amount, &unit, is_snow);
        }
    }
    let precipitation = precipitation.map(round1);

    let mut condition = Condition::resolve(icon.and_then(icon_code), phrase.as_deref());
    if condition == Some(Condition::Rainy)
        && precipitation.is_some_and(|mm| mm >= POURING_MM_PER_HOUR)
    {
        condition = Some(Condition::Pouring);
    }

    Some(HourlyForecast {
        datetime,
        condition,
        native_temperature: first_number(entry, selector!(".forecast-temp"))?,
        native_apparent_temperature: first_number(entry, selector!(".forecast-feeltemp")),
        native_precipitation: precipitation,
        native_wind_speed: first_number(entry, selector!(".wind-speed .speed-value")),
        native_wind_gust_speed: first_number(entry, selector!(".wind-hit")),
        wind_bearing: first_text(entry, selector!(".wind-direction"))
            .as_deref()
            .and_then(compass_to_degrees),
        humidity: first_number(entry, selector!(".entry-humidity")),
        cloud_coverage,
    })
}

/// Parses the 45-day page into one entry per day.
///
/// The page gives dates as day and month only; `today` anchors the year.
/// Fails when the page has no usable entries.
pub fn parse_long_term(html: &str, tz: Tz, today: NaiveDate) -> Result<Vec<DailyForecast>> {
    let doc = Html::parse_document(html);
    let mut daily = Vec::new();
    let mut previous = None;

    for entry in doc
        .root_element()
        .select(selector!(".weather-forecast-longterm-list-entry"))
    {
        let Some(date) = first_text(entry, selector!(".date"))
            .and_then(|text| resolve_date(&text, previous, today))
        else {
            continue;
        };
        if let Some(forecast) = parse_day(entry, date, tz) {
            daily.push(forecast);
            previous = Some(date);
        }
    }

    if daily.is_empty() {
        bail!("no daily forecast entries found on the page");
    }
    Ok(daily)
}

/// Completes a "22.11" (day.month) date with a year.
///
/// The first entry is the date closest to `today`; every later entry is the
/// next such date after the previous one, which carries the list over New
/// Year.
fn resolve_date(text: &str, previous: Option<NaiveDate>, today: NaiveDate) -> Option<NaiveDate> {
    let (day, month) = text.trim().trim_end_matches('.').split_once('.')?;
    let (day, month): (u32, u32) = (day.parse().ok()?, month.parse().ok()?);
    let in_year = |year| NaiveDate::from_ymd_opt(year, month, day);
    match previous {
        Some(previous) => (previous.year()..=previous.year() + 1)
            .filter_map(in_year)
            .find(|date| *date > previous),
        None => (today.year() - 1..=today.year() + 1)
            .filter_map(in_year)
            .min_by_key(|date| (*date - today).num_days().abs()),
    }
}

/// Returns `None` for entries without usable temperatures.
fn parse_day(entry: ElementRef, date: NaiveDate, tz: Tz) -> Option<DailyForecast> {
    let datetime = tz
        .from_local_datetime(&date.and_time(NaiveTime::MIN))
        .earliest()?;

    let icon = entry
        .select(selector!(
            ".weather-forecast-longterm-list-entry-forecast-icon"
        ))
        .next();
    let phrase = first_text(
        entry,
        selector!(".weather-forecast-longterm-list-entry-forecast-phrase"),
    )
    .or_else(|| {
        icon.and_then(|i| i.value().attr("title"))
            .map(str::to_owned)
    });

    // "Deszcz:" (rain, mm) and, on some days, "Śnieg:" (snow, cm).
    let mut precipitation = None;
    for item in entry.select(selector!(".precipitation-types .item")) {
        let Some(value) = item
            .select(selector!(
                ".weather-forecast-longterm-list-entry-precipitation-value"
            ))
            .next()
        else {
            continue;
        };
        let Some(amount) = number(&text(value)) else {
            continue;
        };
        let unit = first_text(value, selector!(".precipitation-unit")).unwrap_or_default();
        let is_snow = first_text(
            item,
            selector!(".weather-forecast-longterm-list-entry-precipitation-type"),
        )
        .is_some_and(|label| label.to_lowercase().contains("śnieg"));
        *precipitation.get_or_insert(0.0) += liquid_mm(amount, &unit, is_snow);
    }

    Some(DailyForecast {
        datetime,
        condition: Condition::resolve(icon.and_then(icon_code), phrase.as_deref()),
        native_temperature: first_number(
            entry,
            selector!(".weather-forecast-longterm-list-entry-forecast-temp"),
        )?,
        native_templow: first_number(
            entry,
            selector!(".weather-forecast-longterm-list-entry-forecast-lowtemp"),
        )?,
        native_precipitation: precipitation.map(round1),
        native_wind_speed: first_number(
            entry,
            selector!(".weather-forecast-longterm-list-entry-wind-value"),
        ),
        native_wind_gust_speed: first_number(
            entry,
            selector!(".weather-forecast-longterm-list-entry-wind-hit"),
        ),
        wind_bearing: first_text(
            entry,
            selector!(".weather-forecast-longterm-list-entry-wind-direction"),
        )
        .as_deref()
        .and_then(compass_to_degrees),
        humidity: None,
        cloud_coverage: first_number(
            entry,
            selector!(".weather-forecast-longterm-list-entry-cloudy-cloudy-value"),
        ),
        // Shown as "1/10".
        uv_index: first_number(
            entry,
            selector!(".weather-forecast-longterm-list-entry-sun-uv-value"),
        ),
    })
}

/// Converts a precipitation amount to mm of water.
///
/// Rain is given in mm, snow as depth in cm. Snow is converted with the
/// usual 10:1 rule (1 cm of snow ≈ 1 mm of water).
fn liquid_mm(amount: f64, unit: &str, is_snow: bool) -> f64 {
    match (unit.trim(), is_snow) {
        ("cm", false) => amount * 10.0,
        _ => amount,
    }
}

fn icon_code(element: ElementRef) -> Option<u32> {
    element
        .value()
        .classes()
        .find_map(|c| c.strip_prefix("ico-")?.parse().ok())
}

/// The element's text with whitespace collapsed.
fn text(element: ElementRef) -> String {
    let raw: String = element.text().collect();
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn first_text(scope: ElementRef, selector: &Selector) -> Option<String> {
    scope
        .select(selector)
        .next()
        .map(text)
        .filter(|t| !t.is_empty())
}

fn first_number(scope: ElementRef, selector: &Selector) -> Option<f64> {
    number(&first_text(scope, selector)?)
}

/// Extracts the first number from text such as "-5°C", "Odczuwalna -4°C",
/// "Max 33 km/h" or "0,5 mm".
fn number(text: &str) -> Option<f64> {
    let text = text.replace('−', "-").replace(',', ".");
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let end = text[start..]
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .map_or(text.len(), |offset| start + offset);
    let value: f64 = text[start..end].trim_end_matches('.').parse().ok()?;
    Some(if text[..start].ends_with('-') {
        -value
    } else {
        value
    })
}

pub fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use chrono::SecondsFormat;

    use super::*;

    const TZ: Tz = chrono_tz::Europe::Warsaw;

    /// The page saved from
    /// https://pogoda.interia.pl/prognoza-szczegolowa-warszawa-srodmiescie,cId,30704
    fn sample() -> Page {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/warszawa-srodmiescie.html"
        );
        let html = std::fs::read_to_string(path).expect("sample page is readable");
        parse(&html, TZ).expect("sample page parses")
    }

    /// The page saved from
    /// https://pogoda.interia.pl/prognoza-dlugoterminowa-warszawa-srodmiescie,cId,30704
    /// on 2026-11-22.
    fn long_term_sample() -> Vec<DailyForecast> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/warszawa-srodmiescie.45.html"
        );
        let html = std::fs::read_to_string(path).expect("sample page is readable");
        let today = NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        parse_long_term(&html, TZ, today).expect("sample page parses")
    }

    fn iso(dt: DateTime<Tz>) -> String {
        dt.to_rfc3339_opts(SecondsFormat::Secs, false)
    }

    #[test]
    fn parses_numbers() {
        assert_eq!(number("1°C"), Some(1.0));
        assert_eq!(number("-5°C"), Some(-5.0));
        assert_eq!(number("−5°C"), Some(-5.0));
        assert_eq!(number("Odczuwalna -4°C "), Some(-4.0));
        assert_eq!(number("Max 33 km/h"), Some(33.0));
        assert_eq!(number("1013 hPa"), Some(1013.0));
        assert_eq!(number("0.1mm"), Some(0.1));
        assert_eq!(number("0,5 mm"), Some(0.5));
        assert_eq!(number("56% normy"), Some(56.0));
        assert_eq!(number("brak danych"), None);
    }

    #[test]
    fn parses_location() {
        let page = sample();
        assert_eq!(page.location.name, "Warszawa");
        assert_eq!(page.location.district.as_deref(), Some("Śródmieście"));
        assert_eq!(page.location.id.as_deref(), Some("30704"));
    }

    #[test]
    fn parses_current_conditions() {
        let current = sample().current;
        assert_eq!(current.condition, Some(Condition::Rainy));
        assert_eq!(
            current.description.as_deref(),
            Some("Zachmurzenie duże z przelotnymi opadami")
        );
        assert_eq!(current.temperature, 4.0);
        assert_eq!(current.apparent_temperature, Some(-3.0));
        assert_eq!(current.humidity, Some(94.0));
        assert_eq!(current.pressure, Some(1008.0));
        assert_eq!(current.pressure_trend.as_deref(), Some("steady"));
        assert_eq!(current.wind_speed, Some(20.0));
        assert_eq!(current.wind_direction.as_deref(), Some("WSW"));
        assert_eq!(current.wind_bearing, Some(247.5));
        assert_eq!(current.sunrise.as_deref(), Some("07:04"));
        assert_eq!(current.sunset.as_deref(), Some("15:38"));
        assert_eq!(current.moonrise.as_deref(), Some("14:37"));
        assert_eq!(current.moonset.as_deref(), Some("03:50"));

        let air = current.air_quality.expect("air quality is present");
        assert_eq!(air.caqi, Some(28.0));
        assert_eq!(air.description.as_deref(), Some("Dobra"));
        assert_eq!(air.pm25_percent, Some(68.0));
        assert_eq!(air.pm10_percent, Some(38.0));
    }

    #[test]
    fn parses_every_hour() {
        let hourly = sample().hourly;
        assert_eq!(hourly.len(), 120);
        assert_eq!(iso(hourly[0].datetime), "2026-11-22T00:00:00+01:00");
        assert_eq!(iso(hourly[119].datetime), "2026-11-26T23:00:00+01:00");
        for pair in hourly.windows(2) {
            assert_eq!(
                pair[1].datetime - pair[0].datetime,
                chrono::Duration::hours(1),
                "gap after {}",
                pair[0].datetime
            );
        }
        for hour in &hourly {
            assert!(
                hour.condition.is_some(),
                "no condition at {}",
                hour.datetime
            );
            assert!(hour.native_apparent_temperature.is_some());
            assert!(hour.native_precipitation.is_some());
            assert!(hour.native_wind_speed.is_some());
            assert!(hour.native_wind_gust_speed.is_some());
            assert!(hour.wind_bearing.is_some());
            assert!(hour.humidity.is_some());
            assert!(hour.cloud_coverage.is_some());
        }
    }

    #[test]
    fn parses_hourly_details() {
        let hourly = sample().hourly;

        let first = &hourly[0];
        assert_eq!(first.condition, Some(Condition::Cloudy));
        assert_eq!(first.native_temperature, 3.0);
        assert_eq!(first.native_apparent_temperature, Some(-2.0));
        assert_eq!(first.native_precipitation, Some(0.0));
        assert_eq!(first.native_wind_speed, Some(18.0));
        assert_eq!(first.native_wind_gust_speed, Some(35.0));
        assert_eq!(first.wind_bearing, Some(247.5));
        assert_eq!(first.humidity, Some(94.0));
        assert_eq!(first.cloud_coverage, Some(75.0));

        // 2026-11-22 02:00, "Deszcz".
        let rainy = &hourly[2];
        assert_eq!(iso(rainy.datetime), "2026-11-22T02:00:00+01:00");
        assert_eq!(rainy.condition, Some(Condition::Rainy));
        assert_eq!(rainy.native_temperature, 3.0);
        assert_eq!(rainy.native_precipitation, Some(0.5));
    }

    #[test]
    fn counts_snow_as_precipitation() {
        let hourly = sample().hourly;
        let count = |condition| {
            hourly
                .iter()
                .filter(|h| h.condition == Some(condition))
                .count()
        };
        assert_eq!(count(Condition::Snowy), 5);
        assert_eq!(count(Condition::SnowyRainy), 3);

        // 2026-11-24 17:00, "Śnieg": no rain, 1.8 cm of snow.
        let snow = &hourly[65];
        assert_eq!(iso(snow.datetime), "2026-11-24T17:00:00+01:00");
        assert_eq!(snow.condition, Some(Condition::Snowy));
        assert_eq!(snow.native_precipitation, Some(1.8));

        // 2026-11-24 02:00, "Deszcz i śnieg": 0.4 mm of rain, 0.1 cm of snow.
        let sleet = &hourly[50];
        assert_eq!(iso(sleet.datetime), "2026-11-24T02:00:00+01:00");
        assert_eq!(sleet.condition, Some(Condition::SnowyRainy));
        assert_eq!(sleet.native_precipitation, Some(0.5));
    }

    #[test]
    fn converts_precipitation_units() {
        assert_eq!(liquid_mm(0.5, "mm", false), 0.5);
        assert_eq!(liquid_mm(2.0, "cm", true), 2.0);
        assert_eq!(liquid_mm(0.2, "cm", false), 2.0);
    }

    #[test]
    fn rejects_pages_without_a_forecast() {
        let html = "<html><body><h1>404</h1></body></html>";
        let error = parse(html, TZ).unwrap_err();
        assert!(error.to_string().contains("no hourly forecast"), "{error}");

        let today = NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        let error = parse_long_term(html, TZ, today).unwrap_err();
        assert!(error.to_string().contains("no daily forecast"), "{error}");
    }

    #[test]
    fn parses_every_day_of_the_long_term_page() {
        let daily = long_term_sample();
        assert_eq!(daily.len(), 45);
        assert_eq!(iso(daily[0].datetime), "2026-11-22T00:00:00+01:00");
        // The dates carry no year; the list runs over New Year.
        assert_eq!(iso(daily[40].datetime), "2027-01-01T00:00:00+01:00");
        assert_eq!(iso(daily[44].datetime), "2027-01-05T00:00:00+01:00");
        for pair in daily.windows(2) {
            assert_eq!(
                pair[1].datetime.date_naive() - pair[0].datetime.date_naive(),
                chrono::Duration::days(1),
                "gap after {}",
                pair[0].datetime
            );
        }
        for day in &daily {
            assert!(day.condition.is_some(), "no condition on {}", day.datetime);
            assert!(day.native_temperature >= day.native_templow);
            assert!(day.native_precipitation.is_some());
            assert!(day.native_wind_speed.is_some());
            assert!(day.native_wind_gust_speed.is_some());
            assert!(day.wind_bearing.is_some());
            assert!(day.cloud_coverage.is_some());
            assert!(day.uv_index.is_some());
            assert_eq!(day.humidity, None);
        }
    }

    #[test]
    fn parses_long_term_details() {
        let daily = long_term_sample();

        // 22.11, "Częściowo słonecznie z przelotnymi opadami".
        let first = &daily[0];
        assert_eq!(first.condition, Some(Condition::Rainy));
        assert_eq!(first.native_temperature, 3.0);
        assert_eq!(first.native_templow, 0.0);
        assert_eq!(first.native_precipitation, Some(0.3));
        assert_eq!(first.native_wind_speed, Some(18.0));
        assert_eq!(first.native_wind_gust_speed, Some(52.0));
        assert_eq!(first.wind_bearing, Some(270.0));
        assert_eq!(first.cloud_coverage, Some(98.0));
        assert_eq!(first.uv_index, Some(1.0));

        // 26.11: a negative low and an easterly wind.
        let cold = &daily[4];
        assert_eq!(cold.condition, Some(Condition::Cloudy));
        assert_eq!(cold.native_temperature, 0.0);
        assert_eq!(cold.native_templow, -4.0);
        assert_eq!(cold.wind_bearing, Some(67.5));

        // 30.11: no rain, 6.32 cm of snow.
        assert_eq!(daily[8].native_precipitation, Some(6.3));
        // 01.12, "Lekkie opady śniegu": no rain, 1.24 cm of snow.
        assert_eq!(daily[9].condition, Some(Condition::Snowy));
        assert_eq!(daily[9].native_precipitation, Some(1.2));
    }

    #[test]
    fn resolves_dates_without_a_year() {
        let date = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();

        // The first entry is the date closest to today...
        let first = |text, today| resolve_date(text, None, today);
        assert_eq!(first("22.11", date(2026, 11, 22)), Some(date(2026, 11, 22)));
        assert_eq!(first("01.01", date(2026, 12, 31)), Some(date(2027, 1, 1)));
        assert_eq!(first("31.12", date(2027, 1, 1)), Some(date(2026, 12, 31)));

        // ...and later entries follow the previous one.
        let next = |text, previous| resolve_date(text, Some(previous), date(2026, 11, 22));
        assert_eq!(next("23.11", date(2026, 11, 22)), Some(date(2026, 11, 23)));
        assert_eq!(next("01.01", date(2026, 12, 31)), Some(date(2027, 1, 1)));
        assert_eq!(next("29.02", date(2028, 2, 28)), Some(date(2028, 2, 29)));

        assert_eq!(first("Niedziela", date(2026, 11, 22)), None);
        assert_eq!(first("31.02", date(2026, 11, 22)), None);
    }

    #[test]
    fn resolves_the_repeated_hour_when_clocks_go_back() {
        let entry = |hour: u32| {
            format!(
                r#"<div class="weather-entry">
                     <div class="entry-hour"><span class="hour">{hour}</span><span class="minutes">00</span></div>
                     <span class="forecast-icon ico-7" title="Pochmurno"></span>
                     <span class="forecast-temp">8°C</span>
                   </div>"#
            )
        };
        let html = format!(
            r#"<h3 class="weather-currently-city" data-city-id="1">X</h3>
               <section class="weather-currently"><div class="weather-currently-temp-strict">8°C</div></section>
               <div class="weather-forecast 2026-10-25 selected">{}{}{}{}</div>"#,
            entry(1),
            entry(2),
            entry(2),
            entry(3)
        );
        let times: Vec<_> = parse(&html, TZ)
            .unwrap()
            .hourly
            .iter()
            .map(|h| iso(h.datetime))
            .collect();
        assert_eq!(
            times,
            [
                "2026-10-25T01:00:00+02:00",
                "2026-10-25T02:00:00+02:00",
                "2026-10-25T02:00:00+01:00",
                "2026-10-25T03:00:00+01:00",
            ]
        );
    }
}
