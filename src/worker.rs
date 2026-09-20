use crate::{
    cache, config, gmail, message,
    model::{AccountIdentity, CacheUsage, MailboxSnapshot, MessageBody, MessageId, SyncMetadata},
    oauth::{self, AuthorizationUrl},
    secrets::{self, RefreshToken},
};
use futures_util::FutureExt;

const KEYRING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
use std::{
    fmt,
    future::Future,
    sync::{
        Arc, Mutex,
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
use zeroize::Zeroizing;

const MAX_CALLBACK_HEADERS: usize = 8192;
const CALLBACK_SUCCESS_BODY: &[u8] =
    b"<!doctype html><title>Whitford</title>Authorization received. You may close this tab.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BodyRequestId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheOperationId(pub u64);
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyFailure {
    Offline,
    TimedOut,
    AuthorizationRequired,
    MailboxChanged,
    Missing,
    Protocol,
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
    Restore {
        id: OperationId,
    },
    Connect {
        id: OperationId,
    },
    Refresh {
        id: OperationId,
    },
    Disconnect {
        id: OperationId,
        generation: u64,
    },
    Cancel {
        id: OperationId,
    },
    SetCacheLimit {
        limit: usize,
    },
    FetchBody {
        request_id: BodyRequestId,
        generation: u64,
        account_email: String,
        message_id: MessageId,
    },
    ClearBodyCache {
        operation_id: CacheOperationId,
        generation: u64,
    },
    Shutdown,
}
impl fmt::Debug for WorkerCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Restore { id } => f.debug_tuple("Restore").field(id).finish(),
            Self::Connect { id } => f.debug_tuple("Connect").field(id).finish(),
            Self::Refresh { id } => f.debug_tuple("Refresh").field(id).finish(),
            Self::Disconnect { id, generation } => f
                .debug_struct("Disconnect")
                .field("id", id)
                .field("generation", generation)
                .finish(),
            Self::Cancel { id } => f.debug_tuple("Cancel").field(id).finish(),
            Self::SetCacheLimit { limit } => f
                .debug_struct("SetCacheLimit")
                .field("limit", limit)
                .finish(),
            Self::FetchBody {
                request_id,
                generation,
                ..
            } => f
                .debug_struct("FetchBody")
                .field("request_id", request_id)
                .field("generation", generation)
                .finish(),
            Self::ClearBodyCache {
                operation_id,
                generation,
            } => f
                .debug_struct("ClearBodyCache")
                .field("operation_id", operation_id)
                .field("generation", generation)
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
    BodyLoaded {
        request_id: BodyRequestId,
        generation: u64,
        message_id: MessageId,
        body: Arc<MessageBody>,
        usage: CacheUsage,
        saved: bool,
    },
    BodyFailed {
        request_id: BodyRequestId,
        generation: u64,
        message_id: MessageId,
        failure: BodyFailure,
    },
    CacheCleared {
        operation_id: CacheOperationId,
        generation: u64,
        reclaimed_bytes: u64,
        usage: CacheUsage,
    },
    CacheClearFailed {
        operation_id: CacheOperationId,
        generation: u64,
    },
    CacheUsageChanged {
        usage: CacheUsage,
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
            Self::BodyLoaded {
                request_id,
                generation,
                saved,
                ..
            } => f
                .debug_struct("BodyLoaded")
                .field("request_id", request_id)
                .field("generation", generation)
                .field("saved", saved)
                .finish(),
            Self::BodyFailed {
                request_id,
                generation,
                failure,
                ..
            } => f
                .debug_struct("BodyFailed")
                .field("request_id", request_id)
                .field("generation", generation)
                .field("failure", failure)
                .finish(),
            Self::CacheCleared {
                operation_id,
                generation,
                reclaimed_bytes,
                ..
            } => f
                .debug_struct("CacheCleared")
                .field("operation_id", operation_id)
                .field("generation", generation)
                .field("reclaimed_bytes", reclaimed_bytes)
                .finish(),
            Self::CacheClearFailed {
                operation_id,
                generation,
            } => f
                .debug_struct("CacheClearFailed")
                .field("operation_id", operation_id)
                .field("generation", generation)
                .finish(),
            Self::CacheUsageChanged { usage } => f
                .debug_struct("CacheUsageChanged")
                .field("body_bytes", &usage.body_bytes)
                .field("body_count", &usage.body_count)
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
    let auth = Arc::new(Mutex::new(None::<RuntimeAuth>));
    let cache_io = Arc::new(Mutex::new(()));
    let cache_generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut body_task: Option<JoinHandle<()>> = None;
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
            if let Some(task) = body_task.take() {
                task.abort();
                let _ = task.await;
            }
            break;
        };
        if let WorkerCommand::SetCacheLimit { limit } = &command {
            let limit = *limit;
            let result = cache_blocking(cache_io.clone(), move || {
                let _ = cache::save_limit_and_prune(limit);
                cache::usage()
            })
            .await;
            if let Ok(Ok(usage)) = result {
                let _ = events.send(WorkerEvent::CacheUsageChanged { usage });
            }
            continue;
        }
        if let WorkerCommand::FetchBody {
            request_id,
            generation,
            account_email,
            message_id,
        } = &command
        {
            if *generation != cache_generation.load(Ordering::Acquire) {
                continue;
            }
            if let Some(task) = body_task.take() {
                task.abort();
                let _ = task.await;
            }
            let tx = events.clone();
            let request_id = *request_id;
            let generation = *generation;
            let account_email = account_email.clone();
            let message_id = message_id.clone();
            let failure_message_id = message_id.clone();
            let auth = auth.clone();
            let cache_generation = cache_generation.clone();
            let body_cache_generation = cache_generation.clone();
            let cache_io = cache_io.clone();
            body_task = Some(tokio::spawn(async move {
                let result = std::panic::AssertUnwindSafe(fetch_body(
                    request_id,
                    generation,
                    account_email,
                    message_id,
                    &tx,
                    &auth,
                    BodyCacheServices {
                        generation: body_cache_generation,
                        io: cache_io,
                    },
                ))
                .catch_unwind()
                .await;
                if result.is_err() && cache_generation.load(Ordering::Acquire) == generation {
                    send_body_failure(
                        &tx,
                        request_id,
                        generation,
                        failure_message_id,
                        BodyFailure::Protocol,
                    );
                }
            }));
            continue;
        }
        if let WorkerCommand::ClearBodyCache {
            operation_id,
            generation,
        } = &command
        {
            cache_generation.store(*generation, Ordering::Release);
            if let Some(task) = body_task.take() {
                task.abort();
                let _ = task.await;
            }
            let cleared = cache_blocking(cache_io.clone(), || {
                let reclaimed = cache::clear_bodies()?;
                let usage = cache::usage().unwrap_or_default();
                Ok::<_, std::io::Error>((reclaimed, usage))
            })
            .await;
            match cleared {
                Ok(Ok((reclaimed_bytes, usage))) => {
                    let _ = events.send(WorkerEvent::CacheCleared {
                        operation_id: *operation_id,
                        generation: *generation,
                        reclaimed_bytes,
                        usage,
                    });
                }
                Ok(Err(_)) | Err(_) => {
                    let _ = events.send(WorkerEvent::CacheClearFailed {
                        operation_id: *operation_id,
                        generation: *generation,
                    });
                }
            }
            continue;
        }
        if let WorkerCommand::Disconnect { generation, .. } = &command {
            cache_generation.store(*generation, Ordering::Release);
            if let Some(task) = body_task.take() {
                task.abort();
                let _ = task.await;
            }
            auth.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
        }
        if matches!(command, WorkerCommand::Shutdown) {
            cache_generation.fetch_add(1, Ordering::AcqRel);
            if let Some(task) = body_task.take() {
                task.abort();
                let _ = task.await;
            }
            auth.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
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
            WorkerCommand::Disconnect { id, .. } => {
                let tx = events.clone();
                let cache_io = cache_io.clone();
                tokio::spawn(async move {
                    emit_phase(&tx, id, WorkerPhase::Disconnecting);
                    match timeout(KEYRING_TIMEOUT, secrets::delete()).await {
                        Ok(Ok(())) => {
                            if matches!(
                                cache_blocking(cache_io, cache::clear_mailbox).await,
                                Ok(Ok(()))
                            ) {
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
                let auth = auth.clone();
                let cache_io = cache_io.clone();
                tokio::spawn(async move {
                    let result = connect(id, &tx, &cleanup_task, &auth, &cache_io).await;
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
                let auth = auth.clone();
                let cache_io = cache_io.clone();
                tokio::spawn(async move {
                    let result =
                        restore_or_refresh(id, &tx, true, &cleanup_task, &auth, &cache_io).await;
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
                let auth = auth.clone();
                let cache_io = cache_io.clone();
                tokio::spawn(async move {
                    let result =
                        restore_or_refresh(id, &tx, false, &cleanup_task, &auth, &cache_io).await;
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
            | WorkerCommand::FetchBody { .. }
            | WorkerCommand::ClearBodyCache { .. }
            | WorkerCommand::Shutdown => unreachable!(),
        };
        active = Some(Active { id, task, cleanup });
    }
}

struct RuntimeAuth {
    email: String,
    access_token: Zeroizing<String>,
}

struct BodyCacheServices {
    generation: Arc<std::sync::atomic::AtomicU64>,
    io: Arc<Mutex<()>>,
}

async fn cache_blocking<T, F>(
    cache_io: Arc<Mutex<()>>,
    operation: F,
) -> Result<T, tokio::task::JoinError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _guard = cache_io
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        operation()
    })
    .await
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
        | WorkerCommand::Disconnect { id, .. }
        | WorkerCommand::Cancel { id } => Some(*id),
        WorkerCommand::SetCacheLimit { .. }
        | WorkerCommand::FetchBody { .. }
        | WorkerCommand::ClearBodyCache { .. }
        | WorkerCommand::Shutdown => None,
    }
}

async fn connect(
    id: OperationId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    cleanup_required: &AtomicBool,
    auth: &Arc<Mutex<Option<RuntimeAuth>>>,
    cache_io: &Arc<Mutex<()>>,
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
    *auth.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(RuntimeAuth {
        email: account.email.clone(),
        access_token: Zeroizing::new(grant.access_token.expose().to_owned()),
    });
    let refresh = oauth::require_initial_refresh_token(&grant)
        .map_err(|_| failure(FailureKind::AuthorizationExpired, false, false))?;
    let token = RefreshToken::new(refresh.expose().to_owned())
        .map_err(|_| failure(FailureKind::CredentialSaveFailed, true, false))?;
    let requested_limit = cache_blocking(cache_io.clone(), cache::load_limit)
        .await
        .map_err(|_| failure(FailureKind::WorkerUnavailable, false, false))?;
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
            gmail::fetch_inbox(&account.email, grant.access_token.expose(), requested_limit)
                .await
                .map_err(map_gmail)
        },
    )
    .await?;
    complete_sync(id, tx, account, fetched, cache_io).await
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
    auth: &Arc<Mutex<Option<RuntimeAuth>>>,
    cache_io: &Arc<Mutex<()>>,
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
    let cached = cache_blocking(cache_io.clone(), || {
        let latest = cache::load_latest(cache::load_limit());
        let usage = cache::usage();
        (latest, usage)
    })
    .await;
    if let Ok((Ok(Some((account, snapshot))), usage)) = cached {
        let _ = tx.send(WorkerEvent::CacheLoaded {
            id,
            account,
            snapshot,
        });
        if let Ok(usage) = usage {
            let _ = tx.send(WorkerEvent::CacheUsageChanged { usage });
        }
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
    *auth.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(RuntimeAuth {
        email: account.email.clone(),
        access_token: Zeroizing::new(grant.access_token.expose().to_owned()),
    });
    sync(id, tx, account, grant.access_token.expose(), cache_io).await
}

async fn sync(
    id: OperationId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    account: AccountIdentity,
    access_token: &str,
    cache_io: &Arc<Mutex<()>>,
) -> Result<(), ServiceFailure> {
    emit_phase(tx, id, WorkerPhase::ConnectingImap);
    let requested_limit = cache_blocking(cache_io.clone(), cache::load_limit)
        .await
        .map_err(|_| failure(FailureKind::WorkerUnavailable, false, true))?;
    let fetched = gmail::fetch_inbox(&account.email, access_token, requested_limit)
        .await
        .map_err(map_gmail)?;
    complete_sync(id, tx, account, fetched, cache_io).await
}

async fn complete_sync(
    id: OperationId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    account: AccountIdentity,
    fetched: gmail::InboxFetch,
    cache_io: &Arc<Mutex<()>>,
) -> Result<(), ServiceFailure> {
    emit_phase(tx, id, WorkerPhase::FetchingInbox);
    let account_for_cache = account.clone();
    let result = cache_blocking(cache_io.clone(), move || {
        let (messages, fallback_count) = message::map_summaries(fetched.records);
        let requested_limit = cache::load_limit();
        let fresh = MailboxSnapshot {
            metadata: SyncMetadata {
                completed_at: SystemTime::now(),
                requested_limit,
                loaded_count: messages.len(),
                fallback_count,
                skipped_count: fetched.skipped_count,
            },
            messages,
        };
        let snapshot = cache::replace_and_save(&account_for_cache, fresh.clone(), requested_limit)
            .unwrap_or(fresh);
        let usage = cache::usage();
        (snapshot, usage)
    })
    .await
    .map_err(|_| failure(FailureKind::WorkerUnavailable, false, true))?;
    let (snapshot, usage) = result;
    let _ = tx.send(WorkerEvent::SyncComplete {
        id,
        account,
        snapshot,
    });
    if let Ok(usage) = usage {
        let _ = tx.send(WorkerEvent::CacheUsageChanged { usage });
    }
    Ok(())
}

async fn fetch_body(
    request_id: BodyRequestId,
    generation: u64,
    account_email: String,
    message_id: MessageId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    auth: &Arc<Mutex<Option<RuntimeAuth>>>,
    cache: BodyCacheServices,
) {
    let cache_generation = cache.generation;
    let cache_io = cache.io;
    let load_account = account_email.clone();
    let load_id = message_id.clone();
    let loaded = cache_blocking(cache_io.clone(), move || {
        let body = cache::load_body(&load_account, &load_id);
        let usage = body
            .as_ref()
            .ok()
            .and_then(|body| body.as_ref())
            .map(|_| cache::usage().unwrap_or_default());
        (body, usage)
    })
    .await;
    if let Ok((Ok(Some(body)), usage)) = loaded {
        if cache_generation.load(Ordering::Acquire) == generation {
            let _ = tx.send(WorkerEvent::BodyLoaded {
                request_id,
                generation,
                message_id,
                body: Arc::new(body),
                usage: usage.unwrap_or_default(),
                saved: true,
            });
        }
        return;
    }
    let Some((uid_validity, uid)) = message_id.gmail_parts() else {
        send_body_failure(
            tx,
            request_id,
            generation,
            message_id,
            BodyFailure::Protocol,
        );
        return;
    };
    let credentials = auth
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|value| {
            (
                value.email.clone(),
                Zeroizing::new(value.access_token.as_str().to_owned()),
            )
        });
    let Some((email, access_token)) = credentials else {
        send_body_failure(
            tx,
            request_id,
            generation,
            message_id,
            BodyFailure::AuthorizationRequired,
        );
        return;
    };
    if !email.eq_ignore_ascii_case(&account_email) {
        send_body_failure(
            tx,
            request_id,
            generation,
            message_id,
            BodyFailure::AuthorizationRequired,
        );
        return;
    }
    let raw = match gmail::fetch_body(&email, access_token.as_str(), uid_validity, uid).await {
        Ok(raw) => raw,
        Err(error) => {
            send_body_failure(
                tx,
                request_id,
                generation,
                message_id,
                map_body_failure(error),
            );
            return;
        }
    };
    if cache_generation.load(Ordering::Acquire) != generation {
        return;
    }
    let body = message::map_body(raw);
    let save_account = account_email.clone();
    let save_id = message_id.clone();
    let save_generation = cache_generation.clone();
    let saved = cache_blocking(cache_io, move || {
        if save_generation.load(Ordering::Acquire) != generation {
            return None;
        }
        let saved_usage = cache::save_body(&save_account, &save_id, &body);
        let (usage, saved) = match saved_usage {
            Ok(usage) => (usage, true),
            Err(_) => (cache::usage().unwrap_or_default(), false),
        };
        Some((body, usage, saved))
    })
    .await;
    if cache_generation.load(Ordering::Acquire) != generation {
        return;
    }
    let Ok(Some((body, usage, saved))) = saved else {
        return;
    };
    let _ = tx.send(WorkerEvent::BodyLoaded {
        request_id,
        generation,
        message_id,
        body: Arc::new(body),
        usage,
        saved,
    });
}

fn send_body_failure(
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    request_id: BodyRequestId,
    generation: u64,
    message_id: MessageId,
    failure: BodyFailure,
) {
    let _ = tx.send(WorkerEvent::BodyFailed {
        request_id,
        generation,
        message_id,
        failure,
    });
}

fn map_body_failure(error: gmail::GmailError) -> BodyFailure {
    match error {
        gmail::GmailError::Offline | gmail::GmailError::TlsFailed => BodyFailure::Offline,
        gmail::GmailError::TimedOut => BodyFailure::TimedOut,
        gmail::GmailError::AuthenticationFailed => BodyFailure::AuthorizationRequired,
        gmail::GmailError::MailboxChanged => BodyFailure::MailboxChanged,
        gmail::GmailError::MessageMissing => BodyFailure::Missing,
        gmail::GmailError::InboxUnavailable | gmail::GmailError::Protocol => BodyFailure::Protocol,
    }
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
        gmail::GmailError::MailboxChanged | gmail::GmailError::MessageMissing => {
            FailureKind::ImapProtocol
        }
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
        let body_command = WorkerCommand::FetchBody {
            request_id: BodyRequestId(4),
            generation: 1,
            account_email: "canary@example.com".into(),
            message_id: MessageId::gmail(7, 9),
        };
        assert!(!format!("{body_command:?}").contains("canary@example.com"));
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
