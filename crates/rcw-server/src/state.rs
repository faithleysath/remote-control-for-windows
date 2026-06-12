use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

use tokio::sync::Mutex;

use crate::ws::Tx;

pub(crate) struct ServerState {
    hosts: Mutex<HashMap<String, HostConn>>,
    sessions: Mutex<HashMap<String, SessionState>>,
    pending_open: Mutex<HashMap<String, PendingOpen>>,
    request_routes: Mutex<HashMap<String, String>>,
    request_controllers: Mutex<HashMap<String, Tx>>,
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
    pub(crate) last_seen: String,
}

#[derive(Clone)]
pub(crate) struct PendingOpen {
    pub(crate) machine_id: String,
    pub(crate) controller_tx: Tx,
    pub(crate) controller_label: String,
}

impl ServerState {
    pub(crate) fn new() -> Self {
        Self {
            hosts: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            pending_open: Mutex::new(HashMap::new()),
            request_routes: Mutex::new(HashMap::new()),
            request_controllers: Mutex::new(HashMap::new()),
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
                last_seen: rcw_common::audit::now_rfc3339(),
            },
        );
    }

    pub(crate) async fn session_if_valid(
        &self,
        session_id: &str,
        session_token: &str,
    ) -> Option<SessionState> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(session_id)
            .filter(|session| session.session_token == session_token)
            .cloned()
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
                session.last_seen = rcw_common::audit::now_rfc3339();
                Some(session.clone())
            }
            _ => None,
        }
    }

    pub(crate) async fn track_request_route(&self, request_id: String, session_id: String, tx: Tx) {
        let mut routes = self.request_routes.lock().await;
        routes.insert(request_id.clone(), session_id);
        drop(routes);

        let mut controllers = self.request_controllers.lock().await;
        controllers.insert(request_id, tx);
    }

    pub(crate) async fn clear_request_route(&self, request_id: &str) {
        let mut routes = self.request_routes.lock().await;
        routes.remove(request_id);
        drop(routes);

        let mut controllers = self.request_controllers.lock().await;
        controllers.remove(request_id);
    }

    pub(crate) async fn request_session_id(&self, request_id: &str) -> Option<String> {
        let routes = self.request_routes.lock().await;
        routes.get(request_id).cloned()
    }

    pub(crate) async fn controller_for_request(&self, request_id: &str) -> Option<Tx> {
        let controllers = self.request_controllers.lock().await;
        controllers.get(request_id).cloned()
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

    pub(crate) async fn unregister_host(&self, machine_id: &str) -> Vec<SessionState> {
        let mut hosts = self.hosts.lock().await;
        hosts.remove(machine_id);
        drop(hosts);

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
        let removed_request_ids = {
            let mut routes = self.request_routes.lock().await;
            let request_ids = routes
                .iter()
                .filter(|(_, session_id)| removed_session_ids.contains(*session_id))
                .map(|(request_id, _)| request_id.clone())
                .collect::<Vec<_>>();
            for request_id in &request_ids {
                routes.remove(request_id);
            }
            request_ids
        };
        if !removed_request_ids.is_empty() {
            let mut controllers = self.request_controllers.lock().await;
            for request_id in removed_request_ids {
                controllers.remove(&request_id);
            }
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
