use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

use rcw_common::protocol::{
    CommandCompletePayload, CommandStatusResultPayload, CommandTaskStatus, ErrorCode, ErrorPayload,
    MAX_CAPTURED_OUTPUT_BYTES,
};
use tokio::sync::Mutex;

use crate::ws::Tx;

pub(crate) const SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
pub(crate) const PENDING_OPEN_TTL: Duration = Duration::from_secs(60);
pub(crate) const REQUEST_ROUTE_TTL: Duration = Duration::from_secs(2 * 60 * 60);
pub(crate) const COMMAND_JOB_TTL: Duration = Duration::from_secs(30 * 60);
pub(crate) const RATE_LIMIT_KEY_TTL: Duration = Duration::from_secs(5 * 60);

pub(crate) struct ServerState {
    hosts: Mutex<HashMap<String, HostConn>>,
    sessions: Mutex<HashMap<String, SessionState>>,
    pending_open: Mutex<HashMap<String, PendingOpen>>,
    request_routes: Mutex<HashMap<String, RequestRoute>>,
    command_jobs: Mutex<HashMap<String, CommandJob>>,
    host_registrations: Mutex<HashMap<String, VecDeque<Instant>>>,
    auth_attempts: Mutex<HashMap<String, VecDeque<Instant>>>,
}

#[derive(Clone)]
pub(crate) struct HostConn {
    pub(crate) tx: Tx,
    pub(crate) totp_period_seconds: u64,
}

#[derive(Clone)]
pub(crate) struct SessionState {
    pub(crate) session_id: String,
    pub(crate) session_token: String,
    pub(crate) machine_id: String,
    pub(crate) controller_tx: Option<Tx>,
    pub(crate) last_seen: Instant,
}

#[derive(Clone)]
pub(crate) struct PendingOpen {
    pub(crate) machine_id: String,
    pub(crate) controller_tx: Tx,
    pub(crate) controller_label: String,
    pub(crate) force_reconnect: bool,
    pub(crate) created_at: Instant,
}

#[derive(Clone)]
pub(crate) struct RequestRoute {
    pub(crate) session_id: String,
    pub(crate) controller_tx: Tx,
    pub(crate) detached: bool,
    pub(crate) created_at: Instant,
}

pub(crate) struct CommandJob {
    snapshot: CommandStatusResultPayload,
    machine_id: String,
    session_id: String,
    session_token: String,
    created_at: Instant,
    finished_at: Option<Instant>,
}

