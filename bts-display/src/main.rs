use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use anyhow::Context;
use bts_protocol::{BtsState, DisplayState, ServerMessage};
use eframe::egui::{
    self, Align, Align2, CentralPanel, Color32, FontData, FontDefinitions, FontFamily, FontId,
    Frame, Layout, Margin, RichText, Stroke, Vec2, ViewportBuilder,
};
use futures_util::StreamExt;
use tokio_tungstenite::{connect_async, tungstenite::Message as WebSocketMessage};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_CORE_URL: &str = "ws://127.0.0.1:3100/api/v1/events/ws";

const BACKGROUND: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
const PRIMARY_TEXT: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
const SECONDARY_TEXT: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xa8);
const MUTED_TEXT: Color32 = Color32::from_rgb(0x70, 0x70, 0x70);
const ACCENT: Color32 = Color32::from_rgb(0xff, 0xb0, 0x00);

const PAGE_MARGIN: f32 = 54.0;
const STATUS_MARGIN: f32 = 26.0;

fn main() -> anyhow::Result<()> {
    initialise_logging();

    let core_url = std::env::var("BTS_CORE_URL").unwrap_or_else(|_| DEFAULT_CORE_URL.to_owned());

    info!(%core_url, "starting BTS Display");

    let native_options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("Bansleben Telephone Services")
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([640.0, 360.0])
            .with_decorations(false),
        ..Default::default()
    };

    eframe::run_native(
        "Bansleben Telephone Services",
        native_options,
        Box::new(move |creation_context| {
            configure_fonts_and_style(&creation_context.egui_ctx);

            Ok(Box::new(BtsDisplayApp::new(
                creation_context.egui_ctx.clone(),
                core_url,
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!("display failed: {error}"))
}

struct BtsDisplayApp {
    state: BtsState,
    connection_status: ConnectionStatus,
    messages: Receiver<DisplayMessage>,
}

#[derive(Debug, Clone)]
enum ConnectionStatus {
    Connecting,
    Connected,
    Disconnected(String),
}

#[derive(Debug)]
enum DisplayMessage {
    Connected,
    Disconnected(String),
    State(BtsState),
}

impl BtsDisplayApp {
    fn new(context: egui::Context, core_url: String) -> Self {
        let (sender, receiver) = mpsc::channel();

        spawn_websocket_worker(core_url, sender, context);

        Self {
            state: BtsState::default(),
            connection_status: ConnectionStatus::Connecting,
            messages: receiver,
        }
    }

    fn process_messages(&mut self) {
        while let Ok(message) = self.messages.try_recv() {
            match message {
                DisplayMessage::Connected => {
                    self.connection_status = ConnectionStatus::Connected;
                }
                DisplayMessage::Disconnected(reason) => {
                    self.connection_status = ConnectionStatus::Disconnected(reason);
                }
                DisplayMessage::State(state) => {
                    self.state = state;
                }
            }
        }
    }
}

impl eframe::App for BtsDisplayApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_messages();

        context.set_cursor_icon(egui::CursorIcon::None);
        context.request_repaint_after(Duration::from_millis(250));

        CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(Margin::same(PAGE_MARGIN as i8)),
            )
            .show(context, |ui| {
                ui.set_min_size(ui.available_size());

                match &self.state.display {
                    DisplayState::Clock {
                        time,
                        seconds,
                        date,
                    } => {
                        draw_clock(ui, time, seconds, date);
                    }

                    DisplayState::Weather {
                        location,
                        temperature,
                        condition,
                        details,
                        updated_at,
                    } => {
                        draw_weather(ui, location, temperature, condition, details, updated_at);
                    }

                    DisplayState::Message { title, body } => {
                        draw_message(ui, title, body);
                    }

                    DisplayState::Blank => {}
                }

                draw_connection_indicator(ui, &self.connection_status);
            });
    }
}

fn draw_message(ui: &mut egui::Ui, title: &str, body: &str) {
    draw_service_heading(ui, "BANSLEBEN TELEPHONE SERVICES");

    ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
        vertical_space_fraction(ui, 0.22);

        ui.label(
            RichText::new(title)
                .font(FontId::new(62.0, FontFamily::Proportional))
                .color(PRIMARY_TEXT),
        );

        ui.add_space(28.0);

        ui.set_max_width((ui.available_width() * 0.78).max(420.0));
        ui.label(
            RichText::new(body)
                .font(FontId::new(30.0, FontFamily::Proportional))
                .color(SECONDARY_TEXT)
                .line_height(Some(40.0)),
        );
    });
}

