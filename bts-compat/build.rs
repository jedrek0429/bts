use std::{env, fs, path::PathBuf};

use serde::Deserialize;

#[derive(Deserialize)]
struct Compatibility {
    core_api: u16,
    addon_api: u16,
    release_manifest_schema: u32,
    component_bundle_format: u32,
    installer_state_schema: u32,
    installer_output_schema: u32,
    addons: Addons,
}

#[derive(Deserialize)]
struct Addons {
    clock: String,
    message: String,
    weather: String,
}

fn addon_version(value: &str) -> [u16; 3] {
    let values: Vec<_> = value
        .split('.')
        .map(|part| part.parse::<u16>().expect("addon versions must be numeric"))
        .collect();
    values
        .try_into()
        .expect("addon versions must contain major.minor.patch")
}

fn main() {
    let source =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../compatibility.json");
    println!("cargo:rerun-if-changed={}", source.display());
    let compatibility: Compatibility =
        serde_json::from_slice(&fs::read(source).expect("compatibility.json must be readable"))
            .expect("compatibility.json must be valid");
    let core_prefix = format!("/api/v{}", compatibility.core_api);
    let constants = format!(
        r#"pub const CORE_API_VERSION: u16 = {core_api};
pub const CORE_API_DISCOVERY_PATH: &str = "/api";
pub const CORE_ADMIN_BASE_PATH: &str = "{core_prefix}/admin";
pub const CORE_ADMIN_STATUS_PATH: &str = "{core_prefix}/admin/status";
pub const CORE_ADMIN_STATE_PATH: &str = "{core_prefix}/admin/state";
pub const CORE_ADMIN_TERMINALS_PATH: &str = "{core_prefix}/admin/terminals";
pub const CORE_ADMIN_TERMINAL_PATH: &str = "{core_prefix}/admin/terminals/{{terminal}}";
pub const CORE_ADMIN_TERMINAL_NAME_PATH: &str = "{core_prefix}/admin/terminals/{{terminal}}/name";
pub const CORE_ADMIN_TERMINAL_DESCRIPTION_PATH: &str = "{core_prefix}/admin/terminals/{{terminal}}/description";
pub const CORE_ADMIN_TERMINAL_TAGS_PATH: &str = "{core_prefix}/admin/terminals/{{terminal}}/tags";
pub const CORE_ADMIN_GROUPS_PATH: &str = "{core_prefix}/admin/groups";
pub const CORE_ADMIN_GROUP_PATH: &str = "{core_prefix}/admin/groups/{{group}}";
pub const CORE_ADMIN_GROUP_NAME_PATH: &str = "{core_prefix}/admin/groups/{{group}}/name";
pub const CORE_ADMIN_GROUP_MEMBERS_PATH: &str = "{core_prefix}/admin/groups/{{group}}/members";
pub const CORE_STATE_PATH: &str = "{core_prefix}/state";
pub const CORE_ADDONS_PATH: &str = "{core_prefix}/addons";
pub const CORE_TELEPHONY_TARGETS_PATH: &str = "{core_prefix}/telephony/targets";
pub const CORE_ASSETS_PATH: &str = "{core_prefix}/assets";
pub const CORE_ASSET_PATH: &str = "{core_prefix}/assets/{{asset_id}}";
pub const CORE_EVENTS_PATH: &str = "{core_prefix}/events";
	pub const CORE_EVENTS_WEBSOCKET_PATH: &str = "{core_prefix}/events/ws";
	pub const CORE_TERMINALS_WEBSOCKET_PATH: &str = "{core_prefix}/terminals/ws";
	pub const CORE_TERMINAL_EVENTS_WEBSOCKET_PATH: &str = "{core_prefix}/terminals/events/ws";
	pub const LOCAL_CORE_HTTP_URL: &str = "http://127.0.0.1:3100";
	pub const LOCAL_CORE_WEBSOCKET_URL: &str = "ws://127.0.0.1:3100{core_prefix}/events/ws";
	pub const LOCAL_CORE_TERMINAL_WEBSOCKET_URL: &str = "ws://127.0.0.1:3100{core_prefix}/terminals/ws";
pub const ADDON_API_VERSION: u16 = {addon_api};
pub const RELEASE_MANIFEST_SCHEMA_VERSION: u32 = {manifest};
pub const COMPONENT_BUNDLE_FORMAT_VERSION: u32 = {bundle};
pub const INSTALLER_STATE_SCHEMA_VERSION: u32 = {state};
pub const INSTALLER_OUTPUT_SCHEMA_VERSION: u32 = {output};
pub const CLOCK_ADDON_VERSION: [u16; 3] = {clock:?};
pub const MESSAGE_ADDON_VERSION: [u16; 3] = {message:?};
pub const WEATHER_ADDON_VERSION: [u16; 3] = {weather:?};
"#,
        core_api = compatibility.core_api,
        addon_api = compatibility.addon_api,
        manifest = compatibility.release_manifest_schema,
        bundle = compatibility.component_bundle_format,
        state = compatibility.installer_state_schema,
        output = compatibility.installer_output_schema,
        clock = addon_version(&compatibility.addons.clock),
        message = addon_version(&compatibility.addons.message),
        weather = addon_version(&compatibility.addons.weather),
    );
    let output = PathBuf::from(env::var("OUT_DIR").unwrap()).join("compatibility.rs");
    fs::write(output, constants).expect("generated compatibility constants must be writable");
}
