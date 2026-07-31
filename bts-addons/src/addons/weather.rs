use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use bts_addons::{
    ADDON_API_VERSION, ActionKind, Addon, AddonCapability, AddonContext, AddonId, AddonManifest,
    AddonVersion,
};
use bts_protocol::{Action, DisplayState, Event, EventKind};
use reqwest::Client;
use serde::Deserialize;
use tokio::{
    sync::Mutex,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};
use tracing::warn;

const LOCATION: &str = "Gdynia";
const LATITUDE: f64 = 54.5189;
const LONGITUDE: f64 = 18.5305;
const UPDATE_INTERVAL: Duration = Duration::from_secs(15 * 60);
const ADDON_ID: &str = "weather";

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

pub(crate) struct WeatherAddon {
    update_task: Mutex<Option<JoinHandle<()>>>,
    http: Client,
}

impl WeatherAddon {
    pub(crate) fn new() -> Self {
        Self {
            update_task: Mutex::new(None),
            http: Client::new(),
        }
    }

    async fn show(&self, context: &AddonContext) -> Result<()> {
        self.stop_update_task().await;
        publish_weather(context, &self.http).await?;

        let context = context.clone();
        let http = self.http.clone();
        let task = tokio::spawn(async move {
            let mut ticker = interval(UPDATE_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

            ticker.tick().await;

            loop {
                ticker.tick().await;

                match weather_is_active(&context).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        warn!(%error, "failed to check active display for weather update");
                        continue;
                    }
                }

                if let Err(error) = publish_weather(&context, &http).await {
                    warn!(%error, "failed to refresh weather");
                }
            }
        });

        *self.update_task.lock().await = Some(task);
        Ok(())
    }

    async fn stop_update_task(&self) {
        if let Some(task) = self.update_task.lock().await.take() {
            task.abort();
        }
    }
}

#[async_trait]
impl Addon for WeatherAddon {
    fn manifest(&self) -> AddonManifest {
        AddonManifest {
            api_version: ADDON_API_VERSION,
            id: AddonId::new(ADDON_ID),
            name: "Weather Service".to_owned(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![ActionKind::Weather],
            capabilities: vec![
                AddonCapability::PublishEvents,
                AddonCapability::ReadState,
                AddonCapability::ExternalHttp,
            ],
        }
    }

    async fn handle_event(&self, context: &AddonContext, event: &Event) -> Result<()> {
        if matches!(
            &event.kind,
            EventKind::ActionRequested {
                action: Action::Weather
            }
        ) {
            self.show(context).await?;
        }

        Ok(())
    }

    async fn stop(&self, _context: &AddonContext) -> Result<()> {
        self.stop_update_task().await;
        Ok(())
    }
}

async fn publish_weather(context: &AddonContext, http: &Client) -> Result<()> {
    let weather = fetch_current_weather(http).await?;

    context
        .publish(
            &AddonId::new(ADDON_ID),
            EventKind::DisplaySet {
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
            },
        )
        .await
}

async fn weather_is_active(context: &AddonContext) -> Result<bool> {
    Ok(matches!(
        context.state().await?.display,
        DisplayState::Weather { .. }
    ))
}

async fn fetch_current_weather(http: &Client) -> Result<CurrentWeather> {
    http
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
