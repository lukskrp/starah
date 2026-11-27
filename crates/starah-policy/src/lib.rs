//! Starah's policy engine (no I/O, no clock: callers pass `now`).
//!
//! An agent has a **level** (0–4). Every privileged action (a polkit action id) has a
//! **class**. The level and the class give a verdict: allow, ask the human, or deny.
//! "Ask" turns into a pending **request**; when the human approves it, the agent gets
//! a **grant** for that action, and its next attempt is allowed. The person's own
//! **rules** (a Rhai script, see `rules`) can refine any verdict except "forbidden".

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod rules;

pub use rules::{RuleVerdict, Rules, RulesError};

/// How much an agent may do on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum Level {
    /// Read system state only; every privileged action is denied.
    Observe = 0,
    /// Nothing on its own: every privileged action is a request to the human.
    Propose = 1,
    /// Low-risk actions on its own; the rest is asked.
    Routine = 2,
    /// System administration on its own; sensitive actions are asked.
    Administer = 3,
    /// Everything except the forbidden class, which stays with the human.
    Delegate = 4,
}

impl Level {
    pub const ALL: [Level; 5] = [
        Level::Observe,
        Level::Propose,
        Level::Routine,
        Level::Administer,
        Level::Delegate,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Level::Observe => "Observe",
            Level::Propose => "Propose",
            Level::Routine => "Routine",
            Level::Administer => "Administer",
            Level::Delegate => "Delegate",
        }
    }
}

impl TryFrom<u8> for Level {
    type Error = String;
    fn try_from(n: u8) -> Result<Self, String> {
        Level::ALL
            .get(n as usize)
            .copied()
            .ok_or_else(|| format!("no such level {n} (0–4)"))
    }
}

impl From<Level> for u8 {
    fn from(l: Level) -> u8 {
        l as u8
    }
}

/// What kind of power an action gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    /// Reads state; changes nothing.
    Observe,
    /// Low-risk, easily undone (browser extensions, mounting a disk, TulsiOS changes a
    /// NixOS rollback undoes).
    Routine,
    /// System administration (free-form configuration, services, network, power).
    Admin,
    /// Could take over the machine or its data (unit files, raw disks, VMs).
    Sensitive,
    /// Never delegated: Starah itself, accounts, pkexec, polkit/sudo rules.
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Allow,
    Ask,
    Deny,
}

/// The level × class table.
pub fn verdict(level: Level, class: Class) -> Verdict {
    use Class as C;
    use Verdict::*;
    match (level, class) {
        (_, C::Forbidden) => Deny,
        (_, C::Observe) => Allow,
        (Level::Observe, _) => Deny,
        (Level::Propose, _) => Ask,
        (Level::Routine, C::Routine) => Allow,
        (Level::Routine, _) => Ask,
        (Level::Administer, C::Sensitive) => Ask,
        (Level::Administer, _) => Allow,
        (Level::Delegate, _) => Allow,
    }
}

/// polkit action id → class. Exact ids win over prefixes; the longest prefix wins;
/// anything unknown is `default` (sensitive unless configured otherwise).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Classes {
    #[serde(default = "default_class")]
    pub default: Class,
    #[serde(default)]
    pub actions: BTreeMap<String, Class>,
    #[serde(default)]
    pub prefixes: BTreeMap<String, Class>,
}

fn default_class() -> Class {
    Class::Sensitive
}

impl Default for Classes {
    fn default() -> Self {
        Classes {
            default: Class::Sensitive,
            actions: BTreeMap::new(),
            prefixes: BTreeMap::new(),
        }
    }
}

impl Classes {
    pub fn class_of(&self, action: &str) -> Class {
        // Starah's own actions are never delegated, whatever the file says.
        if action.starts_with("org.starah.") {
            return Class::Forbidden;
        }
        if let Some(c) = self.actions.get(action) {
            return *c;
        }
        self.prefixes
            .iter()
            .filter(|(p, _)| action.starts_with(p.as_str()))
            .max_by_key(|(p, _)| p.len())
            .map(|(_, c)| *c)
            .unwrap_or(self.default)
    }
}

/// An agent as declared by the system configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    /// The level until the human chooses one.
    pub level: Level,
    #[serde(default)]
    pub description: String,
    /// Links made in the agent's home for each person at an interactive session's
    /// start (`.claude/skills` → `/etc/tulsi/agents/skills`), unless already there.
    #[serde(default)]
    pub home_links: BTreeMap<String, String>,
}

