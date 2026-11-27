//! `starah rules …`: the person's own rules (a Rhai script, see starah-policy).
//!
//!   starah rules show                 the rules and whether they are in use
//!   starah rules check [FILE]         whether FILE (or the rules) compile, and what
//!                                     they decide for each agent on common actions
//!   starah rules edit                 edit in $VISUAL/$EDITOR; checked before they
//!                                     are used (like visudo); asks the admin password
//!   starah rules undo                 back to the rules before the last change
//!   starah rules test AGENT ACTION [--detail D] [--title T] [--change C]…
//!                                     what the level and the rules decide

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde_json::{json, Value};

use crate::proxy::Broker1Proxy;

type Res = Result<ExitCode, String>;

fn err(e: zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(_, Some(msg), _) => msg,
        other => other.to_string(),
    }
}

fn status_line(status: &Value) -> String {
    match status["state"].as_str() {
        Some("ok") if status["active"] == true => format!(
            "in use (version {})",
            status["hash"].as_str().unwrap_or("?")
        ),
        Some("ok") => "no decide() yet: the levels alone decide".into(),
        Some("error") => format!(
            "the file has an error ({}); the rules before it are still in use",
            error_text(&status["error"])
        ),
        _ => "none: the levels alone decide".into(),
    }
}

fn error_text(e: &Value) -> String {
    let msg = e["message"].as_str().unwrap_or("?");
    match (e["line"].as_u64(), e["column"].as_u64()) {
        (Some(l), Some(c)) => format!("line {l}, column {c}: {msg}"),
        (Some(l), None) => format!("line {l}: {msg}"),
        _ => msg.to_string(),
    }
}

