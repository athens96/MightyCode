//! MC-34: the network gate of one sandboxed run: the [`Policy`] its proxy asks. A target on the allow list passes at once; any other
//! raises the card "이 접속을 허용할까요?" on the pane (an ordinary permission request whose tool is [`NETWORK_TOOL`]) and waits for
//! the answer: 이번만 (this run), 항상 (the host also saves it to the workspace's list) or 거부. One card per target however many
//! connections wait for it; the answer holds for the rest of the run. No answer within [`ASK_LIMIT`] is a refusal, as is the run's end.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::permission::ToolPermissionRequest;
use mighty_core::{time, wire};
use mighty_platform::BoxFuture;
use tokio::sync::oneshot;

use super::hosts::{AllowList, Target};
use super::proxy::Policy;
use crate::strings;

/// The tool name of the network card (never a CLI's own tool name).
pub use mighty_core::permission::SANDBOX_NETWORK_TOOL as NETWORK_TOOL;
/// How long a connection waits for the person.
pub const ASK_LIMIT: Duration = Duration::from_secs(300);
/// The most cards one run's gate keeps waiting.
pub const MAXIMUM_ASKED: usize = 16;

/// A connection was refused, and why.
pub type Blocked = Arc<dyn Fn(&Target, &str) + Send + Sync>;

/// What the gate tells its run.
#[derive(Clone)]
pub struct GateHooks {
    /// The card, waiting or settled (the pane's permission sink).
    pub card: Arc<dyn Fn(ToolPermissionRequest) + Send + Sync>,
    /// A connection was refused (the pane's progress and the phone).
    pub blocked: Blocked,
}

struct Asked {
    target: Target,
    display: ToolPermissionRequest,
    waiters: Vec<oneshot::Sender<bool>>,
}

#[derive(Default)]
struct State {
    allowed: HashSet<Target>,
    denied: HashSet<Target>,
    asked: HashMap<String, Asked>,
}

pub struct NetworkGate {
    run_id: String,
    list: Mutex<AllowList>,
    state: Mutex<State>,
    hooks: GateHooks,
    closed: AtomicBool,
    /// No card can be shown (a host without one): a target off the list is refused at once.
    refuse: AtomicBool,
    ask_limit: Duration,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

impl NetworkGate {
    pub fn new(run_id: &str, list: AllowList, hooks: GateHooks) -> Arc<NetworkGate> {
        Self::with_limit(run_id, list, hooks, ASK_LIMIT)
    }

    pub fn with_limit(run_id: &str, list: AllowList, hooks: GateHooks, ask_limit: Duration) -> Arc<NetworkGate> {
        Arc::new(NetworkGate {
            run_id: run_id.into(),
            list: Mutex::new(list),
            state: Mutex::new(State::default()),
            hooks,
            closed: AtomicBool::new(false),
            refuse: AtomicBool::new(false),
            ask_limit,
        })
    }

    /// The card for `target` (pending).
    pub fn card(&self, id: &str, target: &Target) -> ToolPermissionRequest {
        let input = serde_json::json!({ "host": target.host, "port": target.port }).to_string();
        let mut request = ToolPermissionRequest::new(id, &self.run_id, id, NETWORK_TOOL, &input, &target.label());
        request.reason = Some(strings::get("sandbox.network.notListed"));
        request.received_at = Some(time::now_o());
        request
    }

    /// Whether `request_id` is one of this gate's cards still waiting.
    pub fn owns(&self, request_id: &str) -> bool {
        lock(&self.state).asked.contains_key(request_id)
    }

    /// The target a waiting card asks about.
    pub fn target_of(&self, request_id: &str) -> Option<Target> {
        lock(&self.state).asked.get(request_id).map(|a| a.target.clone())
    }

    /// The cards waiting now.
    pub fn waiting(&self) -> Vec<ToolPermissionRequest> {
        lock(&self.state).asked.values().map(|a| a.display.clone()).collect()
    }

