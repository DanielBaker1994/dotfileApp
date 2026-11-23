//! kitchen-sink shared core: the code both the main daemon (`ws-rs`) and the
//! quick agent (`kitchen-sink quick`) use. This crate must never reference
//! `ws-rs`; anything that needs the main daemon is reached through a hook
//! defined here and installed by `ws-rs`.

#![allow(dead_code)]

pub mod app;
pub mod engines;
pub mod panes;
pub mod platform;
pub mod views;
pub mod ui;