/// The configuration starahd starts from (generated by the NixOS module).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub agents: BTreeMap<String, AgentSpec>,
    #[serde(default)]
    pub classes: Classes,
    /// Minutes an approved request stays granted for polkit retries.
    #[serde(default = "default_grant_minutes")]
    pub grant_minutes: u64,
}

fn default_grant_minutes() -> u64 {
    10
}

/// What the human chose (persisted by starahd).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Policy {
    /// False until the human has confirmed the levels once (first-login setup).
    pub configured: bool,
    #[serde(default)]
    pub levels: BTreeMap<String, Level>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_by: Option<String>,
    #[serde(default)]
    pub configured_at: u64,
    /// Agent → the people who let it use their whole home in interactive sessions
    /// (logins, SSH keys, other projects); otherwise it sees only the project folder.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub home_access: BTreeMap<String, std::collections::BTreeSet<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Grant {
    pub agent: String,
    pub action: String,
    /// Restricts the grant to one detail (e.g. the systemd unit); None = any.
    pub detail: Option<String>,
    pub expires: u64,
    /// None = unlimited until expiry.
    pub uses_left: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestKind {
    /// Filed by the polkit check when an action was refused pending approval.
    Polkit,
    /// Filed by a cooperating agent (tulsi-mcp) before acting, with a description.
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pending,
    Approved,
    Rejected,
    Expired,
}

/// What a cooperating agent tells the human about the change it wants.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Description {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub changes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub agent: String,
    pub level: Level,
    pub action: String,
    pub class: Class,
    pub kind: RequestKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub description: Description,
    pub created: u64,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
}

/// The answer to a check or an ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum Outcome {
    Allow { why: String },
    Deny { why: String },
    Pending { id: u64 },
}

impl Outcome {
    pub fn allowed(&self) -> bool {
        matches!(self, Outcome::Allow { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    UnknownAgent(String),
    UnknownRequest(u64),
    NotPending(u64),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UnknownAgent(a) => write!(f, "no agent named {a:?}"),
            Error::UnknownRequest(id) => write!(f, "no request {id}"),
            Error::NotPending(id) => write!(f, "request {id} was already decided"),
        }
    }
}

impl std::error::Error for Error {}

/// Pending requests older than this expire (seconds).
pub const REQUEST_TTL: u64 = 15 * 60;
/// An approved `Ask` is good for one use within this many seconds.
pub const ASK_GRANT_TTL: u64 = 10 * 60;

pub struct Broker {
    pub config: Config,
    pub policy: Policy,
    pub grants: Vec<Grant>,
    pub requests: Vec<Request>,
    /// The person's own rules (rules.rhai), if any.
    pub rules: Option<Rules>,
    /// The local time zone's offset from UTC (seconds), for the rules' `hour`.
    pub utc_offset: i64,
    /// The last time the rules failed while deciding (taken by the caller, for the
    /// audit log).
    pub rules_failure: Option<String>,
    next_id: u64,
}

/// The stricter of two verdicts.
pub fn stricter(a: Verdict, b: Verdict) -> Verdict {
    let rank = |v| match v {
        Verdict::Allow => 0,
        Verdict::Ask => 1,
        Verdict::Deny => 2,
    };
    if rank(a) >= rank(b) {
        a
    } else {
        b
    }
}

/// A decision and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub verdict: Verdict,
    pub why: String,
    pub level: Level,
    pub class: Class,
    /// Whether the person's rules decided (rather than the level table).
    pub by_rules: bool,
}

impl Broker {
    pub fn new(config: Config, policy: Policy) -> Self {
        Broker {
            config,
            policy,
            grants: Vec::new(),
            requests: Vec::new(),
            rules: None,
            utc_offset: 0,
            rules_failure: None,
            next_id: 1,
        }
    }

