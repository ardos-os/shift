use std::os::fd::OwnedFd;

use tab_protocol::{BufferIndex, FramebufferLinkPayload, SessionCreatePayload};

use crate::{auth::Token, monitor::MonitorId};
#[derive(Debug)]
pub enum C2SMsg {
    Shutdown,
    Auth(Token),
    CreateSession(SessionCreatePayload),
    SwapBuffers {
        monitor_id: MonitorId,
	    buffer: BufferIndex,
    },
    FramebufferLink {
		payload: FramebufferLinkPayload,
		dma_bufs: [OwnedFd; 2],
	},
}


pub type C2SRx = tokio::sync::mpsc::Receiver<C2SMsg>;
pub type C2STx = tokio::sync::mpsc::Sender<C2SMsg>;
pub type C2SWeakTx = tokio::sync::mpsc::WeakSender<C2SMsg>;