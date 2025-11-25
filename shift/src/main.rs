#![allow(unsafe_op_in_unsafe_fn)]

use tracing::error;
use tracing_subscriber::EnvFilter;

#[macro_use]
mod macros;
mod animations;
mod app;
mod dma_buf_importer;
mod egl;
mod error;
mod input;
mod opengl;
mod output;
mod presenter;
mod renderer;

use crate::app::ShiftApp;

fn main() {
	init_tracing();
	if let Err(err) = ShiftApp::new().and_then(|mut app| app.run()) {
		error!(error = %err, "Shift daemon crashed");
	}
}

fn init_tracing() {
	let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
	tracing_subscriber::fmt().with_env_filter(filter).init();
}
