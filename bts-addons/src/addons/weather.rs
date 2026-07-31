use anyhow::{Context, Result};
use async_trait::async_trait;
use bts_addons::{Addon, AddonContext};
use bts_protocol::{
    ADDON_API_VERSION, ActionId, ActionRegistration, AddonCapability, AddonId, AddonManifest,
    AddonVersion, DisplayLeaseId, DisplayState, Event, EventKind, MenuEntry, ScreenKind,
};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tokio::{
    sync::Mutex,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

pub(crate) const ID: &str = "weather";
pub(crate) const ACTION: &str = "weather.show";
const LOCATION: &str = "Gdynia";

#[derive(Deserialize)]
struct Forecast {
    current: Current,
}
#[derive(Deserialize)]
struct Current {
    time: String,
    temperature_2m: f32,
    apparent_temperature: f32,
    relative_humidity_2m: u8,
    weather_code: u16,
    wind_speed_10m: f32,
}

pub(crate) struct WeatherAddon {
    task: Mutex<Option<JoinHandle<()>>>,
    lease: Mutex<Option<DisplayLeaseId>>,
    http: Client,
}
impl WeatherAddon {
    pub(crate) fn new() -> Self {
        Self {
            task: Mutex::new(None),
            lease: Mutex::new(None),
            http: Client::new(),
        }
    }
}

#[async_trait]
impl Addon for WeatherAddon {
    fn manifest(&self) -> AddonManifest {
        AddonManifest {
            api_version: ADDON_API_VERSION,
            id: AddonId::new(ID),
            name: "Weather Service".into(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![ActionRegistration {
                id: ActionId::new(ACTION),
                description: "Show current weather".into(),
            }],
            menu: vec![MenuEntry {
                digit: '3',
                prompt: "sound:bts/press-3-weather".into(),
                action: ActionId::new(ACTION),
                order: 30,
            }],
            capabilities: vec![AddonCapability::Display, AddonCapability::ExternalHttp],
            screens: vec![ScreenKind::Weather],
        }
    }
    async fn handle_event(&self, context: &AddonContext, event: &Event) -> Result<()> {
        let EventKind::ActionRequested { request } = &event.kind else {
            return Ok(());
        };
        if request.action.as_str() != ACTION {
            return Ok(());
        }
        self.stop(context).await?;
        let lease = context.show(fetch(&self.http).await?, 10).await?;
        *self.lease.lock().await = Some(lease);
        let context = context.clone();
        let http = self.http.clone();
        let task = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(900));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let Ok(screen) = fetch(&http).await else {
                    continue;
                };
                if context.update(lease, screen).await.is_err() {
                    break;
                }
            }
        });
        *self.task.lock().await = Some(task);
        Ok(())
    }
    async fn stop(&self, context: &AddonContext) -> Result<()> {
        if let Some(task) = self.task.lock().await.take() {
            task.abort();
        }
        if let Some(lease) = self.lease.lock().await.take() {
            let _ = context.release(lease).await;
        }
        Ok(())
    }
}

async fn fetch(http: &Client) -> Result<DisplayState> {
    let current = http.get("https://api.open-meteo.com/v1/forecast").query(&[("latitude","54.5189"),("longitude","18.5305"),("current","temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m"),("timezone","Europe/Warsaw")]).send().await.context("failed to contact the weather provider")?.error_for_status()?.json::<Forecast>().await?.current;
    Ok(DisplayState::Weather {
        location: LOCATION.into(),
        temperature: format!("{:.0}°C", current.temperature_2m),
        condition: describe(current.weather_code).into(),
        details: vec![
            format!("Feels like {:.0}°C", current.apparent_temperature),
            format!("Humidity {}%", current.relative_humidity_2m),
            format!("Wind {:.0} km/h", current.wind_speed_10m),
        ],
        updated_at: current.time,
    })
}
fn describe(code: u16) -> &'static str {
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
