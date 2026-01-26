use crate::{auth, client_layer::client::{Client, ClientId}, comms::{client2server::{C2SMsg, C2SRx, C2STx, C2SWeakTx}, server2client::{S2CMsg, S2CRx, S2CTx}}, sessions::{Session, SessionId}};


#[derive(Debug)]
pub struct ChannelsServerEnd(C2SRx, S2CTx);

impl ChannelsServerEnd {
    pub fn to_client(&self) -> &S2CTx {
        &self.1
    }
    pub fn from_client(&mut self) -> &mut C2SRx {
        &mut self.0
    }
}
#[derive(Debug)]
pub struct ChannelsClientEnd(S2CRx, C2STx);

impl ChannelsClientEnd {
    pub fn to_server(&self) -> &C2STx {
        &self.1
    }
    pub fn from_server(&mut self) -> &mut S2CRx {
        &mut self.0
    }
}
#[derive(Debug)]
pub struct Channels {
    pub client_end: ChannelsClientEnd,
    pub server_end: ChannelsServerEnd
}
impl Channels {
    pub(super) fn new() -> Self {
        let c2s = tokio::sync::mpsc::channel(1000);
        let s2c = tokio::sync::mpsc::channel(1000);
        Self {
            client_end: ChannelsClientEnd(s2c.1, c2s.0),
            server_end: ChannelsServerEnd(c2s.1, s2c.0)
        }
    }
}
#[derive(Debug)]
pub struct ClientView {
    id: ClientId,
    pub(super) channels: ChannelsServerEnd,
    session_id: Option<SessionId>
}

impl ClientView {
    pub(super) fn from_client(client: &Client, channels: ChannelsServerEnd) -> ClientView {
        Self {
            id: client.id(),
            channels,
            session_id: None
        }
    }

    pub fn id(&self) -> ClientId {
        self.id
    }
    pub async fn read_message(&mut self) -> Option<C2SMsg> {
        self.channels.from_client().recv().await
    }
    pub fn running(&self) -> bool {
        !self.channels.1.is_closed() && self.has_messages()
    }
    pub fn has_messages(&self) -> bool {
        !self.channels.0.is_closed() || !self.channels.0.is_empty()
    }
    pub async fn notify_auth_error(&self, reason: auth::error::Error) -> bool {
        self.channels.1.send(S2CMsg::AuthError(reason)).await.is_ok()
    }
    pub async fn notify_auth_success(&mut self, session: &Session) -> bool {
        self.session_id = Some(session.id());
        self.channels.1.send(S2CMsg::BindToSession{
            id: session.id(),
            role: session.role()
        }).await.is_ok()
    }
}
