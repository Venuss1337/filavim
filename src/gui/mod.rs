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
pub use application::{run, smoke};