fn draw_clock(ui: &mut egui::Ui, time: &str, seconds: &str, date: &str) {
    draw_service_heading(ui, "BANSLEBEN TELEPHONE SERVICES");

    ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
        vertical_space_fraction(ui, 0.19);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;

            ui.label(
                RichText::new(time)
                    .font(FontId::new(156.0, FontFamily::Proportional))
                    .color(PRIMARY_TEXT),
            );

            ui.add_space(4.0);

            ui.label(
                RichText::new(seconds)
                    .font(FontId::new(46.0, FontFamily::Proportional))
                    .color(ACCENT),
            );
        });

        ui.add_space(18.0);

        ui.label(
            RichText::new(date)
                .font(FontId::new(32.0, FontFamily::Proportional))
                .color(SECONDARY_TEXT),
        );
    });
}

fn draw_weather(
    ui: &mut egui::Ui,
    location: &str,
    temperature: &str,
    condition: &str,
    details: &[String],
    updated_at: &str,
) {
    draw_service_heading(ui, "BANSLEBEN TELEPHONE SERVICES");

    ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
        vertical_space_fraction(ui, 0.12);

        ui.label(
            RichText::new(location)
                .font(FontId::new(48.0, FontFamily::Proportional))
                .color(PRIMARY_TEXT),
        );

        ui.add_space(40.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 42.0;

            ui.label(
                RichText::new(temperature)
                    .font(FontId::new(128.0, FontFamily::Proportional))
                    .color(PRIMARY_TEXT),
            );

            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.add_space(26.0);
                ui.label(
                    RichText::new(condition)
                        .font(FontId::new(38.0, FontFamily::Proportional))
                        .color(SECONDARY_TEXT),
                );
            });
        });

        ui.add_space(40.0);

        for detail in details {
            ui.label(
                RichText::new(detail)
                    .font(FontId::new(28.0, FontFamily::Proportional))
                    .color(SECONDARY_TEXT),
            );
        }

        ui.add_space(40.0);
        ui.label(
            RichText::new(format!("Updated at {updated_at}"))
                .font(FontId::new(22.0, FontFamily::Proportional))
                .color(MUTED_TEXT),
        );
    });
}

fn draw_service_heading(ui: &mut egui::Ui, heading: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(heading)
                .font(FontId::new(22.0, FontFamily::Proportional))
                .color(ACCENT),
        );
    });

    ui.add_space(16.0);
    let width = ui.available_width().min(360.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 2.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, ACCENT);
}

fn draw_connection_indicator(ui: &mut egui::Ui, status: &ConnectionStatus) {
    let status_text = match status {
        ConnectionStatus::Connected => return,
        ConnectionStatus::Connecting => "Connecting to BTS Core".to_owned(),
        ConnectionStatus::Disconnected(reason) => {
            format!("BTS Core is currently unavailable. {reason}")
        }
    };

    let rectangle = ui.max_rect();

    ui.painter().text(
        rectangle.left_bottom() + Vec2::new(STATUS_MARGIN, -STATUS_MARGIN),
        Align2::LEFT_BOTTOM,
        status_text,
        FontId::new(18.0, FontFamily::Proportional),
        MUTED_TEXT,
    );
}

fn vertical_space_fraction(ui: &mut egui::Ui, fraction: f32) {
    ui.add_space(ui.available_height() * fraction);
}

fn configure_fonts_and_style(context: &egui::Context) {
    configure_cabin_font(context);

    let mut style = (*context.style()).clone();

    style.visuals.window_fill = BACKGROUND;
    style.visuals.panel_fill = BACKGROUND;
    style.visuals.extreme_bg_color = BACKGROUND;
    style.visuals.faint_bg_color = BACKGROUND;
    style.visuals.window_stroke = Stroke::NONE;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
    style.visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    style.visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    style.visuals.widgets.active.bg_stroke = Stroke::NONE;

    style.spacing.item_spacing = Vec2::new(12.0, 12.0);

    context.set_style(style);
}

