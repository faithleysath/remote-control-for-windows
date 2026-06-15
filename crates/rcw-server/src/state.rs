use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

use rcw_common::protocol::{
    CommandCompletePayload, CommandStatusResultPayload, CommandTaskStatus, ErrorCode, ErrorPayload,
    TunnelDirection, TunnelEndpointSide, TunnelInfo, TunnelStatus, MAX_CAPTURED_OUTPUT_BYTES,
};
use tokio::sync::Mutex;

use crate::ws::Tx;

pub(crate) const SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
pub(crate) const PENDING_OPEN_TTL: Duration = Duration::from_secs(60);
pub(crate) const REQUEST_ROUTE_TTL: Duration = Duration::from_secs(2 * 60 * 60);
pub(crate) const COMMAND_JOB_TTL: Duration = Duration::from_secs(30 * 60);
pub(crate) const RATE_LIMIT_KEY_TTL: Duration = Duration::from_secs(5 * 60);
pub(crate) const TUNNEL_IDLE_SWEEP_GRACE: Duration = Duration::from_secs(5);
const MAX_TUNNELS_PER_SESSION: usize = 16;
const MAX_STREAMS_PER_TUNNEL: usize = 64;

pub(crate) struct ServerState {
    hosts: Mutex<HashMap<String, HostConn>>,
    machine_index: Mutex<HashMap<String, HashSet<String>>>,
    sessions: Mutex<HashMap<String, SessionState>>,
    pending_open: Mutex<HashMap<String, PendingOpen>>,
    request_routes: Mutex<HashMap<String, RequestRoute>>,
    command_jobs: Mutex<HashMap<String, CommandJob>>,
    tunnels: Mutex<HashMap<String, TunnelState>>,
    tunnel_streams: Mutex<HashMap<String, TunnelStreamRoute>>,
    host_registrations: Mutex<HashMap<String, VecDeque<Instant>>>,
    auth_attempts: Mutex<HashMap<String, VecDeque<Instant>>>,
}

#[derive(Clone)]
pub(crate) struct HostConn {
    pub(crate) host_id: String,
    pub(crate) machine_id: String,
    pub(crate) connection_id: String,
    pub(crate) tx: Tx,
    pub(crate) totp_period_seconds: u64,
}

#[derive(Clone)]
pub(crate) struct SessionState {
    pub(crate) session_id: String,
    pub(crate) session_token: String,
    pub(crate) host_id: String,
    pub(crate) machine_id: String,
    pub(crate) connection_id: String,
    pub(crate) controller_tx: Option<Tx>,
    pub(crate) last_seen: Instant,
}

#[derive(Clone)]
pub(crate) struct PendingOpen {
    pub(crate) host_id: String,
    pub(crate) machine_id: String,
    pub(crate) connection_id: String,
    pub(crate) controller_tx: Tx,
    pub(crate) controller_label: String,
    pub(crate) force_reconnect: bool,
    pub(crate) created_at: Instant,
}

#[derive(Clone)]
pub(crate) struct RequestRoute {
    pub(crate) session_id: String,
    pub(crate) host_id: String,
    pub(crate) connection_id: String,
    pub(crate) controller_tx: Tx,
    pub(crate) detached: bool,
    pub(crate) created_at: Instant,
}

pub(crate) struct CommandJob {
    snapshot: CommandStatusResultPayload,
    host_id: String,
    connection_id: String,
    session_id: String,
    session_token: String,
    created_at: Instant,
    finished_at: Option<Instant>,
}

#[derive(Clone)]
pub(crate) struct TunnelState {
    pub(crate) info: TunnelInfo,
    pub(crate) host_id: String,
    pub(crate) connection_id: String,
    pub(crate) target_side: TunnelEndpointSide,
    pub(crate) last_activity: Instant,
}

#[derive(Clone)]
pub(crate) struct TunnelStreamRoute {
    pub(crate) tunnel_id: String,
    pub(crate) session_id: String,
    pub(crate) host_id: String,
    pub(crate) connection_id: String,
    pub(crate) last_activity: Instant,
    pub(crate) controller_eof: bool,
    pub(crate) host_eof: bool,
}

pub(crate) struct CreateTunnel {
    pub(crate) tunnel_id: String,
    pub(crate) session: SessionState,
    pub(crate) direction: TunnelDirection,
    pub(crate) listen_addr: String,
    pub(crate) listen_port: u16,
    pub(crate) target_host: String,
    pub(crate) target_port: u16,
    pub(crate) idle_timeout_ms: u64,
}

pub(crate) struct CreateExecJob {
    pub(crate) task_id: String,
    pub(crate) host_id: String,
    pub(crate) connection_id: String,
    pub(crate) session_id: String,
    pub(crate) session_token: String,
    pub(crate) started_at: String,
}