    /// What the level table and the person's rules say about `agent` doing
    /// `action`; files nothing. None for an unknown agent.
    pub fn evaluate(
        &mut self,
        agent: &str,
        action: &str,
        detail: Option<&str>,
        description: &Description,
        now: u64,
    ) -> Option<Decision> {
        let level = self.level(agent)?;
        let class = self.config.classes.class_of(action);
        let table = verdict(level, class);
        let table_why = |v: Verdict| {
            let says = match v {
                Verdict::Allow => "which allows this",
                Verdict::Ask => "which asks you first",
                Verdict::Deny => "which does not allow this",
            };
            format!(
                "{agent} has level {} ({}), {says}",
                level as u8,
                level.name()
            )
        };
        let decision = |verdict, why, by_rules| Decision {
            verdict,
            why,
            level,
            class,
            by_rules,
        };
        // Starah itself, accounts, polkit and sudo rules: no rule changes that.
        if class == Class::Forbidden {
            return Some(decision(
                Verdict::Deny,
                "no agent may do this (Starah keeps it for people)".into(),
                false,
            ));
        }
        let Some(rules) = &self.rules else {
            return Some(decision(table, table_why(table), false));
        };
        let input = rules::RuleInput {
            agent,
            level,
            action,
            class,
            detail,
            description,
            default: table,
            now,
            utc_offset: self.utc_offset,
        };
        Some(match rules.decide(&input) {
            Ok(RuleVerdict::Default) => decision(table, table_why(table), false),
            Ok(RuleVerdict::Allow) => {
                decision(Verdict::Allow, "your rules allow this".into(), true)
            }
            Ok(RuleVerdict::Ask) => decision(Verdict::Ask, "your rules ask you first".into(), true),
            Ok(RuleVerdict::Deny) => {
                decision(Verdict::Deny, "your rules do not allow this".into(), true)
            }
            Err(e) => {
                let v = stricter(table, Verdict::Ask);
                let why = format!("your rules failed ({e}), so: {}", table_why(v));
                self.rules_failure = Some(e);
                decision(v, why, true)
            }
        })
    }

    /// The agent's effective level: the human's choice, else the configured default.
    pub fn level(&self, agent: &str) -> Option<Level> {
        let spec = self.config.agents.get(agent)?;
        Some(self.policy.levels.get(agent).copied().unwrap_or(spec.level))
    }

    /// The polkit path: never blocks. "Ask" files (or reuses) a pending request and
    /// answers no; after approval the agent's retry finds the grant.
    pub fn check(&mut self, agent: &str, action: &str, detail: Option<&str>, now: u64) -> Outcome {
        self.expire(now);
        let Some(d) = self.evaluate(agent, action, detail, &Description::default(), now) else {
            return Outcome::Deny {
                why: format!("{agent:?} is not a Starah agent"),
            };
        };
        let (level, class) = (d.level, d.class);
        match d.verdict {
            Verdict::Allow => Outcome::Allow { why: d.why },
            Verdict::Deny => Outcome::Deny { why: d.why },
            Verdict::Ask => {
                if self.use_grant(agent, action, detail) {
                    return Outcome::Allow {
                        why: "approved by the human".into(),
                    };
                }
                let existing = self.requests.iter().find(|r| {
                    r.status == Status::Pending
                        && r.kind == RequestKind::Polkit
                        && r.agent == agent
                        && r.action == action
                        && r.detail.as_deref() == detail
                });
                if let Some(r) = existing {
                    return Outcome::Pending { id: r.id };
                }
                let id = self.file(
                    agent,
                    level,
                    action,
                    class,
                    RequestKind::Polkit,
                    detail,
                    Description::default(),
                    now,
                );
                Outcome::Pending { id }
            }
        }
    }

    /// The cooperating-agent path: the agent describes the change before making it.
    /// Allow means "go ahead" (the following polkit check will pass too).
    pub fn ask(
        &mut self,
        agent: &str,
        action: &str,
        description: Description,
        now: u64,
    ) -> Outcome {
        self.expire(now);
        let Some(d) = self.evaluate(agent, action, None, &description, now) else {
            return Outcome::Deny {
                why: format!("{agent:?} is not a Starah agent"),
            };
        };
        let (level, class) = (d.level, d.class);
        match d.verdict {
            Verdict::Allow => Outcome::Allow { why: d.why },
            Verdict::Deny => Outcome::Deny { why: d.why },
            Verdict::Ask => {
                let id = self.file(
                    agent,
                    level,
                    action,
                    class,
                    RequestKind::Ask,
                    None,
                    description,
                    now,
                );
                Outcome::Pending { id }
            }
        }
    }

