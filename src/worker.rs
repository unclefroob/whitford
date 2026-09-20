use crate::{
    cache, config, gmail, message,
    model::{AccountIdentity, MailboxSnapshot, SyncMetadata},
    oauth::{self, AuthorizationUrl},
    secrets::{self, RefreshToken},
};

const KEYRING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
use std::{
    fmt,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle as ThreadJoinHandle},
    time::SystemTime,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, timeout, timeout_at},
};

const MAX_CALLBACK_HEADERS: usize = 8192;
const CALLBACK_SUCCESS_BODY: &[u8] =
    b"<!doctype html><title>Whitford</title>Authorization received. You may close this tab.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncKind {
    Restore,
    Connect,
    Refresh,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerPhase {
    LoadingConfiguration,
    WaitingForBrowser,
    ExchangingCode,
    OpeningKeyring,
    RefreshingToken,
    VerifyingIdentity,
    ConnectingImap,
    FetchingInbox,
    Disconnecting,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    ConfigurationDirectoryUnavailable,
    ConfigurationMissing,
    ConfigurationUnreadable,
    ConfigurationTooLarge,
    ConfigurationInvalid,
    ConfigurationWrongProject,
    BrowserLaunchFailed,
    AuthorizationDenied,
    AuthorizationTimedOut,
    AuthorizationInvalid,
    Network,
    ProviderUnavailable,
    RateLimited,
    AuthorizationExpired,
    IdentityInvalid,
    KeyringUnavailable,
    CredentialSaveFailed,
    DisconnectFailed,
    TlsFailed,
    ImapAuthenticationFailed,
    InboxUnavailable,
    ImapProtocol,
    SyncTimedOut,
    WorkerUnavailable,
}
impl FailureKind {
    pub fn is_configuration(self) -> bool {
        matches!(
            self,
            Self::ConfigurationDirectoryUnavailable
                | Self::ConfigurationMissing
                | Self::ConfigurationUnreadable
                | Self::ConfigurationTooLarge
                | Self::ConfigurationInvalid
                | Self::ConfigurationWrongProject
        )
    }
}
#[derive(Clone, Eq, PartialEq)]
pub struct ServiceFailure {
    pub kind: FailureKind,
    pub retryable: bool,
    pub preserve_mail: bool,
    pub cleanup_failed: bool,
    pub config_path: Option<String>,
}
impl fmt::Debug for ServiceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceFailure")
            .field("kind", &self.kind)
            .field("retryable", &self.retryable)
            .field("preserve_mail", &self.preserve_mail)
            .field("cleanup_failed", &self.cleanup_failed)
            .field(
                "config_path",
                &self.config_path.as_ref().map(|_| "[REDACTED PATH]"),
            )
            .finish()
    }
}

pub enum WorkerCommand {
    Restore { id: OperationId },
    Connect { id: OperationId },
    Refresh { id: OperationId },
    Disconnect { id: OperationId },
    Cancel { id: OperationId },
    SetCacheLimit { limit: usize },
    Shutdown,
}
impl fmt::Debug for WorkerCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Restore { id } => f.debug_tuple("Restore").field(id).finish(),
            Self::Connect { id } => f.debug_tuple("Connect").field(id).finish(),
            Self::Refresh { id } => f.debug_tuple("Refresh").field(id).finish(),
            Self::Disconnect { id } => f.debug_tuple("Disconnect").field(id).finish(),
            Self::Cancel { id } => f.debug_tuple("Cancel").field(id).finish(),
            Self::SetCacheLimit { limit } => f
                .debug_struct("SetCacheLimit")
                .field("limit", limit)
                .finish(),
            Self::Shutdown => f.write_str("Shutdown"),
        }
    }
}