    /// The person's answer to a card.
    pub fn respond(&self, request_id: &str, allow: bool) -> Result<(), String> {
        if !self.owns(request_id) {
            return Err(strings::get("sandbox.network.settled"));
        }
        let reason = strings::get("sandbox.network.deniedByYou");
        self.settle(request_id, allow, if allow { "allowed" } else { "denied" }, Some(&reason));
        Ok(())
    }

    fn settle(&self, request_id: &str, allow: bool, state: &str, reason: Option<&str>) {
        let Some(asked) = ({
            let mut current = lock(&self.state);
            let asked = current.asked.remove(request_id);
            if let Some(asked) = &asked {
                if allow {
                    current.allowed.insert(asked.target.clone());
                } else {
                    current.denied.insert(asked.target.clone());
                }
            }
            asked
        }) else {
            return;
        };
        for waiter in asked.waiters {
            let _ = waiter.send(allow);
        }
        (self.hooks.card)(ToolPermissionRequest { state: state.into(), ..asked.display });
        if !allow && state != "cancelled" {
            (self.hooks.blocked)(&asked.target, reason.unwrap_or(""));
        }
    }

    /// Nothing can ask the person: every target off the list is refused without a card.
    pub fn refuse_unlisted(&self) {
        self.refuse.store(true, Ordering::SeqCst);
    }

    /// The run ended: every waiting card is withdrawn and nothing passes any more.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let ids: Vec<String> = lock(&self.state).asked.keys().cloned().collect();
        for id in ids {
            self.settle(&id, false, "cancelled", None);
        }
    }

    /// Whether `target` may be reached (asking the person when it must).
    pub async fn decide(self: &Arc<Self>, target: Target) -> bool {
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        if lock(&self.list).allows(&target) {
            return true;
        }
        let (sender, receiver) = oneshot::channel();
        let new_card = {
            let mut state = lock(&self.state);
            if state.allowed.contains(&target) {
                return true;
            }
            if state.denied.contains(&target) {
                return false;
            }
            if self.refuse.load(Ordering::SeqCst) {
                state.denied.insert(target.clone());
                drop(state);
                (self.hooks.blocked)(&target, &strings::get("sandbox.network.notListed"));
                return false;
            }
            if let Some(asked) = state.asked.values_mut().find(|a| a.target == target) {
                asked.waiters.push(sender);
                None
            } else if state.asked.len() >= MAXIMUM_ASKED {
                drop(state);
                (self.hooks.blocked)(&target, &strings::get("sandbox.network.tooMany"));
                return false;
            } else {
                let id = wire::new_id();
                let display = self.card(&id, &target);
                state.asked.insert(id.clone(), Asked { target: target.clone(), display: display.clone(), waiters: vec![sender] });
                Some((id, display))
            }
        };
        let id = new_card.as_ref().map(|(id, _)| id.clone());
        if let Some((_, display)) = new_card {
            (self.hooks.card)(display);
        }
        match tokio::time::timeout(self.ask_limit, receiver).await {
            Ok(Ok(allow)) => allow,
            Ok(Err(_)) => false,
            Err(_) => {
                if let Some(id) = id {
                    self.settle(&id, false, "denied", Some(&strings::get("sandbox.network.timedOut")));
                }
                false
            }
        }
    }
}

/// The proxy's policy: the gate.
pub struct GatePolicy(pub Arc<NetworkGate>);

