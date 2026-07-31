//! Testable deployment engine for the BTS installer.

pub mod activation;
pub mod archive;
pub mod cli;
pub mod config;
pub mod diagnostics;
pub mod manifest;
pub mod model;
pub mod plan;
pub mod platform;
pub mod release;
pub mod state;
pub mod system;

pub const INSTALLER_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_REPOSITORY: &str = "jedrek0429/bts";
pub const DEFAULT_CHANNEL: &str = "stable";
pub const COPYRIGHT: &str = "Copyright © 2026 BTS contributors";

pub fn legal_notice() -> String {
    format!(
        "bts-install {INSTALLER_VERSION}\n{COPYRIGHT}\nLicensed under GPL-3.0-or-later; there is NO WARRANTY.\nRun 'bts-install licence' to view the full licence."
    )
}

pub fn warranty_notice() -> &'static str {
    "This program comes with ABSOLUTELY NO WARRANTY, to the extent permitted by law. See sections 15 and 16 of the GNU General Public License for details."
}
