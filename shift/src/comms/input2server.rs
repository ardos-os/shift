use std::sync::Arc;

use tab_protocol::InputEventPayload;

#[derive(Debug, Clone)]
pub enum InputEvt {
	Event(InputEventPayload),
	FatalError { reason: Arc<str> },
}

pub type InputEvtRx = tokio::sync::mpsc::Receiver<InputEvt>;
pub type InputEvtTx = tokio::sync::mpsc::Sender<InputEvt>;
pub type InputEvtWeakTx = tokio::sync::mpsc::WeakSender<InputEvt>;