impl Policy for GatePolicy {
    fn decide(&self, target: Target) -> BoxFuture<'static, bool> {
        let gate = self.0.clone();
        Box::pin(async move { gate.decide(target).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Seen {
        cards: Mutex<Vec<ToolPermissionRequest>>,
        blocked: Mutex<Vec<(Target, String)>>,
    }

    fn gate(limit: Duration) -> (Arc<NetworkGate>, Arc<Seen>) {
        crate::strings::set_language("ko");
        let seen = Arc::new(Seen { cards: Mutex::new(vec![]), blocked: Mutex::new(vec![]) });
        let (cards, blocked) = (seen.clone(), seen.clone());
        let hooks = GateHooks {
            card: Arc::new(move |c| cards.cards.lock().unwrap().push(c)),
            blocked: Arc::new(move |t, why| blocked.blocked.lock().unwrap().push((t.clone(), why.to_string()))),
        };
        (NetworkGate::with_limit("pane-1", AllowList::new("claude", &["github.com".into()]), hooks, limit), seen)
    }

    fn t(text: &str) -> Target {
        Target::parse(text, 443).unwrap()
    }

    /// A blocked host raises one card (however many connections wait for it), and the answer settles them all and holds for the run.
    #[tokio::test]
    async fn a_blocked_host_raises_one_card() {
        let (gate, seen) = gate(ASK_LIMIT);
        assert!(gate.decide(t("api.anthropic.com")).await, "the agent's own service");
        assert!(gate.decide(t("github.com")).await);
        assert!(seen.cards.lock().unwrap().is_empty());
        let (a, b) = (gate.clone(), gate.clone());
        let first = tokio::spawn(async move { a.decide(t("example.com:8443")).await });
        let second = tokio::spawn(async move { b.decide(t("example.com:8443")).await });
        while gate.waiting().is_empty() {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        let card = seen.cards.lock().unwrap()[0].clone();
        assert_eq!(seen.cards.lock().unwrap().len(), 1, "one card for one target");
        assert_eq!(card.tool_name, NETWORK_TOOL);
        assert_eq!(card.summary, "example.com:8443");
        assert_eq!(card.run_id, "pane-1");
        assert_eq!(card.state, "pending");
        assert!(card.can_allow);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&card.input_json).unwrap()["port"], 8443);
        assert_eq!(gate.target_of(&card.id), Some(t("example.com:8443")));
        gate.respond(&card.id, true).unwrap();
        assert!(first.await.unwrap() && second.await.unwrap());
        assert_eq!(seen.cards.lock().unwrap().last().unwrap().state, "allowed");
        assert!(gate.decide(t("example.com:8443")).await, "held for the run");
        assert!(gate.respond(&card.id, false).is_err(), "settled once");

        let other = gate.clone();
        let refused = tokio::spawn(async move { other.decide(t("evil.example")).await });
        while gate.waiting().is_empty() {
            tokio::task::yield_now().await;
        }
        let id = gate.waiting()[0].id.clone();
        gate.respond(&id, false).unwrap();
        assert!(!refused.await.unwrap());
        assert!(!gate.decide(t("evil.example")).await, "asked once");
        assert_eq!(seen.blocked.lock().unwrap().len(), 1);
        assert_eq!(seen.blocked.lock().unwrap()[0].0, t("evil.example"));
    }

    /// No answer in time is a refusal; the run's end withdraws what waits and lets nothing through.
    #[tokio::test]
    async fn silence_and_the_end_refuse() {
        let (gate, seen) = gate(Duration::from_millis(100));
        assert!(!gate.decide(t("slow.example")).await);
        let last = seen.cards.lock().unwrap().last().unwrap().clone();
        assert_eq!(last.state, "denied");
        assert_eq!(seen.blocked.lock().unwrap().len(), 1);

        let (gate, seen) = self::gate(ASK_LIMIT);
        let waiting = gate.clone();
        let pending = tokio::spawn(async move { waiting.decide(t("late.example")).await });
        while gate.waiting().is_empty() {
            tokio::task::yield_now().await;
        }
        gate.close();
        assert!(!pending.await.unwrap());
        assert_eq!(seen.cards.lock().unwrap().last().unwrap().state, "cancelled");
        assert!(!gate.decide(t("github.com")).await, "nothing passes after the end");
    }
}
