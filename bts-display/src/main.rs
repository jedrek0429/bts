use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, ensure};
use bts_protocol::{
    DisplayState, PresentationRejection, PresentationRejectionCode, RegistrationRejection,
    RegistrationRejectionReason, TerminalCapabilities, TerminalCapability, TerminalId,
    TerminalImplementationId, TerminalName, core::CORE_TERMINALS_WEBSOCKET_PATH,
};
use bts_terminal::{
    ConnectionState, RuntimeDiagnostics, TerminalConfiguration, TerminalEvent, TerminalHandle,
    TerminalRuntime,
};
use eframe::egui::{
    self, Align, Align2, CentralPanel, Color32, FontData, FontDefinitions, FontFamily, FontId,
    Frame, Layout, Margin, RichText, Stroke, Vec2, ViewportBuilder, ViewportCommand,
};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const BACKGROUND: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
const PRIMARY_TEXT: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
const SECONDARY_TEXT: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xa8);
const MUTED_TEXT: Color32 = Color32::from_rgb(0x70, 0x70, 0x70);
const ACCENT: Color32 = Color32::from_rgb(0xff, 0xb0, 0x00);

const PAGE_MARGIN: f32 = 54.0;
const STATUS_MARGIN: f32 = 26.0;

const DISPLAY_LOG_TARGET: &str = "bts_display";
const DISPLAY_IMPLEMENTATION: &str = "bts-display";
const REPAINT_INTERVAL: Duration = Duration::from_millis(100);
const CABIN_FONT_PATHS: [&str; 5] = [
    "/usr/share/fonts/TTF/impallari/Cabin-Regular.ttf",
    "/usr/share/fonts/TTF/Cabin-Regular.ttf",
    "/usr/share/fonts/cabin/Cabin-Regular.ttf",
    "/usr/share/fonts/truetype/cabin/Cabin-Regular.ttf",
    "/usr/local/share/fonts/Cabin-Regular.ttf",
];