impl ServerState {
    pub(crate) fn new() -> Self {
        Self {
            hosts: Mutex::new(HashMap::new()),
            machine_index: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            pending_open: Mutex::new(HashMap::new()),
            request_routes: Mutex::new(HashMap::new()),
            command_jobs: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(HashMap::new()),
            tunnel_streams: Mutex::new(HashMap::new()),
            host_registrations: Mutex::new(HashMap::new()),
            auth_attempts: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn register_host(
        &self,
        host_id: String,
        machine_id: String,
        connection_id: String,
        tx: Tx,
        totp_period_seconds: u64,
    ) -> Option<HostConn> {
        let old_machine_id = {
            let mut hosts = self.hosts.lock().await;
            hosts
                .insert(
                    host_id.clone(),
                    HostConn {
                        host_id: host_id.clone(),
                        machine_id: machine_id.clone(),
                        connection_id,
                        tx,
                        totp_period_seconds,
                    },
                )
                .map(|old| (old.machine_id.clone(), old))
        };

        let mut index = self.machine_index.lock().await;
        let old_host = if let Some((old_machine_id, old_host)) = old_machine_id {
            if old_machine_id != machine_id {
                remove_host_from_machine_index(&mut index, &old_machine_id, &host_id);
            }
            Some(old_host)
        } else {
            None
        };
        index.entry(machine_id).or_default().insert(host_id);
        old_host
    }

    pub(crate) async fn host(&self, host_id: &str) -> Option<HostConn> {
        let hosts = self.hosts.lock().await;
        hosts.get(host_id).cloned()
    }

    pub(crate) async fn host_for_machine_id(&self, machine_id: &str) -> HostLookup {
        let host_ids = {
            let index = self.machine_index.lock().await;
            index
                .get(machine_id)
                .map(|ids| ids.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };
        if host_ids.is_empty() {
            return HostLookup::NotFound;
        }

        let hosts = self.hosts.lock().await;
        let matches = host_ids
            .iter()
            .filter_map(|host_id| hosts.get(host_id).cloned())
            .collect::<Vec<_>>();
        match matches.len() {
            0 => HostLookup::NotFound,
            1 => HostLookup::Found(matches[0].clone()),
            _ => HostLookup::Ambiguous(matches),
        }
    }

    pub(crate) async fn host_tx(&self, host_id: &str, connection_id: &str) -> Option<Tx> {
        self.host(host_id)
            .await
            .filter(|host| host.connection_id == connection_id)
            .map(|host| host.tx)
    }

    pub(crate) async fn host_has_active_session(&self, host_id: &str) -> bool {
        let sessions = self.sessions.lock().await;
        sessions.values().any(|session| session.host_id == host_id)
    }

    pub(crate) async fn insert_pending_open(&self, request_id: String, pending: PendingOpen) {
        let mut pending_open = self.pending_open.lock().await;
        pending_open.insert(request_id, pending);
    }

    pub(crate) async fn take_pending_open(&self, request_id: &str) -> Option<PendingOpen> {
        let mut pending_open = self.pending_open.lock().await;
        pending_open.remove(request_id)
    }

    pub(crate) async fn remove_pending_open(&self, request_id: &str) {
        let mut pending_open = self.pending_open.lock().await;
        pending_open.remove(request_id);
    }

    pub(crate) async fn remove_pending_open_for_host(
        &self,
        host_id: &str,
        connection_id: Option<&str>,
    ) -> Vec<(String, PendingOpen)> {
        let mut pending_open = self.pending_open.lock().await;
        let request_ids = pending_open
            .iter()
            .filter(|(_, pending)| pending.host_id == host_id)
            .filter(|(_, pending)| {
                connection_id
                    .map(|connection_id| pending.connection_id == connection_id)
                    .unwrap_or(true)
            })
            .map(|(request_id, _)| request_id.clone())
            .collect::<Vec<_>>();
        request_ids
            .into_iter()
            .filter_map(|request_id| {
                pending_open
                    .remove(&request_id)
                    .map(|pending| (request_id, pending))
            })
            .collect()
    }

    pub(crate) async fn create_session(
        &self,
        session_id: String,
        session_token: String,
        host_id: String,
        machine_id: String,
        connection_id: String,
        controller_tx: Tx,
    ) {
        let mut sessions = self.sessions.lock().await;
        sessions.insert(
            session_id.clone(),
            SessionState {
                session_id,
                session_token,
                host_id,
                machine_id,
                connection_id,
                controller_tx: Some(controller_tx),
                last_seen: Instant::now(),
            },
        );
    }

    pub(crate) async fn session_if_valid(
        &self,
        session_id: &str,
        session_token: &str,
    ) -> Option<SessionState> {
        let mut sessions = self.sessions.lock().await;
        match sessions.get_mut(session_id) {
            Some(session) if session.session_token == session_token => {
                session.last_seen = Instant::now();
                Some(session.clone())
            }
            _ => None,
        }
    }

    pub(crate) async fn remove_session_if_valid(
        &self,
        session_id: &str,
        session_token: &str,
    ) -> Option<SessionState> {
        let removed = {
            let mut sessions = self.sessions.lock().await;
            match sessions.get(session_id) {
                Some(session) if session.session_token == session_token => {
                    sessions.remove(session_id)
                }
                _ => None,
            }
        };
        if let Some(session) = &removed {
            let session_ids = HashSet::from([session.session_id.clone()]);
            self.remove_routes_for_sessions(&session_ids).await;
            self.remove_tunnels_for_sessions(&session_ids, "session_closed")
                .await;
            self.fail_running_exec_jobs_for_sessions(
                &session_ids,
                ErrorPayload {
                    code: ErrorCode::Cancelled,
                    message: "session closed by controller".to_owned(),
                },
            )
            .await;
        }
        removed
    }

    pub(crate) async fn remove_session_for_host(
        &self,
        session_id: &str,
        host_id: &str,
        connection_id: &str,
        error: ErrorPayload,
    ) -> Option<SessionState> {
        let removed = {
            let mut sessions = self.sessions.lock().await;
            match sessions.get(session_id) {
                Some(session)
                    if session.host_id == host_id && session.connection_id == connection_id =>
                {
                    sessions.remove(session_id)
                }
                _ => None,
            }
        };
        if let Some(session) = &removed {
            let session_ids = HashSet::from([session.session_id.clone()]);
            self.remove_routes_for_sessions(&session_ids).await;
            self.remove_tunnels_for_sessions(&session_ids, "session_closed")
                .await;
            self.fail_running_exec_jobs_for_sessions(&session_ids, error)
                .await;
        }
        removed
    }

    pub(crate) async fn bind_controller_to_session(
        &self,
        session_id: &str,
        session_token: &str,
        controller_tx: Tx,
    ) -> Option<SessionState> {
        let mut sessions = self.sessions.lock().await;
        match sessions.get_mut(session_id) {
            Some(session) if session.session_token == session_token => {
                session.controller_tx = Some(controller_tx);
                session.last_seen = Instant::now();
                Some(session.clone())
            }
            _ => None,
        }
    }

    pub(crate) async fn track_request_route(
        &self,
        request_id: String,
        session: &SessionState,
        tx: Tx,
    ) {
        self.track_request_route_with_mode(request_id, session, tx, false)
            .await;
    }

    pub(crate) async fn track_detached_request_route(
        &self,
        request_id: String,
        session: &SessionState,
        tx: Tx,
    ) {
        self.track_request_route_with_mode(request_id, session, tx, true)
            .await;
    }

    async fn track_request_route_with_mode(
        &self,
        request_id: String,
        session: &SessionState,
        tx: Tx,
        detached: bool,
    ) {
        let mut routes = self.request_routes.lock().await;
        routes.insert(
            request_id,
            RequestRoute {
                session_id: session.session_id.clone(),
                host_id: session.host_id.clone(),
                connection_id: session.connection_id.clone(),
                controller_tx: tx,
                detached,
                created_at: Instant::now(),
            },
        );
    }

    pub(crate) async fn clear_request_route(&self, request_id: &str) {
        let mut routes = self.request_routes.lock().await;
        routes.remove(request_id);
    }

    pub(crate) async fn clear_request_routes_for_controller(&self, tx: &Tx) {
        let mut routes = self.request_routes.lock().await;
        routes.retain(|_, route| route.detached || !route.controller_tx.same_channel(tx));
    }

    pub(crate) async fn request_route(&self, request_id: &str) -> Option<RequestRoute> {
        let routes = self.request_routes.lock().await;
        routes.get(request_id).cloned()
    }

    pub(crate) async fn request_route_for_host(
        &self,
        request_id: &str,
        host_id: &str,
        connection_id: &str,
        session_id: Option<&str>,
    ) -> Option<RequestRoute> {
        let routes = self.request_routes.lock().await;
        routes
            .get(request_id)
            .filter(|route| route.host_id == host_id)
            .filter(|route| route.connection_id == connection_id)
            .filter(|route| {
                session_id
                    .map(|session_id| route.session_id == session_id)
                    .unwrap_or(true)
            })
            .cloned()
    }

    pub(crate) async fn request_session_id(&self, request_id: &str) -> Option<String> {
        self.request_route(request_id)
            .await
            .map(|route| route.session_id)
    }

    pub(crate) async fn session_controller_for_machine(
        &self,
        session_id: &str,
        host_id: &str,
        connection_id: &str,
    ) -> Option<Tx> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(session_id)
            .filter(|session| session.host_id == host_id)
            .filter(|session| session.connection_id == connection_id)
            .and_then(|session| session.controller_tx.clone())
    }

    pub(crate) async fn command_route(&self, session_id: &str) -> Option<SessionState> {
        let sessions = self.sessions.lock().await;
        sessions.get(session_id).cloned()
    }

    pub(crate) async fn touch_session(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.last_seen = Instant::now();
        }
    }

    pub(crate) async fn unregister_host(
        &self,
        host_id: &str,
        connection_id: &str,
    ) -> Vec<SessionState> {
        let removed = {
            let mut hosts = self.hosts.lock().await;
            match hosts.get(host_id) {
                Some(host) if host.connection_id == connection_id => hosts.remove(host_id),
                _ => None,
            }
        };

        let Some(removed_host) = removed else {
            return Vec::new();
        };

        {
            let mut index = self.machine_index.lock().await;
            remove_host_from_machine_index(&mut index, &removed_host.machine_id, host_id);
        }

        self.remove_sessions_for_host(
            host_id,
            Some(connection_id),
            ErrorPayload {
                code: ErrorCode::HostDisconnected,
                message: ErrorCode::HostDisconnected.message().to_owned(),
            },
        )
        .await
    }

    pub(crate) async fn remove_sessions_for_host(
        &self,
        host_id: &str,
        connection_id: Option<&str>,
        error: ErrorPayload,
    ) -> Vec<SessionState> {
        let removed_sessions = {
            let mut sessions = self.sessions.lock().await;
            let session_ids = sessions
                .values()
                .filter(|session| session.host_id == host_id)
                .filter(|session| {
                    connection_id
                        .map(|connection_id| session.connection_id == connection_id)
                        .unwrap_or(true)
                })
                .map(|session| session.session_id.clone())
                .collect::<Vec<_>>();
            session_ids
                .into_iter()
                .filter_map(|session_id| sessions.remove(&session_id))
                .collect::<Vec<_>>()
        };
        let removed_session_ids = removed_sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect::<HashSet<_>>();
        if !removed_session_ids.is_empty() {
            self.remove_routes_for_sessions(&removed_session_ids).await;
            self.remove_tunnels_for_sessions(&removed_session_ids, "host_disconnected")
                .await;
            self.fail_running_exec_jobs_for_sessions(&removed_session_ids, error)
                .await;
        }

        removed_sessions
    }

    pub(crate) async fn allow_host_registration(&self, host_id: &str) -> bool {
        allow_rate_limit(
            &self.host_registrations,
            host_id,
            20,
            Duration::from_secs(60),
        )
        .await
    }

    pub(crate) async fn allow_auth_attempt(&self, machine_id: &str) -> bool {
        allow_rate_limit(&self.auth_attempts, machine_id, 12, Duration::from_secs(60)).await
    }

    pub(crate) async fn prune_stale(&self, now: Instant) -> Vec<SessionState> {
        let active_request_session_ids = {
            let routes = self.request_routes.lock().await;
            routes
                .values()
                .map(|route| route.session_id.clone())
                .collect::<HashSet<_>>()
        };

        let removed_sessions = {
            let mut sessions = self.sessions.lock().await;
            let session_ids = sessions
                .values()
                .filter(|session| {
                    now.duration_since(session.last_seen) > SESSION_IDLE_TTL
                        && !active_request_session_ids.contains(&session.session_id)
                })
                .map(|session| session.session_id.clone())
                .collect::<Vec<_>>();
            session_ids
                .into_iter()
                .filter_map(|session_id| sessions.remove(&session_id))
                .collect::<Vec<_>>()
        };

        let removed_session_ids = removed_sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect::<HashSet<_>>();
        if !removed_session_ids.is_empty() {
            self.remove_tunnels_for_sessions(&removed_session_ids, "session_idle_timeout")
                .await;
            self.fail_running_exec_jobs_for_sessions(
                &removed_session_ids,
                ErrorPayload {
                    code: ErrorCode::SessionExpired,
                    message: "session idle timeout".to_owned(),
                },
            )
            .await;
        }

        {
            let mut pending_open = self.pending_open.lock().await;
            pending_open
                .retain(|_, pending| now.duration_since(pending.created_at) <= PENDING_OPEN_TTL);
        }

        {
            let mut routes = self.request_routes.lock().await;
            routes.retain(|_, route| {
                now.duration_since(route.created_at) <= REQUEST_ROUTE_TTL
                    && !removed_session_ids.contains(&route.session_id)
            });
        }

        {
            let idle_tunnel_ids = {
                let tunnels = self.tunnels.lock().await;
                tunnels
                    .values()
                    .filter(|tunnel| {
                        let idle_timeout = Duration::from_millis(tunnel.info.idle_timeout_ms)
                            .saturating_add(TUNNEL_IDLE_SWEEP_GRACE);
                        now.duration_since(tunnel.last_activity) > idle_timeout
                    })
                    .map(|tunnel| tunnel.info.tunnel_id.clone())
                    .collect::<Vec<_>>()
            };
            if !idle_tunnel_ids.is_empty() {
                self.close_tunnels_by_ids(&idle_tunnel_ids, "idle_timeout")
                    .await;
            }
        }

        {
            let mut jobs = self.command_jobs.lock().await;
            jobs.retain(|_, job| {
                if let Some(finished_at) = job.finished_at {
                    return now.duration_since(finished_at) <= COMMAND_JOB_TTL;
                }
                if now.duration_since(job.created_at) <= REQUEST_ROUTE_TTL {
                    return true;
                }
                job.snapshot.status = CommandTaskStatus::Failed;
                job.snapshot.finished_at = Some(rcw_common::audit::now_rfc3339());
                job.snapshot.error = Some(ErrorPayload {
                    code: ErrorCode::RequestTimeout,
                    message: "exec job route expired".to_owned(),
                });
                job.finished_at = Some(now);
                true
            });
        }

        prune_rate_limit_table(&self.host_registrations, now, RATE_LIMIT_KEY_TTL).await;
        prune_rate_limit_table(&self.auth_attempts, now, RATE_LIMIT_KEY_TTL).await;

        removed_sessions
    }

    pub(crate) async fn create_exec_job(&self, job: CreateExecJob) -> CommandStatusResultPayload {
        let snapshot = CommandStatusResultPayload {
            task_id: job.task_id.clone(),
            status: CommandTaskStatus::Running,
            request_id: job.task_id.clone(),
            started_at: job.started_at,
            finished_at: None,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            complete: None,
            error: None,
        };
        let mut jobs = self.command_jobs.lock().await;
        jobs.insert(
            job.task_id,
            CommandJob {
                snapshot: snapshot.clone(),
                host_id: job.host_id,
                connection_id: job.connection_id,
                session_id: job.session_id,
                session_token: job.session_token,
                created_at: Instant::now(),
                finished_at: None,
            },
        );
        snapshot
    }

    pub(crate) async fn exec_job_if_valid(
        &self,
        task_id: &str,
        session_token: &str,
    ) -> Option<CommandStatusResultPayload> {
        let jobs = self.command_jobs.lock().await;
        jobs.get(task_id)
            .filter(|job| job.session_token == session_token)
            .map(|job| job.snapshot.clone())
    }

    pub(crate) async fn exec_job_route_if_valid(
        &self,
        task_id: &str,
        session_token: &str,
    ) -> Option<(String, String, String)> {
        let jobs = self.command_jobs.lock().await;
        jobs.get(task_id)
            .filter(|job| job.session_token == session_token)
            .filter(|job| job.snapshot.status == CommandTaskStatus::Running)
            .map(|job| {
                (
                    job.host_id.clone(),
                    job.connection_id.clone(),
                    job.session_id.clone(),
                )
            })
    }

    pub(crate) async fn append_exec_job_output(&self, task_id: &str, stream: &str, data: &str) {
        let mut jobs = self.command_jobs.lock().await;
        let Some(job) = jobs.get_mut(task_id) else {
            return;
        };
        if job.snapshot.status != CommandTaskStatus::Running {
            return;
        }
        match stream {
            "stdout" => append_limited_output(
                &mut job.snapshot.stdout,
                &mut job.snapshot.stdout_truncated,
                data,
            ),
            "stderr" => append_limited_output(
                &mut job.snapshot.stderr,
                &mut job.snapshot.stderr_truncated,
                data,
            ),
            _ => {}
        }
    }

    pub(crate) async fn finish_exec_job(&self, task_id: &str, complete: CommandCompletePayload) {
        let mut jobs = self.command_jobs.lock().await;
        let Some(job) = jobs.get_mut(task_id) else {
            return;
        };
        if job.snapshot.status != CommandTaskStatus::Running {
            return;
        }
        job.snapshot.status = CommandTaskStatus::Completed;
        job.snapshot.finished_at = Some(rcw_common::audit::now_rfc3339());
        job.snapshot.complete = Some(complete);
        job.finished_at = Some(Instant::now());
    }

    pub(crate) async fn fail_exec_job(&self, task_id: &str, error: ErrorPayload) {
        let mut jobs = self.command_jobs.lock().await;
        let Some(job) = jobs.get_mut(task_id) else {
            return;
        };
        if job.snapshot.status != CommandTaskStatus::Running {
            return;
        }
        job.snapshot.status = if error.code == rcw_common::protocol::ErrorCode::Cancelled {
            CommandTaskStatus::Cancelled
        } else {
            CommandTaskStatus::Failed
        };
        job.snapshot.finished_at = Some(rcw_common::audit::now_rfc3339());
        job.snapshot.error = Some(error);
        job.finished_at = Some(Instant::now());
    }

    async fn remove_routes_for_sessions(&self, session_ids: &HashSet<String>) {
        let mut routes = self.request_routes.lock().await;
        routes.retain(|_, route| !session_ids.contains(&route.session_id));
    }

    pub(crate) async fn create_tunnel(
        &self,
        create: CreateTunnel,
    ) -> Result<TunnelInfo, ErrorPayload> {
        let session_tunnel_count = {
            let tunnels = self.tunnels.lock().await;
            tunnels
                .values()
                .filter(|tunnel| tunnel.info.session_id == create.session.session_id)
                .count()
        };
        if session_tunnel_count >= MAX_TUNNELS_PER_SESSION {
            return Err(ErrorPayload {
                code: ErrorCode::PermissionDenied,
                message: "per-session tunnel limit exceeded".to_owned(),
            });
        }

        let now = Instant::now();
        let now_text = rcw_common::audit::now_rfc3339();
        let info = TunnelInfo {
            tunnel_id: create.tunnel_id.clone(),
            session_id: create.session.session_id.clone(),
            direction: create.direction,
            listen_addr: create.listen_addr,
            listen_port: create.listen_port,
            target_host: create.target_host,
            target_port: create.target_port,
            status: TunnelStatus::Opening,
            opened_at: now_text.clone(),
            last_activity_at: now_text,
            idle_timeout_ms: create.idle_timeout_ms,
            bytes_from_listener: 0,
            bytes_from_target: 0,
            active_streams: 0,
            total_streams: 0,
            close_reason: None,
        };
        let state = TunnelState {
            info: info.clone(),
            host_id: create.session.host_id,
            connection_id: create.session.connection_id,
            target_side: create.direction.target_endpoint_side(),
            last_activity: now,
        };
        let mut tunnels = self.tunnels.lock().await;
        tunnels.insert(create.tunnel_id, state);
        Ok(info)
    }

    pub(crate) async fn activate_tunnel(
        &self,
        tunnel_id: &str,
        listen_addr: Option<String>,
        listen_port: Option<u16>,
    ) -> Option<TunnelInfo> {
        let mut tunnels = self.tunnels.lock().await;
        let tunnel = tunnels.get_mut(tunnel_id)?;
        if let Some(listen_addr) = listen_addr {
            tunnel.info.listen_addr = listen_addr;
        }
        if let Some(listen_port) = listen_port {
            tunnel.info.listen_port = listen_port;
        }
        tunnel.info.status = TunnelStatus::Active;
        tunnel.info.last_activity_at = rcw_common::audit::now_rfc3339();
        tunnel.last_activity = Instant::now();
        Some(tunnel.info.clone())
    }

    pub(crate) async fn fail_tunnel(&self, tunnel_id: &str, reason: &str) -> Option<TunnelInfo> {
        let mut tunnels = self.tunnels.lock().await;
        let mut tunnel = tunnels.remove(tunnel_id)?;
        tunnel.info.status = TunnelStatus::Failed;
        tunnel.info.last_activity_at = rcw_common::audit::now_rfc3339();
        tunnel.info.close_reason = Some(reason.to_owned());
        drop(tunnels);
        let mut streams = self.tunnel_streams.lock().await;
        streams.retain(|_, route| route.tunnel_id != tunnel_id);
        Some(tunnel.info)
    }

    pub(crate) async fn tunnel_if_valid(
        &self,
        tunnel_id: &str,
        session_token: &str,
    ) -> Option<TunnelState> {
        let session_id = {
            let tunnels = self.tunnels.lock().await;
            tunnels
                .get(tunnel_id)
                .map(|tunnel| tunnel.info.session_id.clone())?
        };
        self.session_if_valid(&session_id, session_token).await?;
        let tunnels = self.tunnels.lock().await;
        tunnels.get(tunnel_id).cloned()
    }

    pub(crate) async fn tunnels_for_session_if_valid(
        &self,
        session_id: &str,
        session_token: &str,
        tunnel_id: Option<&str>,
    ) -> Option<Vec<TunnelInfo>> {
        self.session_if_valid(session_id, session_token).await?;
        let tunnels = self.tunnels.lock().await;
        let list = tunnels
            .values()
            .filter(|tunnel| tunnel.info.session_id == session_id)
            .filter(|tunnel| {
                tunnel_id
                    .map(|tunnel_id| tunnel.info.tunnel_id == tunnel_id)
                    .unwrap_or(true)
            })
            .map(|tunnel| tunnel.info.clone())
            .collect();
        Some(list)
    }

    pub(crate) async fn close_tunnel_if_valid(
        &self,
        tunnel_id: &str,
        session_token: &str,
        reason: &str,
    ) -> Option<TunnelInfo> {
        let tunnel = self.tunnel_if_valid(tunnel_id, session_token).await?;
        self.close_tunnels_by_ids(std::slice::from_ref(&tunnel.info.tunnel_id), reason)
            .await
            .into_iter()
            .next()
    }

    pub(crate) async fn add_tunnel_stream(
        &self,
        tunnel_id: &str,
        stream_id: String,
        source_side: TunnelEndpointSide,
    ) -> Result<TunnelStreamRoute, ErrorPayload> {
        let now = Instant::now();
        let mut tunnels = self.tunnels.lock().await;
        let Some(tunnel) = tunnels.get_mut(tunnel_id) else {
            return Err(ErrorPayload {
                code: ErrorCode::SessionExpired,
                message: "tunnel is not active".to_owned(),
            });
        };
        if tunnel.info.status != TunnelStatus::Active {
            return Err(ErrorPayload {
                code: ErrorCode::SessionExpired,
                message: "tunnel is not active".to_owned(),
            });
        }
        if tunnel.info.active_streams >= MAX_STREAMS_PER_TUNNEL {
            return Err(ErrorPayload {
                code: ErrorCode::PermissionDenied,
                message: "per-tunnel stream limit exceeded".to_owned(),
            });
        }
        if source_side == tunnel.target_side {
            return Err(ErrorPayload {
                code: ErrorCode::PermissionDenied,
                message: "tunnel stream opened from invalid side".to_owned(),
            });
        }
        tunnel.info.active_streams += 1;
        tunnel.info.total_streams += 1;
        tunnel.info.last_activity_at = rcw_common::audit::now_rfc3339();
        tunnel.last_activity = now;
        let route = TunnelStreamRoute {
            tunnel_id: tunnel.info.tunnel_id.clone(),
            session_id: tunnel.info.session_id.clone(),
            host_id: tunnel.host_id.clone(),
            connection_id: tunnel.connection_id.clone(),
            last_activity: now,
            controller_eof: false,
            host_eof: false,
        };
        drop(tunnels);

        let mut streams = self.tunnel_streams.lock().await;
        streams.insert(stream_id, route.clone());
        Ok(route)
    }

    pub(crate) async fn tunnel_stream(
        &self,
        tunnel_id: &str,
        stream_id: &str,
    ) -> Option<TunnelStreamRoute> {
        let streams = self.tunnel_streams.lock().await;
        streams
            .get(stream_id)
            .filter(|route| route.tunnel_id == tunnel_id)
            .cloned()
    }

    pub(crate) async fn close_tunnel_stream(
        &self,
        tunnel_id: &str,
        stream_id: &str,
    ) -> Option<TunnelStreamRoute> {
        let removed = {
            let mut streams = self.tunnel_streams.lock().await;
            match streams.get(stream_id) {
                Some(route) if route.tunnel_id == tunnel_id => streams.remove(stream_id),
                _ => None,
            }
        };
        if let Some(route) = &removed {
            let mut tunnels = self.tunnels.lock().await;
            if let Some(tunnel) = tunnels.get_mut(tunnel_id) {
                tunnel.info.active_streams = tunnel.info.active_streams.saturating_sub(1);
                tunnel.info.last_activity_at = rcw_common::audit::now_rfc3339();
                tunnel.last_activity = Instant::now();
            }
            self.touch_session(&route.session_id).await;
        }
        removed
    }

    pub(crate) async fn mark_tunnel_stream_eof(
        &self,
        tunnel_id: &str,
        stream_id: &str,
        side: TunnelEndpointSide,
    ) -> Option<(TunnelStreamRoute, bool)> {
        let (route, remove) = {
            let mut streams = self.tunnel_streams.lock().await;
            let route = streams
                .get_mut(stream_id)
                .filter(|route| route.tunnel_id == tunnel_id)?;
            match side {
                TunnelEndpointSide::Controller => route.controller_eof = true,
                TunnelEndpointSide::Host => route.host_eof = true,
            }
            route.last_activity = Instant::now();
            (route.clone(), route.controller_eof && route.host_eof)
        };
        if remove {
            self.close_tunnel_stream(tunnel_id, stream_id).await;
        } else {
            let mut tunnels = self.tunnels.lock().await;
            if let Some(tunnel) = tunnels.get_mut(tunnel_id) {
                tunnel.info.last_activity_at = rcw_common::audit::now_rfc3339();
                tunnel.last_activity = Instant::now();
            }
        }
        Some((route, remove))
    }

    pub(crate) async fn record_tunnel_bytes(
        &self,
        tunnel_id: &str,
        stream_id: &str,
        from_side: TunnelEndpointSide,
        bytes: usize,
    ) -> Option<TunnelStreamRoute> {
        let now = Instant::now();
        let route = {
            let mut streams = self.tunnel_streams.lock().await;
            let route = streams
                .get_mut(stream_id)
                .filter(|route| route.tunnel_id == tunnel_id)?;
            route.last_activity = now;
            route.clone()
        };
        {
            let mut tunnels = self.tunnels.lock().await;
            if let Some(tunnel) = tunnels.get_mut(tunnel_id) {
                if from_side == tunnel.info.direction.local_endpoint_side() {
                    tunnel.info.bytes_from_listener =
                        tunnel.info.bytes_from_listener.saturating_add(bytes as u64);
                } else {
                    tunnel.info.bytes_from_target =
                        tunnel.info.bytes_from_target.saturating_add(bytes as u64);
                }
                tunnel.info.last_activity_at = rcw_common::audit::now_rfc3339();
                tunnel.last_activity = now;
            }
        }
        self.touch_session(&route.session_id).await;
        Some(route)
    }

    async fn remove_tunnels_for_sessions(&self, session_ids: &HashSet<String>, reason: &str) {
        let tunnel_ids = {
            let tunnels = self.tunnels.lock().await;
            tunnels
                .values()
                .filter(|tunnel| session_ids.contains(&tunnel.info.session_id))
                .map(|tunnel| tunnel.info.tunnel_id.clone())
                .collect::<Vec<_>>()
        };
        if !tunnel_ids.is_empty() {
            self.close_tunnels_by_ids(&tunnel_ids, reason).await;
        }
    }

    async fn close_tunnels_by_ids(&self, tunnel_ids: &[String], reason: &str) -> Vec<TunnelInfo> {
        let tunnel_id_set = tunnel_ids.iter().cloned().collect::<HashSet<_>>();
        {
            let mut streams = self.tunnel_streams.lock().await;
            streams.retain(|_, route| !tunnel_id_set.contains(&route.tunnel_id));
        }
        let mut tunnels = self.tunnels.lock().await;
        tunnel_ids
            .iter()
            .filter_map(|tunnel_id| {
                tunnels.remove(tunnel_id).map(|mut tunnel| {
                    tunnel.info.status = TunnelStatus::Closed;
                    tunnel.info.active_streams = 0;
                    tunnel.info.last_activity_at = rcw_common::audit::now_rfc3339();
                    tunnel.info.close_reason = Some(reason.to_owned());
                    tunnel.info
                })
            })
            .collect()
    }

    async fn fail_running_exec_jobs_for_sessions(
        &self,
        session_ids: &HashSet<String>,
        error: ErrorPayload,
    ) {
        let mut jobs = self.command_jobs.lock().await;
        for job in jobs.values_mut() {
            if session_ids.contains(&job.session_id)
                && job.snapshot.status == CommandTaskStatus::Running
            {
                job.snapshot.status = if error.code == ErrorCode::Cancelled {
                    CommandTaskStatus::Cancelled
                } else {
                    CommandTaskStatus::Failed
                };
                job.snapshot.finished_at = Some(rcw_common::audit::now_rfc3339());
                job.snapshot.error = Some(error.clone());
                job.finished_at = Some(Instant::now());
            }
        }
    }
}

#[derive(Clone)]
pub(crate) enum HostLookup {
    NotFound,
    Found(HostConn),
    Ambiguous(Vec<HostConn>),
}

fn remove_host_from_machine_index(
    index: &mut HashMap<String, HashSet<String>>,
    machine_id: &str,
    host_id: &str,
) {
    if let Some(host_ids) = index.get_mut(machine_id) {
        host_ids.remove(host_id);
        if host_ids.is_empty() {
            index.remove(machine_id);
        }
    }
}

async fn allow_rate_limit(
    table: &Mutex<HashMap<String, VecDeque<Instant>>>,
    key: &str,
    max_events: usize,
    window: Duration,
) -> bool {
    let now = Instant::now();
    let mut table = table.lock().await;
    let events = table.entry(key.to_owned()).or_default();
    while events
        .front()
        .map(|instant| now.duration_since(*instant) > window)
        .unwrap_or(false)
    {
        events.pop_front();
    }
    if events.len() >= max_events {
        return false;
    }
    events.push_back(now);
    true
}

async fn prune_rate_limit_table(
    table: &Mutex<HashMap<String, VecDeque<Instant>>>,
    now: Instant,
    window: Duration,
) {
    let mut table = table.lock().await;
    table.retain(|_, events| {
        while events
            .front()
            .map(|instant| now.duration_since(*instant) > window)
            .unwrap_or(false)
        {
            events.pop_front();
        }
        !events.is_empty()
    });
}

fn append_limited_output(target: &mut String, truncated: &mut bool, chunk: &str) {
    if *truncated {
        return;
    }
    let remaining = MAX_CAPTURED_OUTPUT_BYTES.saturating_sub(target.len());
    if remaining == 0 {
        *truncated = true;
        return;
    }
    if chunk.len() <= remaining {
        target.push_str(chunk);
        return;
    }
    let cutoff = chunk
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= remaining)
        .last()
        .unwrap_or(0);
    target.push_str(&chunk[..cutoff]);
    *truncated = true;
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use rcw_common::protocol::ErrorPayload;
    use tokio::sync::mpsc;

