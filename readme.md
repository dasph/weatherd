# weatherd

Scrapes the hour-by-hour forecast from [pogoda.interia.pl](https://pogoda.interia.pl)
and pushes it as JSON to a Home Assistant webhook, where a template weather
entity turns it into a regular `weather.*` entity with hourly and daily
forecasts.

## Things to know

- **Home Assistant needs configuring too.** weatherd only pushes data; the
  `weather.*` entity comes from a webhook-triggered template weather entity
  that you add to `configuration.yaml` (see [Home Assistant](#home-assistant)).
  The YAML there uses the current option names; older Home Assistant releases
  want a `_template` suffix.
- **The daily forecast is computed, not scraped.** The page only lists hours,
  so each day's high/low, precipitation and condition are aggregated from
  them. The first day covers only the hours that are left of it.
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
- **The parser tests need the sample page.**
  `tests/fixtures/warszawa-srodmiescie.html` is a saved copy of a forecast
  page that the parser tests read. When the site changes its markup, save a
  fresh copy there and update the parser and the expected values in the tests.

## Configuration

| Variable           | Required | Default         | Description |
| ------------------ | -------- | --------------- | ----------- |
| `LOCATION`         | yes      |                 | Location part of the forecast URL. For `https://pogoda.interia.pl/prognoza-szczegolowa-warszawa,cId,36917` it is `warszawa,cId,36917`. The full URL is accepted too. |
| `WEBHOOK_URL`      | yes      |                 | Home Assistant webhook, e.g. `http://homeassistant/api/webhook/<webhook_id>`. |
| `REFRESH_INTERVAL` | no       | `1h`            | How often to fetch and push, e.g. `30m`, `1h`, `1h 30m`. Minimum `1m`; the site updates every 30 minutes. |
| `TZ`               | no       | `Europe/Warsaw` | Time zone of the times shown on the page. |

Flags, mainly for testing:

- `--once` fetches and pushes a single time, then exits (non-zero on failure).
- `--dry-run` prints the payload to stdout instead of pushing it; `WEBHOOK_URL`
  is then optional.

```sh
LOCATION='warszawa,cId,36917' cargo run -- --once --dry-run
```

A failed fetch or push is logged and retried after 30 s, doubling up to the
refresh interval. Nothing is pushed when the page cannot be parsed.

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
      "native_temperature": 4.0,
      "native_templow": 2.0,
      "native_precipitation": 3.4,
      "native_wind_speed": 20.0,
      "native_wind_gust_speed": 44.0,
      "wind_bearing": 270.0,
      "humidity": 88.0,
      "cloud_coverage": 89.0
    }
  ]
}
```

Notes:

- Values the page does not provide are `null`.
- `hourly` has one entry per hour for about five days, starting with the next
  full hour. Its entries, like the `daily` ones, only use keys Home Assistant
  accepts in a forecast, so the lists can be passed through unchanged.
- `daily` is computed from `hourly`: high/low, total precipitation, strongest
  wind and gust, mean humidity and cloud cover. The condition is the most
  frequent one between 07:00 and 18:00, with precipitation taking over once it
  covers a quarter of those hours. The first day only covers the hours that
  are left of it.
- `condition` is a Home Assistant weather condition, mapped from the page's
  (AccuWeather) icon; `current.description` keeps the page's Polish wording.
- `native_precipitation` is rain plus the liquid equivalent of snow, and
  `current.humidity` comes from the first forecast hour (see
  [Things to know](#things-to-know)).
