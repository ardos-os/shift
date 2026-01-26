use crate::{auth, sessions::{self, SessionId}};

#[derive(Debug)]
pub enum S2CMsg {
    BindToSession {
        id: SessionId,
        role: sessions::Role
    },
    AuthError(auth::error::Error)
}


pub type S2CRx = tokio::sync::mpsc::Receiver<S2CMsg>;
pub type S2CTx = tokio::sync::mpsc::Sender<S2CMsg>;
pub type S2CWeakTx = tokio::sync::mpsc::WeakSender<S2CMsg>;