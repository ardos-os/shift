use std::os::fd::OwnedFd;

use tab_protocol::{BufferIndex, FramebufferLinkPayload};

use crate::monitor::MonitorId;

#[derive(Debug)]
pub enum RenderCmd {
	/// Request the renderer to clean up and exit.
	Shutdown,
	/// Ask the renderer to associate a client-provided framebuffer with internal GPU state.
	FramebufferLink {
		payload: FramebufferLinkPayload,
		dma_bufs: [OwnedFd; 2],
	},
	/// Present a framebuffer on a given monitor.
	SwapBuffers {
		monitor_id: MonitorId,
		buffer: BufferIndex,
	},
}

pub type RenderCmdRx = tokio::sync::mpsc::Receiver<RenderCmd>;
pub type RenderCmdTx = tokio::sync::mpsc::Sender<RenderCmd>;
pub type RenderCmdWeakTx = tokio::sync::mpsc::WeakSender<RenderCmd>;
