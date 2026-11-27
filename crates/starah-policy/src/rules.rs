//! The person's own rules: a Rhai script that refines the level table.
//!
//! The script defines `decide(req)`, called for each decision about an agent. It
//! returns `"allow"`, `"ask"` (the person approves, as usual), `"deny"`, or
//! `"default"` (the level table decides). `req` is a map:
//!
//! | field | example |
//! | --- | --- |
//! | `agent` | `"claude"` |
//! | `level`, `level_name` | `2`, `"Routine"` |
//! | `action` | `"org.tulsi.apply"` (a polkit action id) |
//! | `class` | `"observe"`, `"routine"`, `"admin"`, `"sensitive"` |
//! | `detail` | the systemd unit, or tulsi's summary (`""` when none) |
//! | `title`, `reason` | what a cooperating agent said it wants (`""` otherwise) |
//! | `changes` | its list of changes, e.g. `["+ package vlc"]` (`[]` otherwise) |
//! | `hour`, `weekday` | local time: `0`–`23`, `"mon"`…`"sun"` |
//! | `table` | what the level table says: `"allow"`, `"ask"` or `"deny"` |
//!
//! Limits that no script changes: the forbidden class (Starah itself, accounts,
//! polkit and sudo rules) is always denied; a script that fails or runs too long
//! counts as the stricter of the table's verdict and "ask". The engine has no
//! files, network, processes or `eval`, and a bounded number of operations.

use rhai::{Array, CallFnOptions, Dynamic, Engine, Map, Scope, AST};
use serde::Serialize;

use crate::{Class, Description, Level, Verdict};

/// What the script said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleVerdict {
    Allow,
    Ask,
    Deny,
    Default,
}

/// Why a script can't be used, and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RulesError {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl std::fmt::Display for RulesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.column) {
            (Some(l), Some(c)) => write!(f, "line {l}, column {c}: {}", self.message),
            (Some(l), None) => write!(f, "line {l}: {}", self.message),
            _ => write!(f, "{}", self.message),
        }
    }
}

/// Operations one decision may take (a loop that never ends stops here).
pub const MAX_OPERATIONS: u64 = 100_000;

/// The rules file a new system starts with: it explains, and changes nothing.
pub const EXAMPLE: &str = r#"// Starah rules: your own refinements of the agents' levels.
//
// Edit with `starah rules edit` (checks the script before it is used), or
// `sudoedit /etc/starah/rules.rhai`. Agent Privileges › Edit Rules… opens it.
//
// Define decide(req) and return "allow", "ask" (you approve, as usual), "deny",
// or "default" (the agent's level decides). req has: agent, level, level_name,
// action, class, detail, title, reason, changes, hour, weekday, table (what
// the level alone would say).
// Whatever you write, Starah never lets an agent change Starah itself, accounts,
// or polkit and sudo rules.
//
// Examples (remove the // to use them):
//
// fn decide(req) {
//     // Codex may install packages on its own, but nothing else without asking.
//     if req.agent == "codex" && req.action == "org.tulsi.apply"
//         && req.changes.len() > 0
//         && req.changes.all(|c| c.starts_with("+ package")) {
//         return "allow";
//     }
//     // No system administration by any agent at night.
//     if req.class == "admin" && (req.hour >= 22 || req.hour < 7) {
//         return "deny";
//     }
//     "default"
// }
"#;

/// What a decision is about, for the script.
pub struct RuleInput<'a> {
    pub agent: &'a str,
    pub level: Level,
    pub action: &'a str,
    pub class: Class,
    pub detail: Option<&'a str>,
    pub description: &'a Description,
    /// The level table's verdict.
    pub default: Verdict,
    /// Unix time, and the local time zone's offset from UTC in seconds.
    pub now: u64,
    pub utc_offset: i64,
}

pub struct Rules {
    engine: Engine,
    ast: AST,
    has_decide: bool,
    /// Identifies this version of the rules in the audit log.
    pub hash: String,
}

impl std::fmt::Debug for Rules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rules").field("hash", &self.hash).finish()
    }
}

fn engine() -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(MAX_OPERATIONS);
    e.set_max_call_levels(32);
    e.set_max_expr_depths(64, 32);
    e.set_max_string_size(64 * 1024);
    e.set_max_array_size(10_000);
    e.set_max_map_size(1_000);
    e.disable_symbol("eval");
    e.on_print(|_| {});
    e.on_debug(|_, _, _| {});
    e
}