fn configure_cabin_font(context: &egui::Context) {
    let Some((font_path, font_bytes)) = load_cabin_font() else {
        warn!(
            "Cabin could not be found. Install the Cabin font package or set BTS_CABIN_FONT to a Cabin .ttf file"
        );
        return;
    };

    info!(path = %font_path.display(), "loaded Cabin font");

    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("Cabin".to_owned(), FontData::from_owned(font_bytes).into());

    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Cabin".to_owned());

    context.set_fonts(fonts);
}

fn load_cabin_font() -> Option<(PathBuf, Vec<u8>)> {
    let mut candidates = Vec::new();

    if let Ok(path) = std::env::var("BTS_CABIN_FONT") {
        candidates.push(PathBuf::from(path));
    }

    candidates.extend([
        PathBuf::from("/usr/share/fonts/TTF/Cabin-Regular.ttf"),
        PathBuf::from("/usr/share/fonts/cabin/Cabin-Regular.ttf"),
        PathBuf::from("/usr/share/fonts/truetype/cabin/Cabin-Regular.ttf"),
        PathBuf::from("/usr/local/share/fonts/Cabin-Regular.ttf"),
    ]);

    candidates
        .into_iter()
        .find_map(|path| read_font_file(&path).map(|bytes| (path, bytes)))
}

fn read_font_file(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn spawn_websocket_worker(
    core_url: String,
    sender: Sender<DisplayMessage>,
    context: egui::Context,
) {
    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                error!(%error, "failed to create Tokio runtime");
                return;
            }
        };

        runtime.block_on(async move {
            websocket_reconnection_loop(core_url, sender, context).await;
        });
    });
}

async fn websocket_reconnection_loop(
    core_url: String,
    sender: Sender<DisplayMessage>,
    context: egui::Context,
) {
    loop {
        info!(%core_url, "connecting to BTS Core");

        match run_websocket_connection(&core_url, &sender, &context).await {
            Ok(()) => {
                warn!("BTS Core WebSocket closed");
                send_display_message(
                    &sender,
                    &context,
                    DisplayMessage::Disconnected("The connection was closed.".to_owned()),
                );
            }
            Err(error) => {
                warn!(%error, "BTS Core connection failed");
                send_display_message(
                    &sender,
                    &context,
                    DisplayMessage::Disconnected(error.to_string()),
                );
            }
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn run_websocket_connection(
    core_url: &str,
    sender: &Sender<DisplayMessage>,
    context: &egui::Context,
) -> anyhow::Result<()> {
    let (websocket, response) = connect_async(core_url)
        .await
        .with_context(|| format!("Could not connect to {core_url}."))?;

    info!(status = %response.status(), "connected to BTS Core");
    send_display_message(sender, context, DisplayMessage::Connected);

    let (_write, mut read) = websocket.split();

    while let Some(message) = read.next().await {
        match message.context("WebSocket protocol error")? {
            WebSocketMessage::Text(text) => {
                process_server_message(text.as_str(), sender, context)?;
            }
            WebSocketMessage::Close(_) => break,
            WebSocketMessage::Ping(_)
            | WebSocketMessage::Pong(_)
            | WebSocketMessage::Binary(_)
            | WebSocketMessage::Frame(_) => {}
        }
    }

    Ok(())
}

fn process_server_message(
    text: &str,
    sender: &Sender<DisplayMessage>,
    context: &egui::Context,
) -> anyhow::Result<()> {
    let message: ServerMessage =
        serde_json::from_str(text).context("Invalid message from BTS Core")?;

    let state = match message {
        ServerMessage::Snapshot { state } => state,
        ServerMessage::Event { state, .. } => state,
    };

    send_display_message(sender, context, DisplayMessage::State(state));

    Ok(())
}

fn send_display_message(
    sender: &Sender<DisplayMessage>,
    context: &egui::Context,
    message: DisplayMessage,
) {
    if sender.send(message).is_ok() {
        context.request_repaint();
    }
}

fn initialise_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("bts_display=info,wgpu=warn,naga=warn"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .compact()
        .init();
}