    use super::*;
    use crate::ws::Outbound;

    fn tx() -> Tx {
        let (tx, _rx) = mpsc::channel::<Outbound>(1);
        tx
    }

    #[tokio::test]
    async fn stale_host_unregister_does_not_remove_current_connection() {
        let state = ServerState::new();
        state
            .register_host(
                "host-1".to_owned(),
                "ABCD-EFGH-IJKL".to_owned(),
                "conn-old".to_owned(),
                tx(),
                120,
            )
            .await;
        state
            .register_host(
                "host-1".to_owned(),
                "ABCD-EFGH-IJKL".to_owned(),
                "conn-new".to_owned(),
                tx(),
                120,
            )
            .await;

        let removed = state.unregister_host("host-1", "conn-old").await;

        assert!(removed.is_empty());
        let current = state.host("host-1").await.unwrap();
        assert_eq!(current.connection_id, "conn-new");
    }

    #[tokio::test]
    async fn remove_sessions_for_host_respects_connection_id() {
        let state = ServerState::new();
        state
            .create_session(
                "session-old".to_owned(),
                "token-old".to_owned(),
                "host-1".to_owned(),
                "ABCD-EFGH-IJKL".to_owned(),
                "conn-old".to_owned(),
                tx(),
            )
            .await;
        state
            .create_session(
                "session-new".to_owned(),
                "token-new".to_owned(),
                "host-1".to_owned(),
                "ABCD-EFGH-IJKL".to_owned(),
                "conn-new".to_owned(),
                tx(),
            )
            .await;

        let removed = state
            .remove_sessions_for_host(
                "host-1",
                Some("conn-old"),
                ErrorPayload {
                    code: ErrorCode::HostDisconnected,
                    message: "old connection removed".to_owned(),
                },
            )
            .await;

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].session_id, "session-old");
        assert!(state
            .session_if_valid("session-old", "token-old")
            .await
            .is_none());
        assert!(state
            .session_if_valid("session-new", "token-new")
            .await
            .is_some());
    }

    #[tokio::test]
    async fn remove_session_for_host_requires_current_connection() {
        let state = ServerState::new();
        state
            .create_session(
                "session-1".to_owned(),
                "token-1".to_owned(),
                "host-1".to_owned(),
                "ABCD-EFGH-IJKL".to_owned(),
                "conn-new".to_owned(),
                tx(),
            )
            .await;

        let stale = state
            .remove_session_for_host(
                "session-1",
                "host-1",
                "conn-old",
                ErrorPayload {
                    code: ErrorCode::Cancelled,
                    message: "host requested close".to_owned(),
                },
            )
            .await;
        assert!(stale.is_none());
        assert!(state
            .session_if_valid("session-1", "token-1")
            .await
            .is_some());

        let current = state
            .remove_session_for_host(
                "session-1",
                "host-1",
                "conn-new",
                ErrorPayload {
                    code: ErrorCode::Cancelled,
                    message: "host requested close".to_owned(),
                },
            )
            .await;
        assert!(current.is_some());
        assert!(state
            .session_if_valid("session-1", "token-1")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn machine_lookup_reports_ambiguous_short_ids() {
        let state = ServerState::new();
        state
            .register_host(
                "host-1".to_owned(),
                "ABCD-EFGH-IJKL".to_owned(),
                "conn-1".to_owned(),
                tx(),
                120,
            )
            .await;
        state
            .register_host(
                "host-2".to_owned(),
                "ABCD-EFGH-IJKL".to_owned(),
                "conn-2".to_owned(),
                tx(),
                120,
            )
            .await;

        match state.host_for_machine_id("ABCD-EFGH-IJKL").await {
            HostLookup::Ambiguous(hosts) => assert_eq!(hosts.len(), 2),
            _ => panic!("expected ambiguous host lookup"),
        }
    }

    #[tokio::test]
    async fn host_lookup_by_id_ignores_short_id_collisions() {
        let state = ServerState::new();
        state
            .register_host(
                "host-1".to_owned(),
                "ABCD-EFGH-IJKL".to_owned(),
                "conn-1".to_owned(),
                tx(),
                120,
            )
            .await;
        state
            .register_host(
                "host-2".to_owned(),
                "ABCD-EFGH-IJKL".to_owned(),
                "conn-2".to_owned(),
                tx(),
                120,
            )
            .await;

        let host = state.host("host-2").await.unwrap();

        assert_eq!(host.host_id, "host-2");
        assert_eq!(host.machine_id, "ABCD-EFGH-IJKL");
        assert_eq!(host.connection_id, "conn-2");
    }

    #[tokio::test]
    async fn remove_pending_open_for_host_respects_connection_id() {
        let state = ServerState::new();
        state
            .insert_pending_open(
                "req-old".to_owned(),
                PendingOpen {
                    host_id: "host-1".to_owned(),
                    machine_id: "ABCD-EFGH-IJKL".to_owned(),
                    connection_id: "conn-old".to_owned(),
                    controller_tx: tx(),
                    controller_label: "token:old".to_owned(),
                    force_reconnect: false,
                    created_at: Instant::now(),
                },
            )
            .await;
        state
            .insert_pending_open(
                "req-new".to_owned(),
                PendingOpen {
                    host_id: "host-1".to_owned(),
                    machine_id: "ABCD-EFGH-IJKL".to_owned(),
                    connection_id: "conn-new".to_owned(),
                    controller_tx: tx(),
                    controller_label: "token:new".to_owned(),
                    force_reconnect: false,
                    created_at: Instant::now(),
                },
            )
            .await;

        let removed = state
            .remove_pending_open_for_host("host-1", Some("conn-old"))
            .await;

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].0, "req-old");
        assert!(state.take_pending_open("req-old").await.is_none());
        assert!(state.take_pending_open("req-new").await.is_some());
    }

    #[test]
    fn append_limited_output_keeps_within_limit() {
        let mut target = String::new();
        let mut truncated = false;

        append_limited_output(&mut target, &mut truncated, "hello");

        assert_eq!(target, "hello");
        assert!(!truncated);
    }

    #[test]
    fn session_idle_ttl_is_nonzero() {
        assert!(SESSION_IDLE_TTL > std::time::Duration::ZERO);
        assert!(Instant::now().checked_add(SESSION_IDLE_TTL).is_some());
    }
}
