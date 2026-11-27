//! Filavim's reusable editing engine, background services, and native frontend.
//!
//! [`editor`] owns logical editing state; [`gui`] adapts it to GPUI without a
//! second text buffer. Native rendering is gated by the `native` feature, while
//! document, viewport, and service logic remains available to headless tests.
//! See `docs/ARCHITECTURE.md` for the active module graph and legacy-source boundary.
pub mod buffer;
pub mod config;
pub mod coordinates;
pub mod editor;
pub mod files;
pub mod gui;
pub mod input;
pub mod key;

pub mod background;
pub mod git;
pub mod lsp;
pub mod lsp_catalog;
pub mod lsp_manager;
pub mod navigation;
pub mod paths;
pub mod perf;
pub mod queue;
pub mod setup;
pub mod snippet;
pub mod syntax;

pub mod palette;

pub mod pairs;

pub mod appearance;

pub mod recovery;

pub mod search;

pub mod editing;
