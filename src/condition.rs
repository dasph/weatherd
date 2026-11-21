//! Mapping from the page's weather icons to Home Assistant weather conditions.

use serde::Serialize;

/// A Home Assistant weather condition.
///
/// Variants are ordered by how much they matter to someone reading a
/// forecast: when two conditions are equally frequent in a day, the later
/// variant wins (see `daily`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Condition {
    #[serde(rename = "clear-night")]
    ClearNight,
    #[serde(rename = "sunny")]
    Sunny,
    #[serde(rename = "partlycloudy")]
    PartlyCloudy,
    #[serde(rename = "cloudy")]
    Cloudy,
    #[serde(rename = "windy")]
    Windy,
    #[serde(rename = "fog")]
    Fog,
    #[serde(rename = "rainy")]
    Rainy,
    #[serde(rename = "snowy")]
    Snowy,
    #[serde(rename = "snowy-rainy")]
    SnowyRainy,
    #[serde(rename = "pouring")]
    Pouring,
    #[serde(rename = "lightning-rainy")]
    LightningRainy,
    #[serde(rename = "hail")]
    Hail,
    #[serde(rename = "exceptional")]
    Exceptional,
}

impl Condition {
    /// Resolves a condition from the icon code, falling back to the Polish
    /// description when the code is missing or unknown.
    pub fn resolve(icon: Option<u32>, phrase: Option<&str>) -> Option<Self> {
        icon.and_then(Self::from_icon)
            .or_else(|| phrase.and_then(Self::from_phrase))
    }

    /// Interia's forecast is powered by AccuWeather and reuses its icon
    /// numbering (`ico-N`): 1-32 are day/any-time icons, 33-44 night ones.
    pub fn from_icon(code: u32) -> Option<Self> {
        Some(match code {
            1 | 2 | 5 => Self::Sunny,
            3 | 4 => Self::PartlyCloudy,
            6..=8 => Self::Cloudy,
            11 => Self::Fog,
            12..=14 | 18 | 26 => Self::Rainy,
            15..=17 => Self::LightningRainy,
            19..=23 => Self::Snowy,
            24 | 30 | 31 => Self::Exceptional,
            25 => Self::Hail,
            29 => Self::SnowyRainy,
            32 => Self::Windy,
            33 | 34 | 37 => Self::ClearNight,
            35 | 36 => Self::PartlyCloudy,
            38 => Self::Cloudy,
            39 | 40 => Self::Rainy,
            41 | 42 => Self::LightningRainy,
            43 | 44 => Self::Snowy,
            _ => return None,
        })
    }

    /// Best-effort keyword match on the Polish description, e.g.
    /// "Przelotne opady" or "Lekkie opady śniegu".
    pub fn from_phrase(phrase: &str) -> Option<Self> {
        let p = phrase.to_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|n| p.contains(n));

        let snow = has(&["śnieg", "śnież"]);
        let rain = has(&["deszcz", "mżaw"]);
        Some(if has(&["burz"]) {
            Self::LightningRainy
        } else if snow && rain {
            Self::SnowyRainy
        } else if snow {
            Self::Snowy
        } else if has(&["grad"]) {
            Self::Hail
        } else if rain || has(&["opad"]) {
            Self::Rainy
        } else if has(&["mgł", "mgl"]) {
            Self::Fog
        } else if has(&["pochmurn", "zachmurzenie duże", "zachmurzenie całkowite"]) {
            Self::Cloudy
        } else if has(&["zachmurz", "chmur"]) {
            Self::PartlyCloudy
        } else if has(&["wiatr", "wietrz"]) {
            Self::Windy
        } else if has(&["słonecz", "pogodn", "bezchmurn"]) {
            Self::Sunny
        } else {
            return None;
        })
    }

    pub fn is_precipitation(self) -> bool {
        matches!(
            self,
            Self::Rainy
                | Self::Snowy
                | Self::SnowyRainy
                | Self::Pouring
                | Self::LightningRainy
                | Self::Hail
        )
    }
}

/// Converts a 16-point compass direction ("WSW") to degrees.
pub fn compass_to_degrees(direction: &str) -> Option<f64> {
    const POINTS: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW",
        "NW", "NNW",
    ];
    let direction = direction.trim();
    POINTS
        .iter()
        .position(|p| p.eq_ignore_ascii_case(direction))
        .map(|i| i as f64 * 22.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_icons_seen_on_the_page() {
        assert_eq!(Condition::from_icon(7), Some(Condition::Cloudy));
        assert_eq!(Condition::from_icon(12), Some(Condition::Rainy));
        assert_eq!(Condition::from_icon(22), Some(Condition::Snowy));
        assert_eq!(Condition::from_icon(35), Some(Condition::PartlyCloudy));
        assert_eq!(Condition::from_icon(33), Some(Condition::ClearNight));
        assert_eq!(Condition::from_icon(99), None);
    }

    #[test]
    fn falls_back_to_the_description() {
        let resolve = |phrase| Condition::resolve(Some(99), Some(phrase));
        assert_eq!(resolve("Lekkie opady śniegu"), Some(Condition::Snowy));
        assert_eq!(resolve("Deszcz ze śniegiem"), Some(Condition::SnowyRainy));
        assert_eq!(resolve("Przelotne opady"), Some(Condition::Rainy));
        assert_eq!(resolve("Zachmurzenie duże"), Some(Condition::Cloudy));
        assert_eq!(
            resolve("Przejściowe zachmurzenie"),
            Some(Condition::PartlyCloudy)
        );
        assert_eq!(resolve("Burze"), Some(Condition::LightningRainy));
        assert_eq!(resolve("???"), None);
        // A known icon wins over the text.
        assert_eq!(
            Condition::resolve(Some(7), Some("Burze")),
            Some(Condition::Cloudy)
        );
    }

    #[test]
    fn serializes_to_home_assistant_names() {
        let json = |c| serde_json::to_string(&c).unwrap();
        assert_eq!(json(Condition::ClearNight), "\"clear-night\"");
        assert_eq!(json(Condition::PartlyCloudy), "\"partlycloudy\"");
        assert_eq!(json(Condition::SnowyRainy), "\"snowy-rainy\"");
    }

    #[test]
    fn converts_compass_points() {
        assert_eq!(compass_to_degrees("N"), Some(0.0));
        assert_eq!(compass_to_degrees("WSW"), Some(247.5));
        assert_eq!(compass_to_degrees(" w "), Some(270.0));
        assert_eq!(compass_to_degrees("X"), None);
    }
}
