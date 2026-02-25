use crate::comms::input2server::{InputEvtRx, InputEvtTx};

const DEFAULT_CHANNEL_CAPACITY: usize = 4096;

#[derive(Debug)]
pub struct ServerEnd {
	input_events: InputEvtRx,
}

impl ServerEnd {
	pub fn new(input_events: InputEvtRx) -> Self {
		Self { input_events }
	}

	pub fn into_parts(self) -> InputEvtRx {
		self.input_events
	}
}

#[derive(Debug)]
pub struct InputEnd {
	events: InputEvtTx,
}

impl InputEnd {
	pub fn new(events: InputEvtTx) -> Self {
		Self { events }
	}

	pub fn into_parts(self) -> InputEvtTx {
		self.events
	}
}

pub struct Channels {
	server_end: ServerEnd,
	input_end: InputEnd,
}

impl Channels {
	pub fn new() -> Self {
		Self::with_capacity(DEFAULT_CHANNEL_CAPACITY)
	}

	pub fn with_capacity(capacity: usize) -> Self {
		let (evt_tx, evt_rx) = tokio::sync::mpsc::channel(capacity);
		Self {
			server_end: ServerEnd::new(evt_rx),
			input_end: InputEnd::new(evt_tx),
		}
	}

	pub fn split(self) -> (ServerEnd, InputEnd) {
		(self.server_end, self.input_end)
	}
}

impl Default for Channels {
	fn default() -> Self {
		Self::new()
	}
}