pub enum WorkerEvent {
    Phase {
        id: OperationId,
        phase: WorkerPhase,
    },
    AuthorizationRequired {
        id: OperationId,
        url: AuthorizationUrl,
        deadline: SystemTime,
    },
    AccountPersisted {
        id: OperationId,
        account: AccountIdentity,
    },
    CacheLoaded {
        id: OperationId,
        account: AccountIdentity,
        snapshot: MailboxSnapshot,
    },
    NoStoredAccount {
        id: OperationId,
    },
    SyncComplete {
        id: OperationId,
        account: AccountIdentity,
        snapshot: MailboxSnapshot,
    },
    Disconnected {
        id: OperationId,
    },
    Cancelled {
        id: OperationId,
    },
    Failed {
        id: OperationId,
        failure: ServiceFailure,
    },
}
impl fmt::Debug for WorkerEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Phase { id, phase } => f
                .debug_struct("Phase")
                .field("id", id)
                .field("phase", phase)
                .finish(),
            Self::AuthorizationRequired { id, .. } => f
                .debug_struct("AuthorizationRequired")
                .field("id", id)
                .finish(),
            Self::AccountPersisted { id, .. } => {
                f.debug_struct("AccountPersisted").field("id", id).finish()
            }
            Self::CacheLoaded { id, snapshot, .. } => f
                .debug_struct("CacheLoaded")
                .field("id", id)
                .field("loaded", &snapshot.metadata.loaded_count)
                .finish(),
            Self::NoStoredAccount { id } => {
                f.debug_struct("NoStoredAccount").field("id", id).finish()
            }
            Self::SyncComplete { id, snapshot, .. } => f
                .debug_struct("SyncComplete")
                .field("id", id)
                .field("loaded", &snapshot.metadata.loaded_count)
                .finish(),
            Self::Disconnected { id } => f.debug_struct("Disconnected").field("id", id).finish(),
            Self::Cancelled { id } => f.debug_struct("Cancelled").field("id", id).finish(),
            Self::Failed { id, failure } => f
                .debug_struct("Failed")
                .field("id", id)
                .field("failure", failure)
                .finish(),
        }
    }
}

pub struct WorkerHandle {
    pub sender: mpsc::UnboundedSender<WorkerCommand>,
    thread: Option<ThreadJoinHandle<()>>,
}
impl WorkerHandle {
    pub fn start() -> (Self, mpsc::UnboundedReceiver<WorkerEvent>) {
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let thread = thread::Builder::new()
            .name("whitford-mail-worker".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                if let Ok(runtime) = runtime {
                    runtime.block_on(controller(commands_rx, events_tx));
                }
            })
            .ok();
        (
            Self {
                sender: commands_tx,
                thread,
            },
            events_rx,
        )
    }

    pub fn shutdown_and_join(mut self) -> thread::Result<()> {
        let _ = self.sender.send(WorkerCommand::Shutdown);
        drop(self.sender);
        self.thread.take().map_or(Ok(()), ThreadJoinHandle::join)
    }
}