    /// The human's answer. Approving creates the grant the agent's next attempt uses.
    pub fn decide(
        &mut self,
        id: u64,
        approve: bool,
        minutes: Option<u64>,
        by: &str,
        now: u64,
    ) -> Result<Request, Error> {
        self.expire(now);
        let grant_minutes = self.config.grant_minutes;
        let req = self
            .requests
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or(Error::UnknownRequest(id))?;
        if req.status != Status::Pending {
            return Err(Error::NotPending(id));
        }
        req.status = if approve {
            Status::Approved
        } else {
            Status::Rejected
        };
        req.decided_by = Some(by.to_string());
        let req = req.clone();
        if approve {
            let grant = match req.kind {
                RequestKind::Ask => Grant {
                    agent: req.agent.clone(),
                    action: req.action.clone(),
                    detail: None,
                    expires: now + ASK_GRANT_TTL,
                    uses_left: Some(1),
                },
                RequestKind::Polkit => Grant {
                    agent: req.agent.clone(),
                    action: req.action.clone(),
                    detail: req.detail.clone(),
                    expires: now + 60 * minutes.unwrap_or(grant_minutes).max(1),
                    uses_left: None,
                },
            };
            self.grants.push(grant);
        }
        Ok(req)
    }

    /// The human's level choices (first-login setup or later changes).
    pub fn set_levels(
        &mut self,
        levels: BTreeMap<String, Level>,
        by: &str,
        now: u64,
    ) -> Result<(), Error> {
        if let Some(unknown) = levels.keys().find(|a| !self.config.agents.contains_key(*a)) {
            return Err(Error::UnknownAgent(unknown.clone()));
        }
        for (agent, level) in levels {
            self.policy.levels.insert(agent, level);
        }
        self.policy.configured = true;
        self.policy.configured_by = Some(by.to_string());
        self.policy.configured_at = now;
        // A changed level should not be bypassed by older approvals.
        self.grants.clear();
        Ok(())
    }

    /// Whether `person` let `agent` use their whole home.
    pub fn home_access(&self, agent: &str, person: &str) -> bool {
        self.policy
            .home_access
            .get(agent)
            .is_some_and(|people| people.contains(person))
    }

    /// `person` lets `agent` use their whole home, or takes that back.
    pub fn set_home_access(&mut self, agent: &str, person: &str, allow: bool) -> Result<(), Error> {
        if !self.config.agents.contains_key(agent) {
            return Err(Error::UnknownAgent(agent.to_string()));
        }
        let people = self
            .policy
            .home_access
            .entry(agent.to_string())
            .or_default();
        if allow {
            people.insert(person.to_string());
        } else {
            people.remove(person);
        }
        self.policy.home_access.retain(|_, p| !p.is_empty());
        Ok(())
    }

    pub fn pending(&self) -> Vec<&Request> {
        self.requests
            .iter()
            .filter(|r| r.status == Status::Pending)
            .collect()
    }

    pub fn request(&self, id: u64) -> Option<&Request> {
        self.requests.iter().find(|r| r.id == id)
    }

    /// Drops spent grants and expires old requests. Returns the newly expired ids.
    pub fn expire(&mut self, now: u64) -> Vec<u64> {
        self.grants
            .retain(|g| g.expires > now && g.uses_left.is_none_or(|n| n > 0));
        let mut expired = Vec::new();
        for r in &mut self.requests {
            if r.status == Status::Pending && now >= r.created + REQUEST_TTL {
                r.status = Status::Expired;
                expired.push(r.id);
            }
        }
        // Keep a bounded history of decided requests.
        let decided = self
            .requests
            .iter()
            .filter(|r| r.status != Status::Pending)
            .count();
        if decided > 200 {
            let mut drop = decided - 200;
            self.requests.retain(|r| {
                if drop > 0 && r.status != Status::Pending {
                    drop -= 1;
                    false
                } else {
                    true
                }
            });
        }
        expired
    }