impl ServerState {
    pub(crate) fn new() -> Self {
        Self {
            hosts: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            pending_open: Mutex::new(HashMap::new()),
            request_routes: Mutex::new(HashMap::new()),
            command_jobs: Mutex::new(HashMap::new()),
            host_registrations: Mutex::new(HashMap::new()),
            auth_attempts: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn register_host(&self, machine_id: String, tx: Tx, totp_period_seconds: u64) {
        let mut hosts = self.hosts.lock().await;
        hosts.insert(
            machine_id,
            HostConn {
                tx,
                totp_period_seconds,
            },
        );
    }

    pub(crate) async fn host(&self, machine_id: &str) -> Option<HostConn> {
        let hosts = self.hosts.lock().await;
        hosts.get(machine_id).cloned()
    }

    pub(crate) async fn host_tx(&self, machine_id: &str) -> Option<Tx> {
        self.host(machine_id).await.map(|host| host.tx)
    }

    pub(crate) async fn host_has_active_session(&self, machine_id: &str) -> bool {
        let sessions = self.sessions.lock().await;
        sessions
            .values()
            .any(|session| session.machine_id == machine_id)
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

    pub(crate) async fn create_session(
        &self,
        session_id: String,
        session_token: String,
        machine_id: String,
        controller_tx: Tx,
    ) {
        let mut sessions = self.sessions.lock().await;
        sessions.insert(
            session_id.clone(),
            SessionState {
                session_id,
                session_token,
                machine_id,
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

    pub(crate) async fn track_request_route(&self, request_id: String, session_id: String, tx: Tx) {
        self.track_request_route_with_mode(request_id, session_id, tx, false)
            .await;
    }

    pub(crate) async fn track_detached_request_route(
        &self,
        request_id: String,
        session_id: String,
        tx: Tx,
    ) {
        self.track_request_route_with_mode(request_id, session_id, tx, true)
            .await;
    }

    async fn track_request_route_with_mode(
        &self,
        request_id: String,
        session_id: String,
        tx: Tx,
        detached: bool,
    ) {
        let mut routes = self.request_routes.lock().await;
        routes.insert(
            request_id,
            RequestRoute {
                session_id,
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

    pub(crate) async fn request_session_id(&self, request_id: &str) -> Option<String> {
        let routes = self.request_routes.lock().await;
        routes.get(request_id).map(|route| route.session_id.clone())
    }

    pub(crate) async fn controller_for_request(&self, request_id: &str) -> Option<Tx> {
        let routes = self.request_routes.lock().await;
        routes
            .get(request_id)
            .filter(|route| !route.detached)
            .map(|route| route.controller_tx.clone())
    }

    pub(crate) async fn request_route_is_detached(&self, request_id: &str) -> bool {
        {
            let routes = self.request_routes.lock().await;
            if routes
                .get(request_id)
                .map(|route| route.detached)
                .unwrap_or(false)
            {
                return true;
            }
        }
        let jobs = self.command_jobs.lock().await;
        jobs.contains_key(request_id)
    }

    pub(crate) async fn session_controller_for_machine(
        &self,
        session_id: &str,
        machine_id: &str,
    ) -> Option<Tx> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(session_id)
            .filter(|session| session.machine_id == machine_id)
            .and_then(|session| session.controller_tx.clone())
    }

    pub(crate) async fn command_route(&self, session_id: &str) -> Option<(String, String)> {
        let sessions = self.sessions.lock().await;
        let session = sessions.get(session_id)?;
        Some((session.machine_id.clone(), session.session_id.clone()))
    }

    pub(crate) async fn touch_session(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.last_seen = Instant::now();
        }
    }

    pub(crate) async fn unregister_host(&self, machine_id: &str) -> Vec<SessionState> {
        let mut hosts = self.hosts.lock().await;
        hosts.remove(machine_id);
        drop(hosts);

        self.remove_sessions_for_machine(
            machine_id,
            ErrorPayload {
                code: ErrorCode::HostDisconnected,
                message: ErrorCode::HostDisconnected.message().to_owned(),
            },
        )
        .await
    }

    pub(crate) async fn remove_sessions_for_machine(
        &self,
        machine_id: &str,
        error: ErrorPayload,
    ) -> Vec<SessionState> {
        let removed_sessions = {
            let mut sessions = self.sessions.lock().await;
            let session_ids = sessions
                .values()
                .filter(|session| session.machine_id == machine_id)
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
            self.fail_running_exec_jobs_for_sessions(&removed_session_ids, error)
                .await;
        }

        removed_sessions
    }

    pub(crate) async fn allow_host_registration(&self, machine_id: &str) -> bool {
        allow_rate_limit(
            &self.host_registrations,
            machine_id,
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

    pub(crate) async fn create_exec_job(
        &self,
        task_id: String,
        machine_id: String,
        session_id: String,
        session_token: String,
        started_at: String,
    ) -> CommandStatusResultPayload {
        let snapshot = CommandStatusResultPayload {
            task_id: task_id.clone(),
            status: CommandTaskStatus::Running,
            request_id: task_id.clone(),
            started_at,
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
            task_id,
            CommandJob {
                snapshot: snapshot.clone(),
                machine_id,
                session_id,
                session_token,
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
    ) -> Option<(String, String)> {
        let jobs = self.command_jobs.lock().await;
        jobs.get(task_id)
            .filter(|job| job.session_token == session_token)
            .filter(|job| job.snapshot.status == CommandTaskStatus::Running)
            .map(|job| (job.machine_id.clone(), job.session_id.clone()))
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