async fn controller(
    mut commands: mpsc::UnboundedReceiver<WorkerCommand>,
    events: mpsc::UnboundedSender<WorkerEvent>,
) {
    struct Active {
        id: OperationId,
        task: JoinHandle<()>,
        cleanup: Arc<AtomicBool>,
    }
    let mut active: Option<Active> = None;
    loop {
        let command = if let Some(current) = active.as_mut() {
            tokio::select! {
                biased;
                command = commands.recv() => command,
                result = &mut current.task => {
                    let id = current.id;
                    if join_failed(&result) {
                        let cleanup_failed = if current.cleanup.load(Ordering::Acquire) {
                            !matches!(timeout(KEYRING_TIMEOUT, secrets::delete()).await, Ok(Ok(())))
                        } else {
                            false
                        };
                        if cleanup_failed {
                            fail_cleanup(&events, id, true);
                        } else {
                            fail(&events, id, FailureKind::WorkerUnavailable, false, true);
                        }
                    }
                    active = None;
                    continue;
                }
            }
        } else {
            commands.recv().await
        };
        let Some(command) = command else {
            if let Some(current) = active.take() {
                let Active { id, task, cleanup } = current;
                let cleanup_failed = abort_with_cleanup(task, &cleanup, async {
                    matches!(
                        timeout(KEYRING_TIMEOUT, secrets::delete()).await,
                        Ok(Ok(()))
                    )
                })
                .await;
                if cleanup_failed {
                    fail_cleanup(&events, id, true);
                }
            }
            break;
        };
        if let WorkerCommand::SetCacheLimit { limit } = &command {
            let _ = cache::save_limit_and_prune(*limit);
            continue;
        }
        let cleanup_failed = if let Some(current) = active.take() {
            let Active { task, cleanup, .. } = current;
            abort_with_cleanup(task, &cleanup, async {
                matches!(
                    timeout(KEYRING_TIMEOUT, secrets::delete()).await,
                    Ok(Ok(()))
                )
            })
            .await
        } else {
            false
        };
        if matches!(command, WorkerCommand::Shutdown) {
            break;
        }
        if let WorkerCommand::Cancel { id } = command {
            if cleanup_failed {
                fail_cleanup(&events, id, true);
            } else {
                let _ = events.send(WorkerEvent::Cancelled { id });
            }
            continue;
        }
        if cleanup_failed {
            let id = command_id(&command).unwrap_or(OperationId(0));
            fail_cleanup(&events, id, true);
            continue;
        }
        let id = command_id(&command).unwrap_or(OperationId(0));
        let cleanup = Arc::new(AtomicBool::new(false));
        let task = match command {
            WorkerCommand::Disconnect { id } => {
                let tx = events.clone();
                tokio::spawn(async move {
                    emit_phase(&tx, id, WorkerPhase::Disconnecting);
                    match timeout(KEYRING_TIMEOUT, secrets::delete()).await {
                        Ok(Ok(())) => {
                            if cache::clear_mailbox().is_ok() {
                                let _ = tx.send(WorkerEvent::Disconnected { id });
                            } else {
                                fail_cleanup(&tx, id, true);
                            }
                        }
                        Ok(Err(_)) | Err(_) => fail_cleanup(&tx, id, true),
                    }
                })
            }
            WorkerCommand::Connect { id } => {
                let tx = events.clone();
                let cleanup_task = cleanup.clone();
                tokio::spawn(async move {
                    let result = connect(id, &tx, &cleanup_task).await;
                    if !result.as_ref().is_err_and(|failure| failure.cleanup_failed) {
                        cleanup_task.store(false, Ordering::Release);
                    }
                    if let Err(failure) = result {
                        let _ = tx.send(WorkerEvent::Failed { id, failure });
                    }
                })
            }
            WorkerCommand::Restore { id } => {
                let tx = events.clone();
                let cleanup_task = cleanup.clone();
                tokio::spawn(async move {
                    let result = restore_or_refresh(id, &tx, true, &cleanup_task).await;
                    if !result.as_ref().is_err_and(|failure| failure.cleanup_failed) {
                        cleanup_task.store(false, Ordering::Release);
                    }
                    if let Err(failure) = result {
                        let _ = tx.send(WorkerEvent::Failed { id, failure });
                    }
                })
            }
            WorkerCommand::Refresh { id } => {
                let tx = events.clone();
                let cleanup_task = cleanup.clone();
                tokio::spawn(async move {
                    let result = restore_or_refresh(id, &tx, false, &cleanup_task).await;
                    if !result.as_ref().is_err_and(|failure| failure.cleanup_failed) {
                        cleanup_task.store(false, Ordering::Release);
                    }
                    if let Err(failure) = result {
                        let _ = tx.send(WorkerEvent::Failed { id, failure });
                    }
                })
            }
            WorkerCommand::Cancel { .. }
            | WorkerCommand::SetCacheLimit { .. }
            | WorkerCommand::Shutdown => unreachable!(),
        };
        active = Some(Active { id, task, cleanup });
    }
}

fn join_failed(result: &Result<(), tokio::task::JoinError>) -> bool {
    result.as_ref().is_err_and(|error| !error.is_cancelled())
}

async fn abort_with_cleanup<F: Future<Output = bool>>(
    task: JoinHandle<()>,
    cleanup_required: &AtomicBool,
    cleanup: F,
) -> bool {
    task.abort();
    let _ = task.await;
    if !cleanup_required.load(Ordering::Acquire) {
        return false;
    }
    let cleaned = cleanup.await;
    if cleaned {
        cleanup_required.store(false, Ordering::Release);
    }
    !cleaned
}

async fn save_with_cleanup<Save, SaveFuture, Cleanup, CleanupFuture>(
    cleanup_required: &AtomicBool,
    preserve_mail: bool,
    save: Save,
    cleanup: Cleanup,
) -> Result<(), ServiceFailure>
where
    Save: FnOnce() -> SaveFuture,
    SaveFuture: Future<Output = bool>,
    Cleanup: FnOnce() -> CleanupFuture,
    CleanupFuture: Future<Output = bool>,
{
    cleanup_required.store(true, Ordering::Release);
    if save().await {
        return Ok(());
    }
    if cleanup().await {
        cleanup_required.store(false, Ordering::Release);
        Err(failure(
            FailureKind::CredentialSaveFailed,
            true,
            preserve_mail,
        ))
    } else {
        Err(ServiceFailure {
            kind: FailureKind::DisconnectFailed,
            retryable: true,
            preserve_mail,
            cleanup_failed: true,
            config_path: None,
        })
    }
}