fn main() -> anyhow::Result<()> {
    initialise_logging();
    let configuration = DisplayConfiguration::from_environment();

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
            hide_cursor(&creation_context.egui_ctx);
            configure_fonts_and_style(&creation_context.egui_ctx);

            Ok(Box::new(BtsDisplayApp::new(
                configuration,
                collect_runtime_diagnostics(&creation_context.egui_ctx),
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!("display failed: {error}"))
}

#[derive(Debug)]
struct DisplayConfiguration {
    core_websocket_url: String,
    terminal_id: TerminalId,
    suggested_name: TerminalName,
}

impl DisplayConfiguration {
    fn from_environment() -> anyhow::Result<Self> {
        Self::from_values(|name| std::env::var(name).ok())
    }

    fn from_values(mut value: impl FnMut(&str) -> Option<String>) -> anyhow::Result<Self> {
        let core_websocket_url = required_configuration(&mut value, "BTS_CORE_WS_URL")?;
        validate_terminal_websocket_url(&core_websocket_url)?;
        let terminal_id = required_configuration(&mut value, "BTS_TERMINAL_ID")?
            .parse()
            .context("BTS_TERMINAL_ID is invalid")?;
        let suggested_name =
            TerminalName::new(required_configuration(&mut value, "BTS_TERMINAL_NAME")?)
                .context("BTS_TERMINAL_NAME is invalid")?;

        Ok(Self {
            core_websocket_url,
            terminal_id,
            suggested_name,
        })
    }

    fn into_terminal_configuration(
        self,
        diagnostics: RuntimeDiagnostics,
    ) -> anyhow::Result<TerminalConfiguration> {
        let implementation = TerminalImplementationId::new(DISPLAY_IMPLEMENTATION)
            .expect("the display implementation identifier is valid");
        let version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .context("the bts-display package version is invalid")?;
        let terminal_id = self.terminal_id.clone();
        let configuration = TerminalConfiguration::new(
            self.core_websocket_url,
            self.terminal_id,
            self.suggested_name,
            implementation,
            version,
            display_capabilities(),
        )?
        .with_runtime_diagnostics(diagnostics);

        info!(
            target: DISPLAY_LOG_TARGET,
            %terminal_id,
            endpoint = %configuration.core_websocket_url(),
            "starting BTS Display terminal"
        );
        Ok(configuration)
    }
}

fn required_configuration(
    value: &mut impl FnMut(&str) -> Option<String>,
    name: &'static str,
) -> anyhow::Result<String> {
    let configured = value(name).with_context(|| format!("{name} is not configured"))?;
    ensure!(!configured.is_empty(), "{name} is not configured");
    Ok(configured)
}

fn validate_terminal_websocket_url(value: &str) -> anyhow::Result<()> {
    ensure!(
        value.starts_with("ws://") || value.starts_with("wss://"),
        "BTS_CORE_WS_URL must use ws:// or wss://"
    );
    ensure!(
        !value.chars().any(char::is_whitespace),
        "BTS_CORE_WS_URL must not contain whitespace"
    );
    let path = value.split(['?', '#']).next().unwrap_or(value);
    ensure!(
        path.ends_with(CORE_TERMINALS_WEBSOCKET_PATH),
        "BTS_CORE_WS_URL must identify the published {CORE_TERMINALS_WEBSOCKET_PATH} endpoint"
    );
    Ok(())
}

fn display_capabilities() -> TerminalCapabilities {
    TerminalCapabilities::new([TerminalCapability::new(TerminalCapability::RENDER_TEXT)
        .expect("the render_text capability is valid")])
}

fn collect_runtime_diagnostics(context: &egui::Context) -> RuntimeDiagnostics {
    let mut values = vec![
        ("platform".to_owned(), std::env::consts::OS.to_owned()),
        ("architecture".to_owned(), std::env::consts::ARCH.to_owned()),
        ("renderer".to_owned(), "glow".to_owned()),
    ];
    if let Some(resolution) = context.input(|input| {
        let viewport = input.viewport();
        let size = viewport.monitor_size?;
        let pixels_per_point = viewport.native_pixels_per_point.unwrap_or(1.0);
        Some(format!(
            "{}x{}",
            (size.x * pixels_per_point).round() as u32,
            (size.y * pixels_per_point).round() as u32
        ))
    }) {
        values.push(("display.resolution".to_owned(), resolution));
    }
    RuntimeDiagnostics::new(values).expect("built-in display diagnostics are valid")
}

struct BtsDisplayApp {
    terminal_id: Option<TerminalId>,
    capabilities: TerminalCapabilities,
    presentation: Option<DisplayState>,
    connection_status: ConnectionStatus,
    terminal: Option<TerminalHandle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConnectionStatus {
    ConfigurationError(String),
    Connecting,
    Connected,
    Disconnected(String),
    Retrying { attempt: u32, delay: Duration },
    RegistrationRejected(String),
}

impl BtsDisplayApp {
    fn new(
        configuration: anyhow::Result<DisplayConfiguration>,
        diagnostics: RuntimeDiagnostics,
    ) -> Self {
        match configuration.and_then(|configuration| {
            let terminal_id = configuration.terminal_id.clone();
            let runtime = configuration.into_terminal_configuration(diagnostics)?;
            let handle = TerminalRuntime::spawn(runtime)?;
            Ok((terminal_id, handle))
        }) {
            Ok((terminal_id, terminal)) => Self {
                terminal_id: Some(terminal_id),
                capabilities: display_capabilities(),
                presentation: None,
                connection_status: ConnectionStatus::Connecting,
                terminal: Some(terminal),
            },
            Err(error) => {
                error!(target: DISPLAY_LOG_TARGET, %error, "display configuration is invalid");
                Self {
                    terminal_id: None,
                    capabilities: display_capabilities(),
                    presentation: None,
                    connection_status: ConnectionStatus::ConfigurationError(error.to_string()),
                    terminal: None,
                }
            }
        }
    }

    fn process_terminal_events(&mut self) {
        loop {
            let event = self
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.try_next_event().ok());
            let Some(event) = event else {
                break;
            };
            self.process_terminal_event(event);
        }
    }

    fn process_terminal_event(&mut self, event: TerminalEvent) {
        match event {
            TerminalEvent::ConnectionStateChanged(state) => self.set_connection_state(state),
            TerminalEvent::RegistrationRejected(rejection) => {
                self.connection_status = ConnectionStatus::RegistrationRejected(
                    registration_rejection_message(&rejection),
                );
            }
            TerminalEvent::PresentationReceived(work) => self.apply_presentation(work),
            TerminalEvent::ProtocolError { detail } => {
                warn!(target: DISPLAY_LOG_TARGET, %detail, "terminal protocol error");
            }
            TerminalEvent::DispatchIgnored {
                presentation_id,
                reason,
            } => {
                warn!(target: DISPLAY_LOG_TARGET, ?presentation_id, ?reason, "ignored presentation dispatch");
            }
            TerminalEvent::CommandIgnored {
                presentation_id,
                reason,
            } => {
                warn!(target: DISPLAY_LOG_TARGET, ?presentation_id, ?reason, "ignored presentation result");
            }
            TerminalEvent::PresentationInvalidated { completion, reason } => {
                info!(target: DISPLAY_LOG_TARGET, presentation_id = ?completion.presentation_id(), ?reason, "presentation work invalidated");
            }
        }
    }

    fn set_connection_state(&mut self, state: ConnectionState) {
        if matches!(
            self.connection_status,
            ConnectionStatus::RegistrationRejected(_)
        ) && !matches!(state, ConnectionState::Registered { .. })
        {
            return;
        }
        self.connection_status = match state {
            ConnectionState::Connecting => ConnectionStatus::Connecting,
            ConnectionState::Registered { .. } => ConnectionStatus::Connected,
            ConnectionState::Disconnected { reason } => ConnectionStatus::Disconnected(reason),
            ConnectionState::Retrying { attempt, delay } => {
                ConnectionStatus::Retrying { attempt, delay }
            }
        };
    }

    fn apply_presentation(&mut self, work: bts_terminal::PresentationWork) {
        if !work.is_applicable() {
            return;
        }
        let Some(terminal_id) = &self.terminal_id else {
            return;
        };
        let presentation = work.presentation();
        let completion = work.completion().clone();
        match presentation_for_display(presentation, terminal_id, &self.capabilities) {
            PresentationApplication::ForeignRecipient => {
                warn!(
                    target: DISPLAY_LOG_TARGET,
                    presentation_id = ?presentation.request.id,
                    %terminal_id,
                    "display defence ignored a presentation for another terminal"
                );
            }
            PresentationApplication::Rejected(rejection) => {
                if let Some(terminal) = &self.terminal
                    && let Err(error) = terminal.reject_presentation(completion, rejection)
                {
                    warn!(target: DISPLAY_LOG_TARGET, %error, "could not report presentation rejection");
                }
            }
            PresentationApplication::Accepted(display) => {
                self.presentation = Some(display);
                if let Some(terminal) = &self.terminal
                    && let Err(error) = terminal.accept_presentation(completion)
                {
                    warn!(target: DISPLAY_LOG_TARGET, %error, "could not report presentation acceptance");
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PresentationApplication {
    Accepted(DisplayState),
    Rejected(PresentationRejection),
    ForeignRecipient,
}

fn presentation_for_display(
    presentation: &bts_protocol::PresentationDispatch,
    terminal_id: &TerminalId,
    capabilities: &TerminalCapabilities,
) -> PresentationApplication {
    if !presentation.resolved_target.terminals.contains(terminal_id) {
        return PresentationApplication::ForeignRecipient;
    }
    if !capabilities.supports_all(&presentation.request.required_capabilities) {
        return PresentationApplication::Rejected(unsupported_capabilities_rejection());
    }
    PresentationApplication::Accepted(presentation.request.display.clone())
}

impl eframe::App for BtsDisplayApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_terminal_events();

        hide_cursor(context);
        context.request_repaint_after(REPAINT_INTERVAL);

        CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(Margin::same(PAGE_MARGIN as i8)),
            )
            .show(context, |ui| {
                ui.set_min_size(ui.available_size());

                if status_blocks_presentation(&self.connection_status) {
                    draw_status_screen(ui, &self.connection_status);
                } else if let Some(presentation) = &self.presentation {
                    draw_presentation(ui, presentation);
                    draw_connection_indicator(ui, &self.connection_status);
                } else {
                    draw_status_screen(ui, &self.connection_status);
                }
            });
    }
}

fn draw_presentation(ui: &mut egui::Ui, presentation: &DisplayState) {
    match presentation {
        DisplayState::Clock {
            time,
            seconds,
            date,
        } => draw_clock(ui, time, seconds, date),
        DisplayState::Weather {
            location,
            temperature,
            condition,
            details,
            updated_at,
        } => draw_weather(ui, location, temperature, condition, details, updated_at),
        DisplayState::Message { title, body } => draw_message(ui, title, body),
        DisplayState::Blank => {}
    }
}

fn status_blocks_presentation(status: &ConnectionStatus) -> bool {
    matches!(
        status,
        ConnectionStatus::ConfigurationError(_) | ConnectionStatus::RegistrationRejected(_)
    )
}

fn draw_status_screen(ui: &mut egui::Ui, status: &ConnectionStatus) {
    let (heading, detail) = match status {
        ConnectionStatus::ConfigurationError(detail) => (
            "Display configuration required",
            format!("{detail}. Run bts-install configure display, then restart the service."),
        ),
        ConnectionStatus::Connecting => (
            "Connecting to BTS Core",
            "The display will become available after terminal registration.".to_owned(),
        ),
        ConnectionStatus::Connected => (
            "Display connected",
            "Waiting for a presentation.".to_owned(),
        ),
        ConnectionStatus::Disconnected(reason) => (
            "BTS Core is unavailable",
            format!("{reason}. The display will reconnect automatically."),
        ),
        ConnectionStatus::Retrying { attempt, delay } => (
            "BTS Core is unavailable",
            format!(
                "Reconnect attempt {attempt} will begin in {} seconds.",
                delay.as_secs()
            ),
        ),
        ConnectionStatus::RegistrationRejected(detail) => {
            ("Terminal registration rejected", detail.clone())
        }
    };

    draw_service_heading(ui, "BANSLEBEN TELEPHONE SERVICES");
    ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
        vertical_space_fraction(ui, 0.22);
        ui.label(
            RichText::new(heading)
                .font(FontId::new(54.0, FontFamily::Proportional))
                .color(PRIMARY_TEXT),
        );
        ui.add_space(28.0);
        ui.set_max_width((ui.available_width() * 0.8).max(420.0));
        ui.label(
            RichText::new(detail)
                .font(FontId::new(28.0, FontFamily::Proportional))
                .color(SECONDARY_TEXT)
                .line_height(Some(38.0)),
        );
    });
}

fn hide_cursor(context: &egui::Context) {
    // `set_cursor_icon(CursorIcon::None)` is ignored by egui-winit until it has
    // observed a pointer position. The viewport command reaches winit's window
    // visibility API directly, including on the first frame and without input.
    context.send_viewport_cmd(ViewportCommand::CursorVisible(false));
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
        ConnectionStatus::Retrying { attempt, delay } => format!(
            "BTS Core is currently unavailable. Reconnect attempt {attempt} in {} seconds.",
            delay.as_secs()
        ),
        ConnectionStatus::ConfigurationError(_) | ConnectionStatus::RegistrationRejected(_) => {
            return;
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
            target: DISPLAY_LOG_TARGET,
            "Cabin could not be found; using the built-in fallback font. Install the Cabin font package or set BTS_CABIN_FONT to a Cabin .ttf file"
        );
        return;
    };

    info!(target: DISPLAY_LOG_TARGET, path = %font_path.display(), "loaded Cabin font");

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
    load_cabin_font_from_root(
        std::env::var_os("BTS_CABIN_FONT").map(PathBuf::from),
        Path::new("/"),
    )
}

fn load_cabin_font_from_root(
    explicit_override: Option<PathBuf>,
    root: &Path,
) -> Option<(PathBuf, Vec<u8>)> {
    let mut candidates = explicit_override.into_iter().chain(
        CABIN_FONT_PATHS
            .iter()
            .map(|path| rooted_path(root, Path::new(path))),
    );

    candidates.find_map(|path| read_font_file(&path).map(|bytes| (path, bytes)))
}

fn rooted_path(root: &Path, path: &Path) -> PathBuf {
    root.join(path.strip_prefix("/").unwrap_or(path))
}

fn read_font_file(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn unsupported_capabilities_rejection() -> PresentationRejection {
    PresentationRejection {
        code: PresentationRejectionCode::new(PresentationRejectionCode::UNSUPPORTED_CAPABILITIES)
            .expect("the unsupported-capabilities rejection code is valid"),
        detail: Some("The display cannot reliably render every required capability.".to_owned()),
    }
}

fn registration_rejection_message(rejection: &RegistrationRejection) -> String {
    match &rejection.reason {
        RegistrationRejectionReason::DuplicateTerminalId
        | RegistrationRejectionReason::IdentityAlreadyConnected => format!(
            "Another active display is already using terminal ID {}. Configure a distinct BTS_TERMINAL_ID for this installation.",
            rejection
                .terminal_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "this display".to_owned())
        ),
        RegistrationRejectionReason::UnsupportedProtocolVersion {
            received,
            supported,
        } => format!(
            "Terminal protocol {}.{} is not supported by Core (Core supports {}.{}). Upgrade the incompatible component.",
            received.major, received.minor, supported.major, supported.minor
        ),
        RegistrationRejectionReason::InvalidRegistration { detail } => {
            format!("Core rejected this terminal's configuration: {detail}")
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use bts_protocol::{
        EventKind, NewEvent, PresentationDeliveryOutcome, PresentationDispatch, PresentationId,
        PresentationRequest, ResolvedTarget, ScreenKind, TargetScope, TerminalTarget,
        addons::v1::{API_VERSION, AddonCapability, AddonId, AddonManifest, AddonVersion},
        core::{CORE_EVENTS_PATH, CORE_TERMINALS_WEBSOCKET_PATH},
    };
    use tokio::sync::oneshot;

    const PACKAGED_CABIN_PATH: &str = "/usr/share/fonts/TTF/impallari/Cabin-Regular.ttf";
    const LEGACY_CABIN_PATH: &str = "/usr/share/fonts/TTF/Cabin-Regular.ttf";

    #[test]
    fn packaged_cabin_path_is_selected_by_default() {
        let root = tempfile::tempdir().unwrap();
        let packaged = write_test_font(root.path(), PACKAGED_CABIN_PATH, b"packaged cabin");
        write_test_font(root.path(), LEGACY_CABIN_PATH, b"legacy cabin");

        let loaded = load_cabin_font_from_root(None, root.path()).unwrap();

        assert_eq!(loaded, (packaged, b"packaged cabin".to_vec()));
    }

    #[test]
    fn explicit_override_takes_precedence_over_packaged_font() {
        let root = tempfile::tempdir().unwrap();
        write_test_font(root.path(), PACKAGED_CABIN_PATH, b"packaged cabin");
        let explicit = write_test_font(root.path(), "/custom/Cabin-Regular.ttf", b"custom cabin");

        let loaded = load_cabin_font_from_root(Some(explicit.clone()), root.path()).unwrap();

        assert_eq!(loaded, (explicit, b"custom cabin".to_vec()));
    }

    #[test]
    fn missing_cabin_font_uses_fallback() {
        let root = tempfile::tempdir().unwrap();

        assert_eq!(load_cabin_font_from_root(None, root.path()), None);
    }

    #[test]
    fn display_configuration_requires_stable_identity_and_terminal_endpoint() {
        let values = BTreeMap::from([
            (
                "BTS_CORE_WS_URL".to_owned(),
                "ws://core:3100/api/v1/terminals/ws".to_owned(),
            ),
            ("BTS_TERMINAL_ID".to_owned(), "bedroom-display".to_owned()),
            ("BTS_TERMINAL_NAME".to_owned(), "Bedroom".to_owned()),
        ]);
        let configuration =
            DisplayConfiguration::from_values(|name| values.get(name).cloned()).unwrap();
        assert_eq!(configuration.terminal_id.as_str(), "bedroom-display");
        assert_eq!(configuration.suggested_name.as_str(), "Bedroom");

        let mut stale_endpoint = values.clone();
        stale_endpoint.insert(
            "BTS_CORE_WS_URL".to_owned(),
            "ws://core:3100/api/v1/events/ws".to_owned(),
        );
        assert!(
            DisplayConfiguration::from_values(|name| stale_endpoint.get(name).cloned()).is_err()
        );

        let mut missing_identity = values;
        missing_identity.remove("BTS_TERMINAL_ID");
        assert!(
            DisplayConfiguration::from_values(|name| missing_identity.get(name).cloned()).is_err()
        );
    }

    #[test]
    fn graphical_capabilities_are_functional_and_diagnostics_are_separate() {
        let capabilities = display_capabilities();
        assert_eq!(
            capabilities
                .iter()
                .map(TerminalCapability::as_str)
                .collect::<Vec<_>>(),
            vec![TerminalCapability::RENDER_TEXT]
        );
        let diagnostics = collect_runtime_diagnostics(&egui::Context::default());
        let diagnostics = diagnostics.iter().collect::<BTreeMap<_, _>>();
        assert_eq!(diagnostics.get("renderer"), Some(&"glow"));
        assert!(!diagnostics.contains_key(TerminalCapability::RENDER_TEXT));
    }

    #[test]
    fn presentation_mapping_accepts_supported_local_work_and_rejects_missing_capabilities() {
        let terminal_id = TerminalId::new("bedroom-display").unwrap();
        let display = DisplayState::Message {
            title: "Hello".to_owned(),
            body: "Bedroom".to_owned(),
        };
        let accepted = dispatch_for(
            terminal_id.clone(),
            display.clone(),
            TerminalCapabilities::default(),
        );
        assert_eq!(
            presentation_for_display(&accepted, &terminal_id, &display_capabilities()),
            PresentationApplication::Accepted(display)
        );

        let image_capability = TerminalCapability::new(TerminalCapability::RENDER_IMAGES).unwrap();
        let rejected = dispatch_for(
            terminal_id.clone(),
            DisplayState::Blank,
            TerminalCapabilities::new([image_capability]),
        );
        let PresentationApplication::Rejected(rejection) =
            presentation_for_display(&rejected, &terminal_id, &display_capabilities())
        else {
            panic!("missing capabilities should reject local rendering")
        };
        assert_eq!(
            rejection.code.as_str(),
            PresentationRejectionCode::UNSUPPORTED_CAPABILITIES
        );
    }

    #[test]
    fn presentation_mapping_ignores_foreign_recipients_as_defence_in_depth() {
        let bedroom = TerminalId::new("bedroom-display").unwrap();
        let dining = TerminalId::new("dining-display").unwrap();
        let presentation =
            dispatch_for(dining, DisplayState::Blank, TerminalCapabilities::default());
        assert_eq!(
            presentation_for_display(&presentation, &bedroom, &display_capabilities()),
            PresentationApplication::ForeignRecipient
        );
    }

    #[test]
    fn transient_disconnect_preserves_content_while_rejection_blocks_it() {
        assert!(!status_blocks_presentation(
            &ConnectionStatus::Disconnected("network unavailable".to_owned())
        ));
        assert!(!status_blocks_presentation(&ConnectionStatus::Retrying {
            attempt: 2,
            delay: Duration::from_secs(2),
        }));
        assert!(status_blocks_presentation(
            &ConnectionStatus::RegistrationRejected("duplicate terminal ID".to_owned())
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_display_adapters_register_and_apply_independent_presentations() {
        let core = RunningCore::start().await;
        core.register_addon().await;
        let alpha_id = TerminalId::new("bedroom-display").unwrap();
        let bravo_id = TerminalId::new("dining-display").unwrap();
        let mut alpha = display_app(&core.terminal_url, "bedroom-display", "Bedroom");
        let mut bravo = display_app(&core.terminal_url, "dining-display", "Dining Room");
        wait_until_registered(&mut alpha).await;
        wait_until_registered(&mut bravo).await;
        assert_eq!(core.services.terminals.definitions().len(), 2);

        let alpha_display = DisplayState::Message {
            title: "Bedroom".to_owned(),
            body: "Alpha presentation".to_owned(),
        };
        let bravo_display = DisplayState::Message {
            title: "Dining Room".to_owned(),
            body: "Bravo presentation".to_owned(),
        };
        let alpha_presentation = PresentationId::new();
        let bravo_presentation = PresentationId::new();
        core.request(alpha_presentation, &alpha_id, alpha_display.clone())
            .await;
        core.request(bravo_presentation, &bravo_id, bravo_display.clone())
            .await;
        wait_until_presentation(&mut alpha, &alpha_display).await;
        wait_until_presentation(&mut bravo, &bravo_display).await;
        assert_eq!(alpha.presentation, Some(alpha_display));
        assert_eq!(bravo.presentation, Some(bravo_display));
        wait_until_accepted(&core, alpha_presentation, &alpha_id).await;
        wait_until_accepted(&core, bravo_presentation, &bravo_id).await;

        let mut duplicate = display_app(&core.terminal_url, "bedroom-display", "Duplicate");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                duplicate.process_terminal_events();
                if matches!(
                    duplicate.connection_status,
                    ConnectionStatus::RegistrationRejected(_)
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        drop(duplicate);
        drop(alpha);
        drop(bravo);
        core.stop().await;
    }

    struct RunningCore {
        _directory: tempfile::TempDir,
        http_url: String,
        terminal_url: String,
        services: bts_core::server::CoreServices,
        shutdown: oneshot::Sender<()>,
        task: tokio::task::JoinHandle<anyhow::Result<()>>,
    }

    impl RunningCore {
        async fn start() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let server = bts_core::server::CoreServer::new(bts_core::server::CoreConfiguration {
                terminal_state_path: directory.path().join("terminals.json"),
                presence_timeout: Duration::from_secs(60),
                acknowledgement_timeout: Duration::from_secs(30),
                presence_expiry_interval: Duration::from_secs(3600),
                acknowledgement_expiry_interval: Duration::from_secs(3600),
            })
            .unwrap();
            let services = server.services();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (ready_sender, ready_receiver) = oneshot::channel();
            let (shutdown_sender, shutdown_receiver) = oneshot::channel();
            let task = tokio::spawn(server.serve(listener, Some(ready_sender), async move {
                let _ = shutdown_receiver.await;
            }));
            let address = ready_receiver.await.unwrap();
            Self {
                _directory: directory,
                http_url: format!("http://{address}"),
                terminal_url: format!("ws://{address}{CORE_TERMINALS_WEBSOCKET_PATH}"),
                services,
                shutdown: shutdown_sender,
                task,
            }
        }

        async fn register_addon(&self) {
            self.post(EventKind::AddonRegistered {
                manifest: AddonManifest {
                    api_version: API_VERSION,
                    id: AddonId::new("display-test"),
                    name: "Display test".to_owned(),
                    version: AddonVersion::new(1, 0, 0),
                    actions: Vec::new(),
                    menu: Vec::new(),
                    capabilities: vec![AddonCapability::Display],
                    screens: vec![ScreenKind::Message],
                },
            })
            .await;
        }

        async fn request(
            &self,
            id: PresentationId,
            terminal_id: &TerminalId,
            display: DisplayState,
        ) {
            self.post(EventKind::PresentationRequested {
                request: PresentationRequest {
                    id,
                    target: TerminalTarget::Terminal {
                        id: terminal_id.clone(),
                        scope: TargetScope::Online,
                    },
                    required_capabilities: display_capabilities(),
                    display,
                },
            })
            .await;
        }

        async fn post(&self, kind: EventKind) {
            reqwest::Client::new()
                .post(format!("{}{}", self.http_url, CORE_EVENTS_PATH))
                .json(&NewEvent {
                    source: "display-test".to_owned(),
                    kind,
                })
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
        }

        async fn stop(self) {
            let _ = self.shutdown.send(());
            self.task.await.unwrap().unwrap();
        }
    }

    fn display_app(url: &str, id: &str, name: &str) -> BtsDisplayApp {
        BtsDisplayApp::new(
            Ok(DisplayConfiguration {
                core_websocket_url: url.to_owned(),
                terminal_id: TerminalId::new(id).unwrap(),
                suggested_name: TerminalName::new(name).unwrap(),
            }),
            RuntimeDiagnostics::new([
                ("platform".to_owned(), "test".to_owned()),
                ("renderer".to_owned(), "headless-test".to_owned()),
            ])
            .unwrap(),
        )
    }

    async fn wait_until_registered(app: &mut BtsDisplayApp) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                app.process_terminal_events();
                if app.connection_status == ConnectionStatus::Connected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    async fn wait_until_presentation(app: &mut BtsDisplayApp, expected: &DisplayState) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                app.process_terminal_events();
                if app.presentation.as_ref() == Some(expected) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    async fn wait_until_accepted(
        core: &RunningCore,
        presentation_id: PresentationId,
        terminal_id: &TerminalId,
    ) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if core
                    .services
                    .presentations
                    .delivery_result(presentation_id)
                    .and_then(|result| result.outcomes.get(terminal_id).cloned())
                    .is_some_and(|outcome| matches!(outcome, PresentationDeliveryOutcome::Accepted))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    fn dispatch_for(
        terminal_id: TerminalId,
        display: DisplayState,
        required_capabilities: TerminalCapabilities,
    ) -> PresentationDispatch {
        let target = TerminalTarget::Terminal {
            id: terminal_id.clone(),
            scope: TargetScope::Online,
        };
        PresentationDispatch::new(
            PresentationRequest {
                id: PresentationId::new(),
                target: target.clone(),
                required_capabilities,
                display,
            },
            ResolvedTarget::new(target, [terminal_id]).unwrap(),
        )
        .unwrap()
    }

    fn write_test_font(root: &Path, path: &str, bytes: &[u8]) -> PathBuf {
        let path = rooted_path(root, Path::new(path));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn cursor_is_hidden_before_first_frame_without_pointer_input() {
        let context = egui::Context::default();

        hide_cursor(&context);
        let first_frame = context.run(egui::RawInput::default(), |_| {});
        assert_cursor_hidden(&first_frame);
    }

    #[test]
    fn cursor_remains_hidden_during_frames_without_pointer_input() {
        let context = egui::Context::default();

        for _ in 0..2 {
            let frame = context.run(egui::RawInput::default(), hide_cursor);
            assert_cursor_hidden(&frame);
        }
    }

    fn assert_cursor_hidden(output: &egui::FullOutput) {
        let root_viewport = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .expect("root viewport output should exist");

        assert!(
            root_viewport
                .commands
                .contains(&ViewportCommand::CursorVisible(false)),
            "each frame must explicitly hide the native cursor"
        );
    }
}