    fn use_grant(&mut self, agent: &str, action: &str, detail: Option<&str>) -> bool {
        let found = self.grants.iter_mut().find(|g| {
            g.agent == agent
                && g.action == action
                && (g.detail.is_none() || g.detail.as_deref() == detail)
                && g.uses_left.is_none_or(|n| n > 0)
        });
        match found {
            Some(g) => {
                if let Some(n) = g.uses_left.as_mut() {
                    *n -= 1;
                }
                true
            }
            None => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn file(
        &mut self,
        agent: &str,
        level: Level,
        action: &str,
        class: Class,
        kind: RequestKind,
        detail: Option<&str>,
        description: Description,
        now: u64,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.requests.push(Request {
            id,
            agent: agent.to_string(),
            level,
            action: action.to_string(),
            class,
            kind,
            detail: detail.map(str::to_string),
            description,
            created: now,
            status: Status::Pending,
            decided_by: None,
        });
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn broker(level: Level) -> Broker {
        let classes: Classes = serde_json::from_value(serde_json::json!({
            "actions": {
                "org.tulsi.extensions": "routine",
                "org.tulsi.apply": "admin",
                "org.freedesktop.policykit.exec": "forbidden"
            },
            "prefixes": {
                "org.freedesktop.systemd1.": "admin",
                "org.freedesktop.systemd1.manage-unit-files": "sensitive"
            }
        }))
        .unwrap();
        let mut agents = BTreeMap::new();
        agents.insert(
            "assistant".to_string(),
            AgentSpec {
                level,
                description: String::new(),
                home_links: BTreeMap::new(),
            },
        );
        Broker::new(
            Config {
                agents,
                classes,
                grant_minutes: 10,
            },
            Policy::default(),
        )
    }

    #[test]
    fn table() {
        use Class as C;
        use Verdict::*;
        let expect = [
            (Level::Observe, [Allow, Deny, Deny, Deny, Deny]),
            (Level::Propose, [Allow, Ask, Ask, Ask, Deny]),
            (Level::Routine, [Allow, Allow, Ask, Ask, Deny]),
            (Level::Administer, [Allow, Allow, Allow, Ask, Deny]),
            (Level::Delegate, [Allow, Allow, Allow, Allow, Deny]),
        ];
        let classes = [C::Observe, C::Routine, C::Admin, C::Sensitive, C::Forbidden];
        for (level, row) in expect {
            for (class, want) in classes.iter().zip(row) {
                assert_eq!(verdict(level, *class), want, "{level:?} × {class:?}");
            }
        }
    }

    /// The shipped classes: what a NixOS rollback undoes is routine, free-form
    /// configuration that reaches outside a generation is admin.
    #[test]
    fn shipped_classes_follow_reversibility() {
        let classes: Classes =
            serde_json::from_str(include_str!("../../../data/classes.json")).unwrap();
        assert_eq!(classes.class_of("org.tulsi.apply"), Class::Routine);
        assert_eq!(classes.class_of("org.tulsi.extensions"), Class::Routine);
        assert_eq!(classes.class_of("org.tulsi.configure"), Class::Admin);
        assert_eq!(classes.class_of("org.tulsi.anything-new"), Class::Sensitive);
        assert_eq!(
            verdict(Level::Routine, classes.class_of("org.tulsi.apply")),
            Verdict::Allow
        );
        assert_eq!(
            verdict(Level::Routine, classes.class_of("org.tulsi.configure")),
            Verdict::Ask
        );
    }

    #[test]
    fn classes_exact_prefix_default() {
        let b = broker(Level::Propose);
        let c = &b.config.classes;
        assert_eq!(c.class_of("org.tulsi.apply"), Class::Admin);
        assert_eq!(
            c.class_of("org.freedesktop.systemd1.manage-units"),
            Class::Admin
        );
        assert_eq!(
            c.class_of("org.freedesktop.systemd1.manage-unit-files"),
            Class::Sensitive,
            "longest prefix wins"
        );
        assert_eq!(c.class_of("com.example.unknown"), Class::Sensitive);
        assert_eq!(c.class_of("org.starah.configure"), Class::Forbidden);
    }

    #[test]
    fn unknown_agent_denied() {
        let mut b = broker(Level::Delegate);
        assert!(matches!(
            b.check("intruder", "org.tulsi.apply", None, 0),
            Outcome::Deny { .. }
        ));
    }

    #[test]
    fn propose_files_one_request_then_grant_allows_retries() {
        let mut b = broker(Level::Propose);
        let a = b.check(
            "assistant",
            "org.freedesktop.systemd1.manage-units",
            Some("x.service"),
            100,
        );
        let Outcome::Pending { id } = a else {
            panic!("{a:?}")
        };
        // A retry before the decision reuses the same request.
        assert_eq!(
            b.check(
                "assistant",
                "org.freedesktop.systemd1.manage-units",
                Some("x.service"),
                101
            ),
            Outcome::Pending { id }
        );
        assert_eq!(b.pending().len(), 1);
        b.decide(id, true, Some(5), "alice", 110).unwrap();
        for t in [111, 200, 300] {
            assert!(b
                .check(
                    "assistant",
                    "org.freedesktop.systemd1.manage-units",
                    Some("x.service"),
                    t
                )
                .allowed());
        }
        // Scoped to the unit it was approved for.
        assert!(!b
            .check(
                "assistant",
                "org.freedesktop.systemd1.manage-units",
                Some("y.service"),
                120
            )
            .allowed());
        // And it expires.
        assert!(!b
            .check(
                "assistant",
                "org.freedesktop.systemd1.manage-units",
                Some("x.service"),
                110 + 5 * 60
            )
            .allowed());
    }

    #[test]
    fn ask_grant_is_single_use() {
        let mut b = broker(Level::Propose);
        let desc = Description {
            title: "install cowsay".into(),
            ..Default::default()
        };
        let Outcome::Pending { id } = b.ask("assistant", "org.tulsi.apply", desc, 0) else {
            panic!()
        };
        b.decide(id, true, None, "alice", 1).unwrap();
        assert!(b.check("assistant", "org.tulsi.apply", None, 2).allowed());
        assert!(!b.check("assistant", "org.tulsi.apply", None, 3).allowed());
    }

    #[test]
    fn rejected_gives_no_grant_and_cannot_be_redecided() {
        let mut b = broker(Level::Propose);
        let Outcome::Pending { id } = b.check("assistant", "org.tulsi.apply", None, 0) else {
            panic!()
        };
        b.decide(id, false, None, "alice", 1).unwrap();
        assert_eq!(
            b.decide(id, true, None, "alice", 2),
            Err(Error::NotPending(id))
        );
        assert!(b.grants.is_empty());
    }

    #[test]
    fn levels_allow_without_asking() {
        let mut b = broker(Level::Routine);
        assert!(b
            .check("assistant", "org.tulsi.extensions", None, 0)
            .allowed());
        assert!(matches!(
            b.check("assistant", "org.tulsi.apply", None, 0),
            Outcome::Pending { .. }
        ));
        let mut b = broker(Level::Administer);
        assert!(b.check("assistant", "org.tulsi.apply", None, 0).allowed());
        assert!(matches!(
            b.ask("assistant", "org.tulsi.apply", Description::default(), 0),
            Outcome::Allow { .. }
        ));
    }

    #[test]
    fn forbidden_even_at_delegate_and_never_asked() {
        let mut b = broker(Level::Delegate);
        for action in ["org.freedesktop.policykit.exec", "org.starah.configure"] {
            assert!(matches!(
                b.check("assistant", action, None, 0),
                Outcome::Deny { .. }
            ));
        }
        assert!(b.pending().is_empty());
    }

    #[test]
    fn observe_denies_and_files_nothing() {
        let mut b = broker(Level::Observe);
        assert!(matches!(
            b.check("assistant", "org.tulsi.extensions", None, 0),
            Outcome::Deny { .. }
        ));
        assert!(b.pending().is_empty());
    }

    #[test]
    fn home_access_is_per_agent_and_person() {
        let mut b = broker(Level::Propose);
        assert!(!b.home_access("assistant", "alice"));
        b.set_home_access("assistant", "alice", true).unwrap();
        assert!(b.home_access("assistant", "alice") && !b.home_access("assistant", "bob"));
        b.set_home_access("assistant", "alice", false).unwrap();
        assert!(!b.home_access("assistant", "alice") && b.policy.home_access.is_empty());
        assert!(b.set_home_access("nobody", "alice", true).is_err());
    }

    #[test]
    fn requests_expire() {
        let mut b = broker(Level::Propose);
        let Outcome::Pending { id } = b.check("assistant", "org.tulsi.apply", None, 0) else {
            panic!()
        };
        assert_eq!(b.expire(REQUEST_TTL), vec![id]);
        assert_eq!(
            b.decide(id, true, None, "alice", REQUEST_TTL + 1),
            Err(Error::NotPending(id))
        );
    }

    #[test]
    fn set_levels_marks_configured_and_drops_grants() {
        let mut b = broker(Level::Propose);
        assert!(!b.policy.configured);
        let Outcome::Pending { id } = b.check("assistant", "org.tulsi.apply", None, 0) else {
            panic!()
        };
        b.decide(id, true, None, "alice", 1).unwrap();
        let mut levels = BTreeMap::new();
        levels.insert("assistant".to_string(), Level::Observe);
        b.set_levels(levels, "alice", 2).unwrap();
        assert!(b.policy.configured);
        assert_eq!(b.level("assistant"), Some(Level::Observe));
        assert!(!b.check("assistant", "org.tulsi.apply", None, 3).allowed());
        let mut bad = BTreeMap::new();
        bad.insert("ghost".to_string(), Level::Delegate);
        assert_eq!(
            b.set_levels(bad, "alice", 4),
            Err(Error::UnknownAgent("ghost".into()))
        );
    }

    #[test]
    fn level_json_is_a_number() {
        assert_eq!(serde_json::to_string(&Level::Administer).unwrap(), "3");
        assert!(serde_json::from_str::<Level>("7").is_err());
    }

    // ---------------------------------------------------------------- rules

    fn with_rules(level: Level, src: &str) -> Broker {
        let mut b = broker(level);
        b.rules = Some(Rules::compile(src).unwrap());
        b
    }

    #[test]
    fn rules_refine_the_table() {
        let src = r#"fn decide(req) {
            if req.action == "org.tulsi.apply" { return "allow"; }
            if req.action == "org.tulsi.extensions" { return "deny"; }
            "default"
        }"#;
        let mut b = with_rules(Level::Propose, src);
        // Admin at level 1 would ask; the rules allow it.
        let out = b.check("assistant", "org.tulsi.apply", None, 0);
        assert_eq!(
            out,
            Outcome::Allow {
                why: "your rules allow this".into()
            }
        );
        // Routine at level 1 would ask; the rules deny it.
        assert!(matches!(
            b.check("assistant", "org.tulsi.extensions", None, 0),
            Outcome::Deny { .. }
        ));
        // Everything else: the table (ask).
        assert!(matches!(
            b.check(
                "assistant",
                "org.freedesktop.systemd1.manage-units",
                None,
                0
            ),
            Outcome::Pending { .. }
        ));
    }

    #[test]
    fn rules_never_open_the_forbidden_class() {
        let mut b = with_rules(Level::Delegate, r#"fn decide(req) { "allow" }"#);
        assert!(matches!(
            b.check("assistant", "org.freedesktop.policykit.exec", None, 0),
            Outcome::Deny { .. }
        ));
        assert!(matches!(
            b.check("assistant", "org.starah.configure", None, 0),
            Outcome::Deny { .. }
        ));
    }

    #[test]
    fn failing_rules_count_as_the_stricter_of_the_table_and_ask() {
        let mut b = with_rules(Level::Delegate, "fn decide(req) { loop {} }");
        // The table would allow: a failure asks.
        assert!(matches!(
            b.check("assistant", "org.tulsi.apply", None, 0),
            Outcome::Pending { .. }
        ));
        assert!(b.rules_failure.take().unwrap().contains("operations"));
        let mut b = with_rules(Level::Observe, "fn decide(req) { 1/0 }");
        // The table would deny: still deny.
        assert!(matches!(
            b.check("assistant", "org.tulsi.apply", None, 0),
            Outcome::Deny { .. }
        ));
    }

    #[test]
    fn rules_see_what_a_cooperating_agent_asks() {
        let src = r#"fn decide(req) {
            if req.changes.len() > 0 && req.changes.all(|c| c.starts_with("+ package")) { "allow" } else { "default" }
        }"#;
        let mut b = with_rules(Level::Propose, src);
        let installs = Description {
            title: "Install vlc".into(),
            changes: vec!["+ package vlc".into()],
            ..Default::default()
        };
        assert!(b.ask("assistant", "org.tulsi.apply", installs, 0).allowed());
        let other = Description {
            changes: vec!["- package vlc".into()],
            ..Default::default()
        };
        assert!(matches!(
            b.ask("assistant", "org.tulsi.apply", other, 0),
            Outcome::Pending { .. }
        ));
    }

    #[test]
    fn evaluate_files_nothing() {
        let mut b = broker(Level::Propose);
        let d = b
            .evaluate(
                "assistant",
                "org.tulsi.apply",
                None,
                &Description::default(),
                0,
            )
            .unwrap();
        assert_eq!(d.verdict, Verdict::Ask);
        assert!(!d.by_rules);
        assert!(b.pending().is_empty());
        assert!(b
            .evaluate("nobody", "x", None, &Description::default(), 0)
            .is_none());
    }
}