fn command_id(command: &WorkerCommand) -> Option<OperationId> {
    match command {
        WorkerCommand::Restore { id }
        | WorkerCommand::Connect { id }
        | WorkerCommand::Refresh { id }
        | WorkerCommand::Disconnect { id }
        | WorkerCommand::Cancel { id } => Some(*id),
        WorkerCommand::SetCacheLimit { .. } | WorkerCommand::Shutdown => None,
    }
}

async fn connect(
    id: OperationId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    cleanup_required: &AtomicBool,
) -> Result<(), ServiceFailure> {
    emit_phase(tx, id, WorkerPhase::LoadingConfiguration);
    let config = config::load().map_err(|error| map_config(error, false))?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| failure(FailureKind::AuthorizationInvalid, true, false))?;
    let port = listener
        .local_addr()
        .map_err(|_| failure(FailureKind::AuthorizationInvalid, true, false))?
        .port();
    let request =
        oauth::authorization_request(&config, port, SystemTime::now()).map_err(map_oauth)?;
    let url = AuthorizationUrl::new(request.url.expose().to_owned());
    let _ = tx.send(WorkerEvent::AuthorizationRequired {
        id,
        url,
        deadline: request.deadline,
    });
    emit_phase(tx, id, WorkerPhase::WaitingForBrowser);
    let host = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + oauth::CALLBACK_TIMEOUT;
    let (mut socket, code) = loop {
        let (mut socket, peer) = timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| failure(FailureKind::AuthorizationTimedOut, true, false))?
            .map_err(|_| failure(FailureKind::AuthorizationInvalid, true, false))?;
        let bytes = read_callback_headers(&mut socket, deadline)
            .await
            .map_err(map_oauth)?;
        match oauth::parse_callback_request(&bytes, peer, &host, request.csrf_state.expose()) {
            Ok(code) => break (socket, code),
            Err(oauth::OAuthError::CallbackWrongPath) => {
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            }
            Err(error) => return Err(map_oauth(error)),
        }
    };
    let response = callback_success_header();
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.write_all(CALLBACK_SUCCESS_BODY).await;
    emit_phase(tx, id, WorkerPhase::ExchangingCode);
    let client = oauth::http_client().map_err(map_oauth)?;
    let grant = oauth::exchange_code(&config, &request, &code, &client)
        .await
        .map_err(map_oauth)?;
    emit_phase(tx, id, WorkerPhase::VerifyingIdentity);
    let account = oauth::fetch_identity(&grant.access_token, &client)
        .await
        .map_err(map_oauth)?;
    let refresh = oauth::require_initial_refresh_token(&grant)
        .map_err(|_| failure(FailureKind::AuthorizationExpired, false, false))?;
    let token = RefreshToken::new(refresh.expose().to_owned())
        .map_err(|_| failure(FailureKind::CredentialSaveFailed, true, false))?;
    emit_phase(tx, id, WorkerPhase::OpeningKeyring);
    let fetched = persist_then_fetch(
        || async {
            save_with_cleanup(
                cleanup_required,
                false,
                || async {
                    matches!(
                        timeout(KEYRING_TIMEOUT, secrets::replace(&token)).await,
                        Ok(Ok(()))
                    )
                },
                || async {
                    matches!(
                        timeout(KEYRING_TIMEOUT, secrets::delete()).await,
                        Ok(Ok(()))
                    )
                },
            )
            .await
        },
        || {
            let _ = tx.send(WorkerEvent::AccountPersisted {
                id,
                account: account.clone(),
            });
            emit_phase(tx, id, WorkerPhase::ConnectingImap);
        },
        || async {
            gmail::fetch_inbox(&account.email, grant.access_token.expose())
                .await
                .map_err(map_gmail)
        },
    )
    .await?;
    complete_sync(id, tx, account, fetched)
}

fn callback_success_header() -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        CALLBACK_SUCCESS_BODY.len()
    )
}

async fn persist_then_fetch<Save, SaveFuture, Notify, Fetch, FetchFuture>(
    save: Save,
    notify: Notify,
    fetch: Fetch,
) -> Result<gmail::InboxFetch, ServiceFailure>
where
    Save: FnOnce() -> SaveFuture,
    SaveFuture: Future<Output = Result<(), ServiceFailure>>,
    Notify: FnOnce(),
    Fetch: FnOnce() -> FetchFuture,
    FetchFuture: Future<Output = Result<gmail::InboxFetch, ServiceFailure>>,
{
    save().await?;
    notify();
    fetch().await
}

