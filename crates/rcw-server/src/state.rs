use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

use tokio::sync::Mutex;

use crate::ws::Tx;

pub(crate) const SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
pub(crate) const PENDING_OPEN_TTL: Duration = Duration::from_secs(60);
pub(crate) const REQUEST_ROUTE_TTL: Duration = Duration::from_secs(2 * 60 * 60);
pub(crate) const RATE_LIMIT_KEY_TTL: Duration = Duration::from_secs(5 * 60);

pub(crate) struct ServerState {
    hosts: Mutex<HashMap<String, HostConn>>,
    sessions: Mutex<HashMap<String, SessionState>>,
    pending_open: Mutex<HashMap<String, PendingOpen>>,
    request_routes: Mutex<HashMap<String, RequestRoute>>,
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
    pub(crate) created_at: Instant,
}

impl ServerState {
    pub(crate) fn new() -> Self {
        Self {
            hosts: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            pending_open: Mutex::new(HashMap::new()),
            request_routes: Mutex::new(HashMap::new()),
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
        let mut sessions = self.sessions.lock().await;
        match sessions.get(session_id) {
            Some(session) if session.session_token == session_token => sessions.remove(session_id),
            _ => None,
        }
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
        let mut routes = self.request_routes.lock().await;
        routes.insert(
            request_id,
            RequestRoute {
                session_id,
                controller_tx: tx,
                created_at: Instant::now(),
            },
        );
    }

    pub(crate) async fn clear_request_route(&self, request_id: &str) {
        let mut routes = self.request_routes.lock().await;
        routes.remove(request_id);
    }

    pub(crate) async fn request_session_id(&self, request_id: &str) -> Option<String> {
        let routes = self.request_routes.lock().await;
        routes.get(request_id).map(|route| route.session_id.clone())
    }

    pub(crate) async fn controller_for_request(&self, request_id: &str) -> Option<Tx> {
        let routes = self.request_routes.lock().await;
        routes
            .get(request_id)
            .map(|route| route.controller_tx.clone())
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

        self.remove_sessions_for_machine(machine_id).await
    }

    pub(crate) async fn remove_sessions_for_machine(&self, machine_id: &str) -> Vec<SessionState> {
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
            let mut routes = self.request_routes.lock().await;
            routes.retain(|_, route| !removed_session_ids.contains(&route.session_id));
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

        prune_rate_limit_table(&self.host_registrations, now, RATE_LIMIT_KEY_TTL).await;
        prune_rate_limit_table(&self.auth_attempts, now, RATE_LIMIT_KEY_TTL).await;

        removed_sessions
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
