use std::{collections::HashMap, future::pending, io, path::Path, sync::Arc};

use futures::future::select_all;
use tab_protocol::TabMessageFrame;
use thiserror::Error;
use tokio::{io::unix::AsyncFd, net::{UnixListener, UnixStream, unix::SocketAddr}, task::JoinHandle as TokioJoinHandle};
use tracing::error;

use crate::{auth::Token, client_layer::{client::{Client, ClientId}, client_view::{self, ClientView}}, comms::client2server::C2SMsg, sessions::{PendingSession, Role, Session, SessionId}};
use crate::auth::error::Error as AuthError;
struct ConnectedClient { client_view: ClientView, join_handle: TokioJoinHandle<()> }
impl Drop for ConnectedClient {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
pub struct ShiftServer {
    listener: Option<UnixListener>,
    current_session: Option<SessionId>,
    pending_sessions: HashMap<Token, PendingSession>,
    active_sessions: HashMap<SessionId, Arc<Session>>,
    connected_clients: HashMap<ClientId, ConnectedClient>
}
#[derive(Error, Debug)]
pub enum BindError {
    #[error("io error: {0}")]
    IOError(#[from] std::io::Error)
}
impl ShiftServer {
    #[tracing::instrument(level= "info", skip(path), fields(path = ?path.as_ref().display()))]
    pub async fn bind(path: impl AsRef<Path>) -> Result<Self, BindError> {
        std::fs::remove_file(&path).ok();
        let listener = UnixListener::bind(path)?;
        Ok(Self {
            listener: Some(listener),
            current_session: Default::default(),
            pending_sessions: Default::default(),
            active_sessions: Default::default(),
            connected_clients: Default::default(),
        })
    }
    #[tracing::instrument(level= "info", skip(self), fields(connected_clients=self.connected_clients.len(), active_sessions=self.active_sessions.len(), pending_sessions = self.pending_sessions.len(), current_session = ?self.current_session))]
    pub fn add_initial_session(&mut self) -> Token {
        let (token, session) = PendingSession::admin(Some("Admin".into()));
        let id = session.id();
        self.pending_sessions.insert(token.clone(), session);
        tracing::info!(?token, %id, "added initial admin session");
        token
    }
    #[tracing::instrument(level= "trace", skip(self), fields(connected_clients=self.connected_clients.len(), active_sessions=self.active_sessions.len(), pending_sessions = self.pending_sessions.len(), current_session = ?self.current_session))]
    pub async fn start(mut self) {
        let listener = self.listener.take().unwrap();
        loop {
            tokio::select! {
                client_message = self.read_clients_messages() => self.handle_client_message(client_message.0, client_message.1).await,
                accept_result = listener.accept() => self.handle_accept(accept_result).await,
            }
        }
    }
    
    #[tracing::instrument(level= "trace", skip(self), fields(connected_clients=self.connected_clients.len(), active_sessions=self.active_sessions.len(), pending_sessions = self.pending_sessions.len(), current_session = ?self.current_session))]
    async fn handle_client_message(&mut self, client_id: ClientId, message: C2SMsg) {
        let Some(connected_client) = self.connected_clients.get_mut(&client_id) else {
            tracing::warn!("tried handling message from a non-existing client");
            return;
        };
        let client_session = connected_client.client_view.authenticated_session().and_then(|s| self.active_sessions.get(&s)).map(Arc::clone);
        match message {
            C2SMsg::Shutdown => {
                self.connected_clients.remove(&client_id);
            },
            C2SMsg::Auth(token) => {
                let Some(pending_session) = self.pending_sessions.remove(&token) else {
                    connected_client.client_view.notify_auth_error(AuthError::NotFound).await;
                    return;
                };
                let session = Arc::new(pending_session.promote());
                if !connected_client.client_view.notify_auth_success(&session).await {
                    self.connected_clients.remove(&client_id);
                    tracing::warn!("failed to notify auth success, removing client");
                    return;
                }
                self.active_sessions.insert(session.id(), Arc::clone(&session));
                if session.role() == Role::Admin && self.current_session.is_none() {
                    self.current_session = Some(session.id());
                }
            },
            C2SMsg::CreateSession(req) => {
                let Some(client_session) = client_session else {
                    connected_client.client_view.notify_error("forbidden".into(), None, false).await;
                    return;
                };
                if client_session.role() != Role::Admin {
                    connected_client.client_view.notify_error("forbidden".into(), None, false).await;
                    return;
                }
                let (token, pending_session) = PendingSession::new(req.display_name.map(Arc::from), match req.role {
                    tab_protocol::SessionRole::Admin => Role::Admin,
                    tab_protocol::SessionRole::Session => Role::Normal,
                });
                self.pending_sessions.insert(token.clone(), pending_session.clone());
                if !connected_client.client_view.notify_session_created(token, pending_session).await {
                    tracing::warn!("failed to notify session created, removing client");
                    self.connected_clients.remove(&client_id);
                    return;
                }
            },
            C2SMsg::SwapBuffers { monitor_id, buffer } => todo!(),
            C2SMsg::FramebufferLink { payload, dma_bufs } => todo!()
        }
    }
    async fn read_clients_messages(&mut self) -> (ClientId, C2SMsg) {
        self.connected_clients.retain(|_, c| {
            c.client_view.has_messages()
        });
        let futures = self.connected_clients.iter_mut().map(|c| Box::pin(async {
            let Some(msg) = c.1.client_view.read_message().await else {
                return pending().await;
            };
            (*c.0, msg)
        })).collect::<Vec<_>>();
        if futures.is_empty() {
            return pending().await;
        }
        select_all(futures).await.0
    }
    #[tracing::instrument(level= "info", skip(self, accept_result), fields(connected_clients=self.connected_clients.len(), active_sessions=self.active_sessions.len(), pending_sessions = self.pending_sessions.len(), current_session = ?self.current_session))]
    async fn handle_accept(&mut self, accept_result: io::Result<(UnixStream, SocketAddr)>) {
        match accept_result {
            Ok((client_socket, ip)) => {
                macro_rules! or_continue {
                    ($expr:expr, $fmt:literal $(, $arg:expr)* $(,)?) => {
                        match $expr {
                            Ok(val) => val,
                            Err(e) => {
                                tracing::error!($fmt $(, $arg)*, e);
                                return;
                            }
                        }
                    };
                }

                let hellopkt = TabMessageFrame::hello("shift 0.1.0-alpha");
                let client_async_fd = or_continue!(
                    client_socket.into_std().and_then(AsyncFd::new),
                    "failed to accept connection: AsyncFd creation from client_socket failed: {}"
                );

                or_continue!(
                    hellopkt.send_frame_to_async_fd(&client_async_fd).await,
                    "failed to send hello packet: {}"
                );
                let (new_client, new_client_view) = Client::wrap_socket(client_async_fd);
                let client_id = new_client_view.id();
                self.connected_clients.insert(new_client_view.id(), ConnectedClient { client_view: new_client_view, join_handle: new_client.spawn().await });
                tracing::info!(%client_id, "client successfully connected");
            }
            Err(e) => {
                tracing::error!("failed to accept connection: {e}");
            }
        }
    }
}