/// FNV-1a, hex: enough to tell versions apart in the audit log.
pub fn hash(source: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in source.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn class_name(c: Class) -> &'static str {
    match c {
        Class::Observe => "observe",
        Class::Routine => "routine",
        Class::Admin => "admin",
        Class::Sensitive => "sensitive",
        Class::Forbidden => "forbidden",
    }
}

fn verdict_name(v: Verdict) -> &'static str {
    match v {
        Verdict::Allow => "allow",
        Verdict::Ask => "ask",
        Verdict::Deny => "deny",
    }
}

/// Local hour (0–23) and weekday ("mon"…"sun").
pub fn local_time(now: u64, utc_offset: i64) -> (i64, &'static str) {
    let t = now as i64 + utc_offset;
    let hour = t.rem_euclid(86_400) / 3_600;
    // 1970-01-01 was a Thursday.
    let day = (t.div_euclid(86_400) + 3).rem_euclid(7);
    let weekday = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"][day as usize];
    (hour, weekday)
}

impl RuleInput<'_> {
    fn to_map(&self) -> Map {
        let s = |v: &str| Dynamic::from(v.to_string());
        let (hour, weekday) = local_time(self.now, self.utc_offset);
        let mut m = Map::new();
        m.insert("agent".into(), s(self.agent));
        m.insert("level".into(), Dynamic::from(self.level as u8 as i64));
        m.insert("level_name".into(), s(self.level.name()));
        m.insert("action".into(), s(self.action));
        m.insert("class".into(), s(class_name(self.class)));
        m.insert("detail".into(), s(self.detail.unwrap_or("")));
        m.insert("title".into(), s(&self.description.title));
        m.insert("reason".into(), s(&self.description.reason));
        let changes: Array = self.description.changes.iter().map(|c| s(c)).collect();
        m.insert("changes".into(), Dynamic::from(changes));
        m.insert("hour".into(), Dynamic::from(hour));
        m.insert("weekday".into(), s(weekday));
        m.insert("table".into(), s(verdict_name(self.default)));
        m
    }
}

impl Rules {
    /// Compiles a rules script. A script without `decide` is valid and changes
    /// nothing (the example file is all comments).
    pub fn compile(source: &str) -> Result<Rules, RulesError> {
        let engine = engine();
        let ast = engine.compile(source).map_err(|e| RulesError {
            message: e.0.to_string(),
            line: e.1.line(),
            column: e.1.position(),
        })?;
        let mut has_decide = false;
        for f in ast.iter_functions() {
            if f.name == "decide" {
                if f.params.len() != 1 {
                    return Err(RulesError {
                        message: "decide must take one argument, the request: fn decide(req)"
                            .into(),
                        line: None,
                        column: None,
                    });
                }
                has_decide = true;
            }
        }
        Ok(Rules {
            engine,
            ast,
            has_decide,
            hash: hash(source),
        })
    }

    pub fn has_decide(&self) -> bool {
        self.has_decide
    }