async fn read_callback_headers<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    deadline: Instant,
) -> Result<Vec<u8>, oauth::OAuthError> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = timeout_at(deadline, reader.read(&mut chunk))
            .await
            .map_err(|_| oauth::OAuthError::AuthorizationTimedOut)?
            .map_err(|_| oauth::OAuthError::CallbackInvalid)?;
        if read == 0 {
            return Err(oauth::OAuthError::CallbackInvalid);
        }
        if request.len().saturating_add(read) > MAX_CALLBACK_HEADERS {
            return Err(oauth::OAuthError::CallbackInvalid);
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(request);
        }
    }
}

async fn restore_or_refresh(
    id: OperationId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    restore: bool,
    cleanup_required: &AtomicBool,
) -> Result<(), ServiceFailure> {
    emit_phase(tx, id, WorkerPhase::OpeningKeyring);
    let Some(token) = timeout(KEYRING_TIMEOUT, secrets::load())
        .await
        .map_err(|_| failure(FailureKind::KeyringUnavailable, true, true))?
        .map_err(|_| failure(FailureKind::KeyringUnavailable, true, true))?
    else {
        if restore {
            let _ = tx.send(WorkerEvent::NoStoredAccount { id });
            return Ok(());
        }
        return Err(failure(FailureKind::AuthorizationExpired, false, true));
    };
    if let Ok(Some((account, snapshot))) = cache::load_latest(cache::load_limit()) {
        let _ = tx.send(WorkerEvent::CacheLoaded {
            id,
            account,
            snapshot,
        });
    }
    emit_phase(tx, id, WorkerPhase::LoadingConfiguration);
    let config = config::load().map_err(|error| map_config(error, true))?;
    let client = oauth::http_client().map_err(map_oauth)?;
    emit_phase(tx, id, WorkerPhase::RefreshingToken);
    let grant = match oauth::refresh(&config, token.expose(), &client).await {
        Ok(grant) => grant,
        Err(oauth::OAuthError::AuthorizationExpired) => {
            emit_phase(tx, id, WorkerPhase::OpeningKeyring);
            let cleanup_failed = !matches!(
                timeout(KEYRING_TIMEOUT, secrets::delete()).await,
                Ok(Ok(()))
            );
            return Err(if cleanup_failed {
                ServiceFailure {
                    kind: FailureKind::DisconnectFailed,
                    retryable: true,
                    preserve_mail: true,
                    cleanup_failed: true,
                    config_path: None,
                }
            } else {
                ServiceFailure {
                    kind: FailureKind::AuthorizationExpired,
                    retryable: false,
                    preserve_mail: true,
                    cleanup_failed: false,
                    config_path: None,
                }
            });
        }
        Err(error) => return Err(map_oauth(error)),
    };
    if let Some(replacement) = grant.refresh_token.as_ref() {
        let token = RefreshToken::new(replacement.expose().to_owned())
            .map_err(|_| failure(FailureKind::CredentialSaveFailed, true, true))?;
        save_with_cleanup(
            cleanup_required,
            true,
            || async {
                matches!(
                    timeout(KEYRING_TIMEOUT, secrets::replace(&token)).await,
                    Ok(Ok(()))
                )
            },
            || async {
                matches!(
                    timeout(KEYRING_TIMEOUT, secrets::delete()).await,
                    Ok(Ok(()))
                )
            },
        )
        .await?;
    }
    emit_phase(tx, id, WorkerPhase::VerifyingIdentity);
    let account = oauth::fetch_identity(&grant.access_token, &client)
        .await
        .map_err(map_oauth)?;
    sync(id, tx, account, grant.access_token.expose()).await
}

async fn sync(
    id: OperationId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    account: AccountIdentity,
    access_token: &str,
) -> Result<(), ServiceFailure> {
    emit_phase(tx, id, WorkerPhase::ConnectingImap);
    let fetched = gmail::fetch_inbox(&account.email, access_token)
        .await
        .map_err(map_gmail)?;
    complete_sync(id, tx, account, fetched)
}

