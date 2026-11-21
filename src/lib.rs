pub mod archive;
pub mod auth;
pub mod config;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod error;
pub mod fsops;
pub mod logging;
pub mod paths;
pub mod recycle;
pub mod server;
pub mod state;
pub mod tasks;
pub mod uploads;

pub fn version() -> &'static str {
    config::APP_VERSION
}