    /// Runs `decide(req)`. An error says why the script failed (it then counts as
    /// the stricter of the table's verdict and "ask"; see `Broker`).
    pub fn decide(&self, input: &RuleInput) -> Result<RuleVerdict, String> {
        if !self.has_decide {
            return Ok(RuleVerdict::Default);
        }
        let mut scope = Scope::new();
        let options = CallFnOptions::new().eval_ast(false);
        let out: Dynamic = self
            .engine
            .call_fn_with_options(options, &mut scope, &self.ast, "decide", (input.to_map(),))
            .map_err(|e| e.to_string())?;
        let text = out.clone().into_string().map_err(|t| {
            format!("decide returned {t} {out}, not \"allow\", \"ask\", \"deny\" or \"default\"")
        })?;
        match text.as_str() {
            "allow" => Ok(RuleVerdict::Allow),
            "ask" => Ok(RuleVerdict::Ask),
            "deny" => Ok(RuleVerdict::Deny),
            "default" => Ok(RuleVerdict::Default),
            other => Err(format!(
                "decide returned \"{other}\", not \"allow\", \"ask\", \"deny\" or \"default\""
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(
        agent: &'a str,
        action: &'a str,
        class: Class,
        d: &'a Description,
    ) -> RuleInput<'a> {
        RuleInput {
            agent,
            level: Level::Propose,
            action,
            class,
            detail: None,
            description: d,
            default: Verdict::Ask,
            // 2026-11-26 (a Thursday) 23:30 UTC.
            now: 1_795_735_800,
            utc_offset: 0,
        }
    }

    #[test]
    fn the_example_compiles_and_changes_nothing() {
        let r = Rules::compile(EXAMPLE).unwrap();
        assert!(!r.has_decide());
        let d = Description::default();
        assert_eq!(
            r.decide(&input("x", "a", Class::Admin, &d)).unwrap(),
            RuleVerdict::Default
        );
    }

    #[test]
    fn the_example_works_uncommented() {
        let src: String = EXAMPLE
            .lines()
            .filter_map(|l| l.strip_prefix("// "))
            .skip_while(|l| !l.starts_with("fn decide"))
            .collect::<Vec<_>>()
            .join("\n");
        let r = Rules::compile(&src).unwrap();
        let installs = Description {
            changes: vec!["+ package vlc".into(), "+ package gimp".into()],
            ..Default::default()
        };
        let other = Description {
            changes: vec!["+ package vlc".into(), "~ service sshd".into()],
            ..Default::default()
        };
        let d = Description::default();
        assert_eq!(
            r.decide(&input(
                "codex",
                "org.tulsi.apply",
                Class::Routine,
                &installs
            ))
            .unwrap(),
            RuleVerdict::Allow
        );
        assert_eq!(
            r.decide(&input("codex", "org.tulsi.apply", Class::Routine, &other))
                .unwrap(),
            RuleVerdict::Default
        );
        // 23:30: night.
        assert_eq!(
            r.decide(&input("claude", "x", Class::Admin, &d)).unwrap(),
            RuleVerdict::Deny
        );
        assert_eq!(
            r.decide(&input("claude", "x", Class::Routine, &d)).unwrap(),
            RuleVerdict::Default
        );
    }

    #[test]
    fn syntax_errors_say_where() {
        let e = Rules::compile("fn decide(req) {\n  if req.agent == \n}").unwrap_err();
        assert_eq!(e.line, Some(3), "{e}");
    }

    #[test]
    fn decide_must_take_the_request() {
        assert!(Rules::compile("fn decide() { \"allow\" }").is_err());
    }

    #[test]
    fn wrong_answers_and_runtime_errors_are_errors() {
        let d = Description::default();
        let i = input("x", "a", Class::Admin, &d);
        assert!(Rules::compile("fn decide(req) { \"yes\" }")
            .unwrap()
            .decide(&i)
            .is_err());
        assert!(Rules::compile("fn decide(req) { 42 }")
            .unwrap()
            .decide(&i)
            .is_err());
        assert!(Rules::compile("fn decide(req) { req.nope.len() }")
            .unwrap()
            .decide(&i)
            .is_err());
    }

    #[test]
    fn endless_loops_stop() {
        let d = Description::default();
        let r = Rules::compile("fn decide(req) { loop { } }").unwrap();
        let e = r.decide(&input("x", "a", Class::Admin, &d)).unwrap_err();
        assert!(e.contains("operations"), "{e}");
    }

    #[test]
    fn no_eval() {
        assert!(Rules::compile("fn decide(req) { eval(\"\\\"allow\\\"\") }").is_err());
    }

    #[test]
    fn the_request_has_its_fields() {
        let d = Description {
            title: "Install vlc".into(),
            ..Default::default()
        };
        let r = Rules::compile(
            r#"fn decide(req) {
                if req.title == "Install vlc" && req.level == 1 && req.level_name == "Propose"
                   && req.class == "admin" && req.weekday == "thu" && req.hour == 23
                   && req.table == "ask" && req.detail == "" { "allow" } else { "deny" }
            }"#,
        )
        .unwrap();
        assert_eq!(
            r.decide(&input("x", "a", Class::Admin, &d)).unwrap(),
            RuleVerdict::Allow
        );
    }

    #[test]
    fn local_time_with_an_offset() {
        // 23:30 UTC on a Thursday is 00:30 on Friday at UTC+1.
        assert_eq!(local_time(1_795_735_800, 3_600), (0, "fri"));
        assert_eq!(local_time(0, 0), (0, "thu"));
    }

    #[test]
    fn the_hash_tells_versions_apart() {
        assert_ne!(hash("a"), hash("b"));
        assert_eq!(hash("a").len(), 16);
    }
}