fn complete_sync(
    id: OperationId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    account: AccountIdentity,
    fetched: gmail::InboxFetch,
) -> Result<(), ServiceFailure> {
    emit_phase(tx, id, WorkerPhase::FetchingInbox);
    let (messages, fallback_count) = message::map_messages(fetched.records);
    let fresh = MailboxSnapshot {
        metadata: SyncMetadata {
            completed_at: SystemTime::now(),
            requested_limit: gmail::MESSAGE_LIMIT,
            loaded_count: messages.len(),
            fallback_count,
            skipped_count: fetched.skipped_count,
        },
        messages,
    };
    let limit = cache::load_limit();
    let snapshot = cache::merge_and_save(&account, fresh.clone(), limit).unwrap_or(fresh);
    let _ = tx.send(WorkerEvent::SyncComplete {
        id,
        account,
        snapshot,
    });
    Ok(())
}

fn emit_phase(tx: &mpsc::UnboundedSender<WorkerEvent>, id: OperationId, phase: WorkerPhase) {
    let _ = tx.send(WorkerEvent::Phase { id, phase });
}
fn failure(kind: FailureKind, retryable: bool, preserve_mail: bool) -> ServiceFailure {
    ServiceFailure {
        kind,
        retryable,
        preserve_mail,
        cleanup_failed: false,
        config_path: None,
    }
}

