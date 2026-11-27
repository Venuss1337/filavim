//! Native frontend and its headless-testable adapters to the shared editor.
//!
//! `application` creates the window; `view::EditorView` owns documents, focus and
//! service coordination. [`documents`] moves real editors between tabs, while
//! [`services`] validates asynchronous results. Rendering-only modules require
//! `native`; the remaining adapters do not depend on a running GPUI application.
#[cfg(feature = "native")]
mod design;
#[cfg(feature = "native")]
mod floating;
#[cfg(feature = "native")]
mod icons;
#[cfg(feature = "native")]
pub mod input;
pub mod io;
pub mod model;
#[cfg(feature = "native")]
mod theme;
#[cfg(feature = "native")]
mod view;
pub mod viewport;
pub const FONT_FAMILY: &str = "Agave Nerd Font Mono";
pub const FONT_BYTES: &[u8] = include_bytes!("../../assets/fonts/AgaveNerdFontMono-Regular.ttf");

#[cfg(feature = "native")]
mod application;
#[cfg(feature = "native")]
pub use application::{run, smoke, smoke_lsp};

pub mod documents;
pub mod services;

pub mod explorer;

pub mod popup;

pub mod cursor;
#[cfg(feature = "native")]
mod cursor_view;

pub mod completion;
mod completion_context;

#[cfg(feature = "native")]
mod wallpaper;

pub mod toast;
#[cfg(feature = "native")]
mod toast_view;
