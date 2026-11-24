//! Desktop services, usable and testable without a webview.
pub mod service;
#[cfg(feature = "desktop")]
mod shell;
#[cfg(feature = "desktop")]
pub use shell::run;