fn map_config(error: config::ConfigLoadError, preserve_mail: bool) -> ServiceFailure {
    let kind = match error.kind {
        config::ConfigError::DirectoryUnavailable => FailureKind::ConfigurationDirectoryUnavailable,
        config::ConfigError::Missing => FailureKind::ConfigurationMissing,
        config::ConfigError::Unreadable => FailureKind::ConfigurationUnreadable,
        config::ConfigError::TooLarge => FailureKind::ConfigurationTooLarge,
        config::ConfigError::Invalid => FailureKind::ConfigurationInvalid,
        config::ConfigError::WrongProject => FailureKind::ConfigurationWrongProject,
    };
    ServiceFailure {
        kind,
        retryable: true,
        preserve_mail,
        cleanup_failed: false,
        config_path: error.path.map(|path| path.to_string_lossy().into_owned()),
    }
}
fn fail(
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    id: OperationId,
    kind: FailureKind,
    retryable: bool,
    preserve_mail: bool,
) {
    let _ = tx.send(WorkerEvent::Failed {
        id,
        failure: failure(kind, retryable, preserve_mail),
    });
}
fn fail_cleanup(tx: &mpsc::UnboundedSender<WorkerEvent>, id: OperationId, preserve_mail: bool) {
    let _ = tx.send(WorkerEvent::Failed {
        id,
        failure: ServiceFailure {
            kind: FailureKind::DisconnectFailed,
            retryable: true,
            preserve_mail,
            cleanup_failed: true,
            config_path: None,
        },
    });
}
fn map_oauth(error: oauth::OAuthError) -> ServiceFailure {
    let kind = match error {
        oauth::OAuthError::AuthorizationDenied => FailureKind::AuthorizationDenied,
        oauth::OAuthError::AuthorizationTimedOut => FailureKind::AuthorizationTimedOut,
        oauth::OAuthError::StateMismatch | oauth::OAuthError::CallbackInvalid => {
            FailureKind::AuthorizationInvalid
        }
        oauth::OAuthError::Network => FailureKind::Network,
        oauth::OAuthError::ProviderUnavailable => FailureKind::ProviderUnavailable,
        oauth::OAuthError::RateLimited => FailureKind::RateLimited,
        oauth::OAuthError::AuthorizationExpired => FailureKind::AuthorizationExpired,
        oauth::OAuthError::IdentityInvalid => FailureKind::IdentityInvalid,
        _ => FailureKind::AuthorizationInvalid,
    };
    failure(
        kind,
        !matches!(kind, FailureKind::AuthorizationDenied),
        true,
    )
}
fn map_gmail(error: gmail::GmailError) -> ServiceFailure {
    let kind = match error {
        gmail::GmailError::Offline => FailureKind::Network,
        gmail::GmailError::TlsFailed => FailureKind::TlsFailed,
        gmail::GmailError::AuthenticationFailed => FailureKind::ImapAuthenticationFailed,
        gmail::GmailError::InboxUnavailable => FailureKind::InboxUnavailable,
        gmail::GmailError::Protocol => FailureKind::ImapProtocol,
        gmail::GmailError::TimedOut => FailureKind::SyncTimedOut,
    };
    failure(kind, true, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    #[test]
    fn debug_contract_redacts_payloads() {
        let command = WorkerCommand::Refresh { id: OperationId(3) };
        assert!(!format!("{command:?}").contains("token"));
        let event = WorkerEvent::Failed {
            id: OperationId(3),
            failure: failure(FailureKind::Network, true, true),
        };
        assert!(!format!("{event:?}").contains("canary@example.com"));
    }
    #[test]
    fn configuration_failures_keep_category_and_safe_path() {
        let mapped = map_config(
            config::ConfigLoadError {
                kind: config::ConfigError::WrongProject,
                path: Some("/home/person/.config/whitford/google-oauth.json".into()),
            },
            true,
        );
        assert_eq!(mapped.kind, FailureKind::ConfigurationWrongProject);
        assert!(mapped.retryable);
        assert!(mapped.preserve_mail);
        assert_eq!(
            mapped.config_path.as_deref(),
            Some("/home/person/.config/whitford/google-oauth.json")
        );
        assert!(!format!("{mapped:?}").contains("/home/person"));
    }
    #[test]
    fn callback_success_content_length_matches_body() {
        let header = callback_success_header();
        let length = header
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert_eq!(length, CALLBACK_SUCCESS_BODY.len());
    }
    #[test]
    fn callback_reader_supports_fragments_exact_limit_and_timeout() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (mut client, mut server) = tokio::io::duplex(16_384);
            let writer = tokio::spawn(async move {
                for part in [
                    b"GET /oauth/callback?code=a".as_slice(),
                    b"&state=s HTTP/1.1\r\nHost: 127.0.0.1:1\r\n",
                    b"\r\n",
                ] {
                    server.write_all(part).await.unwrap();
                }
            });
            let request = read_callback_headers(
                &mut client,
                Instant::now() + std::time::Duration::from_secs(1),
            )
            .await
            .unwrap();
            writer.await.unwrap();
            assert!(request.ends_with(b"\r\n\r\n"));

            let base =
                b"GET /oauth/callback?code=a&state=s HTTP/1.1\r\nHost: 127.0.0.1:1\r\nX-Pad: ";
            let mut exact = base.to_vec();
            exact.extend(std::iter::repeat_n(
                b'x',
                MAX_CALLBACK_HEADERS - base.len() - 4,
            ));
            exact.extend_from_slice(b"\r\n\r\n");
            let (mut reader, mut writer) = tokio::io::duplex(16_384);
            writer.write_all(&exact).await.unwrap();
            assert_eq!(
                read_callback_headers(
                    &mut reader,
                    Instant::now() + std::time::Duration::from_secs(1)
                )
                .await
                .unwrap()
                .len(),
                MAX_CALLBACK_HEADERS
            );
            let (mut reader, mut writer) = tokio::io::duplex(16_384);
            writer
                .write_all(&vec![b'x'; MAX_CALLBACK_HEADERS + 1])
                .await
                .unwrap();
            assert_eq!(
                read_callback_headers(
                    &mut reader,
                    Instant::now() + std::time::Duration::from_secs(1)
                )
                .await
                .unwrap_err(),
                oauth::OAuthError::CallbackInvalid
            );
            let (mut reader, _writer) = tokio::io::duplex(16);
            assert_eq!(
                read_callback_headers(
                    &mut reader,
                    Instant::now() + std::time::Duration::from_millis(1)
                )
                .await
                .unwrap_err(),
                oauth::OAuthError::AuthorizationTimedOut
            );
        });
    }

    #[test]
    fn first_connect_and_rotation_persist_before_notification_and_imap() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
            let save_calls = calls.clone();
            let notify_calls = calls.clone();
            let fetch_calls = calls.clone();
            let result = persist_then_fetch(
                move || async move {
                    save_calls.lock().unwrap().push("persist");
                    Ok(())
                },
                move || notify_calls.lock().unwrap().push("account-persisted"),
                move || async move {
                    fetch_calls.lock().unwrap().push("imap");
                    Ok(gmail::InboxFetch {
                        records: vec![],
                        skipped_count: 0,
                    })
                },
            )
            .await
            .unwrap();
            assert!(result.records.is_empty());
            assert_eq!(
                *calls.lock().unwrap(),
                ["persist", "account-persisted", "imap"]
            );
        });
    }

    #[test]
    fn credential_save_and_boundary_failure_short_circuit_imap() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let fetched = Arc::new(AtomicBool::new(false));
            let fetched_call = fetched.clone();
            let result = persist_then_fetch(
                || async { Err(failure(FailureKind::CredentialSaveFailed, true, false)) },
                || panic!("must not notify"),
                move || async move {
                    fetched_call.store(true, Ordering::Release);
                    Ok(gmail::InboxFetch {
                        records: vec![],
                        skipped_count: 0,
                    })
                },
            )
            .await;
            assert_eq!(result.unwrap_err().kind, FailureKind::CredentialSaveFailed);
            assert!(!fetched.load(Ordering::Acquire));
        });
    }

    #[test]
    fn indeterminate_save_requires_confirmed_cleanup() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let cleanup_required = AtomicBool::new(false);
            let partial_save = Arc::new(AtomicBool::new(false));
            let partial_save_call = partial_save.clone();
            let result = save_with_cleanup(
                &cleanup_required,
                false,
                move || async move {
                    partial_save_call.store(true, Ordering::Release);
                    false
                },
                || async { true },
            )
            .await;
            assert!(partial_save.load(Ordering::Acquire));
            assert_eq!(result.unwrap_err().kind, FailureKind::CredentialSaveFailed);
            assert!(!cleanup_required.load(Ordering::Acquire));

            let result = save_with_cleanup(
                &cleanup_required,
                true,
                || async { false },
                || async { false },
            )
            .await;
            let failure = result.unwrap_err();
            assert_eq!(failure.kind, FailureKind::DisconnectFailed);
            assert!(failure.cleanup_failed);
            assert!(failure.preserve_mail);
            assert!(cleanup_required.load(Ordering::Acquire));
        });
    }

    #[test]
    fn controller_cancel_shutdown_and_channel_close_terminate_cleanly() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (command_tx, command_rx) = mpsc::unbounded_channel();
            let (event_tx, mut event_rx) = mpsc::unbounded_channel();
            let task = tokio::spawn(controller(command_rx, event_tx));
            command_tx
                .send(WorkerCommand::Cancel { id: OperationId(9) })
                .unwrap();
            assert!(matches!(
                event_rx.recv().await,
                Some(WorkerEvent::Cancelled { id: OperationId(9) })
            ));
            command_tx.send(WorkerCommand::Shutdown).unwrap();
            task.await.unwrap();
            assert!(event_rx.recv().await.is_none());

            let (command_tx, command_rx) = mpsc::unbounded_channel();
            let (event_tx, _event_rx) = mpsc::unbounded_channel();
            let task = tokio::spawn(controller(command_rx, event_tx));
            drop(command_tx);
            task.await.unwrap();
        });
    }

    #[test]
    fn lifecycle_owner_sends_shutdown_and_joins_worker_thread() {
        let (worker, mut events) = WorkerHandle::start();
        worker
            .sender
            .send(WorkerCommand::Cancel {
                id: OperationId(17),
            })
            .unwrap();
        assert!(matches!(
            events.blocking_recv(),
            Some(WorkerEvent::Cancelled {
                id: OperationId(17)
            })
        ));

        worker.shutdown_and_join().unwrap();
        assert!(events.blocking_recv().is_none());
    }
    #[test]
    fn cancellation_cleanup_is_stage_aware_and_reports_cleanup_failure() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let waiting = AtomicBool::new(false);
            let called = Arc::new(AtomicBool::new(false));
            let called_cleanup = called.clone();
            let task = tokio::spawn(std::future::pending::<()>());
            assert!(
                !abort_with_cleanup(task, &waiting, async move {
                    called_cleanup.store(true, Ordering::Release);
                    true
                })
                .await
            );
            assert!(!called.load(Ordering::Acquire));

            let persisted = AtomicBool::new(true);
            let task = tokio::spawn(std::future::pending::<()>());
            assert!(!abort_with_cleanup(task, &persisted, async { true }).await);
            assert!(!persisted.load(Ordering::Acquire));
            persisted.store(true, Ordering::Release);
            let task = tokio::spawn(std::future::pending::<()>());
            assert!(abort_with_cleanup(task, &persisted, async { false }).await);
            assert!(persisted.load(Ordering::Acquire));
        });
    }
    #[test]
    fn panicked_worker_task_is_classified_as_unavailable() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let result = tokio::spawn(async { panic!("test panic") }).await;
            assert!(join_failed(&result));
        });
    }
}
