//! OxiDav — personal CalDAV/CardDAV/WebDAV server.
//!
//! This library crate exposes the application modules so that both the binary
//! (`src/main.rs`) and the integration tests (`tests/`) can share them.

pub mod api;
pub mod assets;
pub mod auth;
pub mod config;
pub mod dav;
pub mod db;
pub mod router;
