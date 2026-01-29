use std::sync::Arc;

use crate::monitor::{Monitor, MonitorId};

/// Events emitted by the rendering layer back into the server core.
#[derive(Debug)]
pub enum RenderEvt {
	/// Rendering layer has started successfully
	Started {
		/// Initial monitors when shift started
		monitors: Vec<Monitor>,
	},
	/// The user plugged in a new monitor
	MonitorOnline { monitor: Monitor },
	/// The user unplugged a monitor
	MonitorOffline { monitor_id: MonitorId },
	/// Rendering reported an unrecoverable condition.
	FatalError { reason: Arc<str> },
	/// Some monitors just page flipped and are ready to be commited to again
	PageFlip { monitors: Vec<MonitorId> },
}

pub type RenderEvtRx = tokio::sync::mpsc::Receiver<RenderEvt>;
pub type RenderEvtTx = tokio::sync::mpsc::Sender<RenderEvt>;
pub type RenderEvtWeakTx = tokio::sync::mpsc::WeakSender<RenderEvt>;
