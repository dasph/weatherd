# weatherd

Scrapes the hour-by-hour and the 45-day forecast from
[pogoda.interia.pl](https://pogoda.interia.pl) and pushes them as JSON to a
Home Assistant webhook, where a template weather entity turns them into a
regular `weather.*` entity with hourly and daily forecasts.

## Things to know

- **Home Assistant needs configuring too.** weatherd only pushes data; the
  `weather.*` entity comes from a webhook-triggered template weather entity
  that you add to `configuration.yaml` (see [Home Assistant](#home-assistant)).
  The YAML there uses the current option names; older Home Assistant releases
  want a `_template` suffix.
- **Hourly and daily forecasts come from two pages.** The hourly forecast and
  the current conditions come from the hour-by-hour page (`prognoza-szczegolowa-…`,
  about five days), the daily forecast from the 45-day page
  (`prognoza-dlugoterminowa-…`). Both are derived from `LOCATION` and sent in
  the same push; Home Assistant shows one or the other depending on the
  forecast type you select.
- **The two pages are fetched on separate schedules.** The hour-by-hour page
  is fetched every `REFRESH_INTERVAL`, the 45-day page every
  `DAILY_REFRESH_INTERVAL`. Every push still carries the daily forecast (the
  last one fetched), because Home Assistant replaces both forecasts on each
  push.
- **Far-out days are rough guesses.** Only the first `DAILY_DAYS` days
  (default 14) of the 45 are pushed.
- **The daily forecast has a fallback.** If the 45-day page cannot be fetched
  or parsed, the previous daily forecast is kept; if there is none, it is
  computed from the hourly forecast (about five days, the first covering only
  the hours that are left of today) until the page works again.
- **Current humidity is a forecast value.** The page shows no observed
  humidity and Home Assistant requires one, so it is taken from the first
  forecast hour.
- **Snow counts as precipitation.** The page gives snow as depth in cm; 1 cm
  of snow is counted as 1 mm of precipitation.
- **The container must be able to reach Home Assistant.** The compose example
  assumes an external network named `homeassistant` and that hostname; adjust
  both. If Home Assistant runs with `network_mode: host`, point `WEBHOOK_URL`
  at the Docker host's address instead.
- **`TZ` matters.** The page shows local times without a zone; they are
  interpreted in `TZ` (default `Europe/Warsaw`).
- **The parser tests need the sample pages.**
  `tests/fixtures/warszawa-srodmiescie.html` (hour-by-hour) and
  `tests/fixtures/warszawa-srodmiescie.45.html` (45-day) are saved copies of
  the forecast pages that the parser tests read. When the site changes its
  markup, save fresh copies there and update the parser and the expected
  values in the tests.

## Configuration

| Variable           | Required | Default         | Description |
| ------------------ | -------- | --------------- | ----------- |
| `LOCATION`         | yes      |                 | Location part of the forecast URL. For `https://pogoda.interia.pl/prognoza-szczegolowa-warszawa,cId,36917` it is `warszawa,cId,36917`. The full URL is accepted too. |
| `WEBHOOK_URL`      | yes      |                 | Home Assistant webhook, e.g. `http://homeassistant/api/webhook/<webhook_id>`. |
| `REFRESH_INTERVAL` | no       | `1h`            | How often to fetch the current conditions and hourly forecast and push, e.g. `30m`, `1h`, `1h 30m`. Minimum `1m`; the site updates every 30 minutes. |
| `DAILY_REFRESH_INTERVAL` | no | `6h`            | How often to fetch the daily (45-day) forecast. It is checked on every refresh, so it is effectively rounded up to a multiple of `REFRESH_INTERVAL`. Minimum `1m`. |
| `DAILY_DAYS`       | no       | `14`            | Number of days of daily forecast to push, `1`-`45`. |
| `TZ`               | no       | `Europe/Warsaw` | Time zone of the times shown on the page. |

Flags, mainly for testing:

- `--once` fetches and pushes a single time, then exits (non-zero on failure).
- `--dry-run` prints the payload to stdout instead of pushing it; `WEBHOOK_URL`
  is then optional.

```sh
LOCATION='warszawa,cId,36917' cargo run -- --once --dry-run
```

A failed fetch or push is logged and retried after 30 s, doubling up to the
refresh interval. Nothing is pushed when the hour-by-hour page cannot be
parsed. A failure of the 45-day page only logs a warning (see
[Things to know](#things-to-know)) and is retried on the next refresh.

## Home Assistant

Add a webhook-triggered template weather entity to `configuration.yaml`. The
webhook id is the only thing protecting the endpoint, so make it long and
random (e.g. `openssl rand -hex 24`).

```yaml
template:
  - triggers:
      - trigger: webhook
        webhook_id: CHANGE-ME
        allowed_methods: [POST]
        local_only: true
    weather:
      - name: Interia
        unique_id: weatherd_interia
        condition: "{{ trigger.json.current.condition }}"
        temperature: "{{ trigger.json.current.temperature }}"
        apparent_temperature: "{{ trigger.json.current.apparent_temperature }}"
        humidity: "{{ trigger.json.current.humidity }}"
        pressure: "{{ trigger.json.current.pressure }}"
        wind_speed: "{{ trigger.json.current.wind_speed }}"
        wind_bearing: "{{ trigger.json.current.wind_bearing }}"
        forecast_hourly: "{{ trigger.json.hourly }}"
        forecast_daily: "{{ trigger.json.daily }}"
        temperature_unit: "°C"
        pressure_unit: hPa
        wind_speed_unit: km/h
        precipitation_unit: mm
        attribution: "pogoda.interia.pl / AccuWeather"
```

The rest of the payload (air quality, sunrise and sunset, ...) is available as
`trigger.json` to any other template entities you add to the same block.

The option names above are the ones in the current Home Assistant
documentation. Older releases use a `_template` suffix for the templated
options (`condition_template`, `forecast_hourly_template`, ...) and
`trigger:` / `platform: webhook` instead of `triggers:` / `trigger: webhook`.

The entity stays `unknown` until the first push. Home Assistant answers `200`
even for an unknown webhook id, so if weatherd logs successful pushes but the
entity does not update, check the id.

## Payload

Metric units throughout: °C, km/h, hPa, mm, %.

```json
{
  "location": { "id": "30704", "name": "Warszawa", "district": "Śródmieście" },
  "source": "https://pogoda.interia.pl/prognoza-szczegolowa-warszawa-srodmiescie,cId,30704",
  "updated": "2026-11-21T23:45:00+01:00",
  "current": {
    "condition": "rainy",
    "description": "Zachmurzenie duże z przelotnymi opadami",
    "temperature": 4.0,
    "apparent_temperature": -3.0,
    "humidity": 94.0,
    "pressure": 1008.0,
    "pressure_trend": "steady",
    "wind_speed": 20.0,
    "wind_bearing": 247.5,
    "wind_direction": "WSW",
    "sunrise": "07:04",
    "sunset": "15:38",
    "moonrise": "14:37",
    "moonset": "03:50",
    "air_quality": {
      "caqi": 28.0,
      "description": "Dobra",
      "pm25_percent": 68.0,
      "pm10_percent": 38.0
    }
  },
  "hourly": [
    {
      "datetime": "2026-11-22T00:00:00+01:00",
      "condition": "cloudy",
      "native_temperature": 3.0,
      "native_apparent_temperature": -2.0,
      "native_precipitation": 0.0,
      "native_wind_speed": 18.0,
      "native_wind_gust_speed": 35.0,
      "wind_bearing": 247.5,
      "humidity": 94.0,
      "cloud_coverage": 75.0
    }
  ],
  "daily": [
    {
      "datetime": "2026-11-22T00:00:00+01:00",
      "condition": "rainy",
      "native_temperature": 3.0,
      "native_templow": 0.0,
      "native_precipitation": 0.3,
      "native_wind_speed": 18.0,
      "native_wind_gust_speed": 52.0,
      "wind_bearing": 270.0,
      "humidity": null,
      "cloud_coverage": 98.0,
      "uv_index": 1.0
    }
  ]
}
```

Notes:

- Values the page does not provide are `null`.
- `hourly` has one entry per hour for about five days, starting with the next
  full hour. Its entries, like the `daily` ones, only use keys Home Assistant
  accepts in a forecast, so the lists can be passed through unchanged.
- `daily` has one entry per day from today on, taken from the 45-day page.
  That page has no humidity, so `humidity` is `null` there.
- When `daily` has to be computed from `hourly` instead, it holds high/low,
  total precipitation, strongest wind and gust, mean humidity and cloud cover,
  and no `uv_index`. The condition is the most frequent one between 07:00 and
  18:00, with precipitation taking over once it covers a quarter of those
  hours.
- `condition` is a Home Assistant weather condition, mapped from the page's
  (AccuWeather) icon; `current.description` keeps the page's Polish wording.
  The icons for ice ("Lód"), heat and cold say nothing about the sky, so for
  those the condition follows the cloud cover (sunny, partly cloudy or
  cloudy).
- `native_precipitation` is rain plus the liquid equivalent of snow, and
  `current.humidity` comes from the first forecast hour (see
  [Things to know](#things-to-know)).
