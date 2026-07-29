use anyhow::{Context, Result};
use bts_protocol::{DisplayState, Event, EventKind};
use serde::Deserialize;

use crate::{AddonContext, addons::requested_digit};

const DIGIT: &str = "3";
const LOCATION: &str = "Gdynia";
const LATITUDE: f64 = 54.5189;
const LONGITUDE: f64 = 18.5305;

#[derive(Debug, Deserialize)]
struct ForecastResponse {
    current: CurrentWeather,
}

#[derive(Debug, Deserialize)]
struct CurrentWeather {
    time: String,
    temperature_2m: f32,
    apparent_temperature: f32,
    relative_humidity_2m: u8,
    weather_code: u16,
    wind_speed_10m: f32,
}

pub(crate) async fn handle(context: &AddonContext, event: &Event) -> Result<()> {
    if requested_digit(event) != Some(DIGIT) {
        return Ok(());
    }

    let weather = fetch_current_weather(context).await?;

    context
        .publish(EventKind::DisplaySet {
            display: DisplayState::Weather {
                location: LOCATION.to_owned(),
                temperature: format!("{:.0}°C", weather.temperature_2m),
                condition: describe_weather_code(weather.weather_code).to_owned(),
                details: vec![
                    format!("Feels like {:.0}°C", weather.apparent_temperature),
                    format!("Humidity {}%", weather.relative_humidity_2m),
                    format!("Wind {:.0} km/h", weather.wind_speed_10m),
                ],
                updated_at: weather.time,
            },
        })
        .await
}

async fn fetch_current_weather(context: &AddonContext) -> Result<CurrentWeather> {
    context
        .http
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", LATITUDE.to_string()),
            ("longitude", LONGITUDE.to_string()),
            (
                "current",
                "temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m"
                    .to_owned(),
            ),
            ("timezone", "Europe/Warsaw".to_owned()),
        ])
        .send()
        .await
        .context("failed to contact the weather provider")?
        .error_for_status()
        .context("weather provider returned an error")?
        .json::<ForecastResponse>()
        .await
        .context("failed to decode weather data")
        .map(|response| response.current)
}

fn describe_weather_code(code: u16) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mainly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing drizzle",
        61 | 63 | 65 => "Rain",
        66 | 67 => "Freezing rain",
        71 | 73 | 75 | 77 => "Snow",
        80..=82 => "Rain showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with hail",
        _ => "Unknown conditions",
    }
}