pub async fn run(broker: &Broker1Proxy<'_>, args: &[String]) -> Res {
    let strs: Vec<&str> = args.iter().map(String::as_str).collect();
    match strs.as_slice() {
        ["show"] => {
            let r: Value = serde_json::from_str(&broker.get_rules().await.map_err(err)?)
                .map_err(|e| e.to_string())?;
            print!("{}", r["text"].as_str().unwrap_or(""));
            eprintln!(
                "starah: {} — {}",
                r["path"].as_str().unwrap_or("rules.rhai"),
                status_line(&r["status"])
            );
            Ok(ExitCode::SUCCESS)
        }
        ["check"] | ["check", _] => {
            let text = match strs.get(1) {
                Some(file) => std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?,
                None => {
                    let r: Value = serde_json::from_str(&broker.get_rules().await.map_err(err)?)
                        .map_err(|e| e.to_string())?;
                    r["text"].as_str().unwrap_or("").to_string()
                }
            };
            let report: Value =
                serde_json::from_str(&broker.check_rules(&text).await.map_err(err)?)
                    .map_err(|e| e.to_string())?;
            print_report(&report);
            Ok(if report["ok"] == true {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        ["edit"] => edit(broker).await,
        // From the Edit Rules… button: the terminal closes when this ends, so a
        // failure waits for Enter (to be read).
        ["edit", "--pause-on-error"] => {
            let result = edit(broker).await;
            let failed = match &result {
                Ok(code) => *code != ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("starah: {e}");
                    true
                }
            };
            if failed {
                pause();
            }
            result.or(Ok(ExitCode::FAILURE))
        }
        ["undo"] => {
            let hash = broker.undo_rules().await.map_err(err)?;
            eprintln!("starah: the rules before are back (version {hash})");
            Ok(ExitCode::SUCCESS)
        }
        ["test", agent, action, rest @ ..] => {
            let (mut detail, mut title, mut changes) = ("", "", Vec::new());
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
                match *flag {
                    "--detail" => detail = value,
                    "--title" => title = value,
                    "--change" => changes.push(*value),
                    other => return Err(format!("unknown option {other}")),
                }
            }
            let description = json!({ "title": title, "changes": changes }).to_string();
            let d: Value = serde_json::from_str(
                &broker
                    .try_rules(agent, action, detail, &description)
                    .await
                    .map_err(err)?,
            )
            .map_err(|e| e.to_string())?;
            println!(
                "{}: {}",
                d["verdict"].as_str().unwrap_or("?"),
                d["why"].as_str().unwrap_or("")
            );
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            eprintln!(
                "usage: starah rules show | check [FILE] | edit | undo\n\
                 \x20      | test AGENT ACTION [--detail D] [--title T] [--change C]…"
            );
            Ok(ExitCode::from(2))
        }
    }
}

fn print_report(report: &Value) {
    if let Some(e) = report.get("error").filter(|e| !e.is_null()) {
        println!("The rules don't compile: {}", error_text(e));
        return;
    }
    if let Some(e) = report["runtime_error"].as_str() {
        println!("The rules fail when they run: {e}");
        println!("(A failing rule asks you instead of allowing.)");
    }
    let samples = report["samples"].as_array().cloned().unwrap_or_default();
    if !samples.is_empty() {
        println!("What they decide:");
        for s in samples {
            println!(
                "  {:<12} {:<55} {}",
                s["agent"].as_str().unwrap_or("?"),
                s["action"].as_str().unwrap_or("?"),
                s["verdict"].as_str().unwrap_or("?")
            );
        }
    }
}

/// The editor to use: $VISUAL, $EDITOR, then the usual ones.
fn editor() -> String {
    for var in ["VISUAL", "EDITOR"] {
        if let Ok(e) = std::env::var(var) {
            if !e.trim().is_empty() {
                return e;
            }
        }
    }
    ["fresh", "nano", "vi"]
        .iter()
        .find(|e| which(e).is_some())
        .unwrap_or(&"vi")
        .to_string()
}

fn which(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

/// A private copy to edit: $XDG_RUNTIME_DIR (or the temp dir)/starah-rules-PID/.
fn scratch() -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("starah-rules-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir.join("rules.rhai"))
}

fn run_editor(file: &Path) -> Result<(), String> {
    let editor = editor();
    // EDITOR may carry arguments ("code --wait").
    let status = Command::new("sh")
        .args(["-c", &format!("{editor} \"$1\""), "sh"])
        .arg(file)
        .status()
        .map_err(|e| format!("cannot start {editor}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{editor} exited with {status}"))
    }
}

fn pause() {
    print!("Press Enter to close. ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
}

fn ask_again(question: &str) -> bool {
    print!("{question} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
        return false;
    }
    !matches!(line.trim().to_lowercase().as_str(), "n" | "no")
}

/// A polkit agent on this terminal, for the password, when the desktop has none.
fn tty_auth_agent() -> Option<std::process::Child> {
    let agent = which("pkttyagent")?;
    Command::new(agent)
        .args(["--process", &std::process::id().to_string(), "--fallback"])
        .spawn()
        .ok()
}

async fn edit(broker: &Broker1Proxy<'_>) -> Res {
    let current: Value =
        serde_json::from_str(&broker.get_rules().await.map_err(err)?).map_err(|e| e.to_string())?;
    let original = current["text"].as_str().unwrap_or("").to_string();
    let file = scratch()?;
    std::fs::write(&file, &original).map_err(|e| format!("{}: {e}", file.display()))?;
    let cleanup = |f: &Path| {
        if let Some(dir) = f.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    };
    let result = async {
        loop {
            run_editor(&file)?;
            let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
            if text == original {
                eprintln!("starah: no change");
                return Ok(ExitCode::SUCCESS);
            }
            let report: Value =
                serde_json::from_str(&broker.check_rules(&text).await.map_err(err)?)
                    .map_err(|e| e.to_string())?;
            print_report(&report);
            if report["ok"] != true {
                if ask_again("Edit again?") {
                    continue;
                }
                eprintln!("starah: not saved; the rules are as they were");
                return Ok(ExitCode::FAILURE);
            }
            let agent = tty_auth_agent();
            let saved = broker.set_rules(&text).await;
            if let Some(mut a) = agent {
                let _ = a.kill();
                let _ = a.wait();
            }
            let hash = saved.map_err(err)?;
            eprintln!("starah: your rules are in use (version {hash})");
            return Ok(ExitCode::SUCCESS);
        }
    }
    .await;
    cleanup(&file);
    result
}
