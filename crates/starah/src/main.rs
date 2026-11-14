//! starah: the command-line side of Starah.
//!
//!   starah status                      agents and their levels
//!   starah set AGENT LEVEL [AGENT LEVEL…]   choose levels (asks for the admin password)
//!   starah requests                    pending requests from agents
//!   starah approve ID [MINUTES]        approve a request
//!   starah reject ID
//!   starah run --agent NAME -- CMD…    run CMD as the agent, sandboxed, with this stdio
//!   starah ask ACTION TITLE [CHANGE…]  (inside an agent) ask before acting; waits
//!   starah audit [N]                   the last N audit entries

mod proxy;

use std::process::ExitCode;

use futures_util::StreamExt;
use proxy::Broker1Proxy;
use serde_json::{json, Value};
use zbus::zvariant::Fd;

fn usage() -> ExitCode {
    eprintln!(
        "usage: starah status | set AGENT LEVEL… | requests | approve ID [MINUTES] | reject ID\n\
         \x20      | run --agent NAME -- CMD… | ask ACTION TITLE [CHANGE…] | audit [N]"
    );
    ExitCode::from(2)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("starah: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &[String]) -> Result<ExitCode, String> {
    let Some((cmd, rest)) = args.split_first() else {
        return Ok(usage());
    };
    let conn = zbus::Connection::system()
        .await
        .map_err(|e| format!("cannot reach the system bus: {e}"))?;
    let broker = Broker1Proxy::new(&conn).await.map_err(|e| e.to_string())?;
    let err = |e: zbus::Error| match e {
        zbus::Error::MethodError(_, Some(msg), _) => msg,
        other => other.to_string(),
    };
    match (cmd.as_str(), rest) {
        ("status", []) => {
            let policy: Value =
                serde_json::from_str(&broker.get_policy().await.map_err(err)?).unwrap_or_default();
            println!(
                "Configured: {}",
                if policy["configured"] == true { "yes" } else { "no (first-login setup pending)" }
            );
            for a in policy["agents"].as_array().into_iter().flatten() {
                println!(
                    "{:<16} level {} ({})  {}",
                    a["name"].as_str().unwrap_or("?"),
                    a["level"],
                    a["level_name"].as_str().unwrap_or("?"),
                    a["description"].as_str().unwrap_or("")
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        ("set", pairs) if !pairs.is_empty() && pairs.len() % 2 == 0 => {
            let mut levels = serde_json::Map::new();
            for pair in pairs.chunks(2) {
                let level: u8 = pair[1]
                    .parse()
                    .map_err(|_| format!("level must be 0–4, not {:?}", pair[1]))?;
                levels.insert(pair[0].clone(), json!(level));
            }
            broker
                .set_policy(&Value::Object(levels).to_string())
                .await
                .map_err(err)?;
            Ok(ExitCode::SUCCESS)
        }
        ("requests", []) => {
            let list: Value =
                serde_json::from_str(&broker.requests().await.map_err(err)?).unwrap_or_default();
            for r in list.as_array().into_iter().flatten() {
                let what = r["description"]["title"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        format!(
                            "{}{}",
                            r["action"].as_str().unwrap_or("?"),
                            r["detail"].as_str().map(|d| format!(" ({d})")).unwrap_or_default()
                        )
                    });
                println!(
                    "{}\t{}\t{}\t{}",
                    r["id"],
                    r["agent"].as_str().unwrap_or("?"),
                    r["class"].as_str().unwrap_or("?"),
                    what
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        ("approve", [id]) | ("approve", [id, _]) => {
            let id: u64 = id.parse().map_err(|_| "ID must be a number")?;
            let minutes: u32 = rest.get(1).and_then(|m| m.parse().ok()).unwrap_or(0);
            broker.decide(id, true, minutes).await.map_err(err)?;
            Ok(ExitCode::SUCCESS)
        }
        ("reject", [id]) => {
            let id: u64 = id.parse().map_err(|_| "ID must be a number")?;
            broker.decide(id, false, 0).await.map_err(err)?;
            Ok(ExitCode::SUCCESS)
        }
        ("run", rest) => {
            let (agent, cmd) = match rest {
                [flag, agent, sep, cmd @ ..] if flag == "--agent" && sep == "--" && !cmd.is_empty() => {
                    (agent, cmd)
                }
                _ => return Ok(usage()),
            };
            let mut argv = cmd.to_vec();
            argv[0] = resolve(&argv[0]).ok_or_else(|| format!("{}: command not found", argv[0]))?;
            let (stdin, stdout, stderr) = (std::io::stdin(), std::io::stdout(), std::io::stderr());
            let unit = broker
                .run(agent, &argv, Fd::from(&stdin), Fd::from(&stdout), Fd::from(&stderr))
                .await
                .map_err(err)?;
            let status = broker.wait(&unit).await.map_err(err)?;
            Ok(ExitCode::from(status.clamp(0, 255) as u8))
        }
        ("ask", [action, title, changes @ ..]) => {
            let description = json!({ "title": title, "changes": changes }).to_string();
            let (code, msg) = ask(&broker, action, &description).await.map_err(err)?;
            eprintln!("starah: {msg}");
            Ok(code)
        }
        ("audit", rest) if rest.len() <= 1 => {
            let n: u32 = rest.first().and_then(|n| n.parse().ok()).unwrap_or(20);
            println!("{}", broker.audit_log(n).await.map_err(err)?);
            Ok(ExitCode::SUCCESS)
        }
        _ => Ok(usage()),
    }
}

/// Asks and, when the request is pending, waits for the human's decision.
async fn ask(
    broker: &Broker1Proxy<'_>,
    action: &str,
    description: &str,
) -> zbus::Result<(ExitCode, String)> {
    // Subscribe first, so a quick decision is not missed.
    let mut decided = broker.receive_decided().await?;
    let (id, status, msg) = broker.ask(action, description).await?;
    match status.as_str() {
        "approved" => return Ok((ExitCode::SUCCESS, msg)),
        "rejected" => return Ok((ExitCode::FAILURE, msg)),
        _ => eprintln!("starah: {msg}"),
    }
    while let Some(signal) = decided.next().await {
        let a = signal.args()?;
        if a.id == id {
            return Ok(if a.approved {
                (ExitCode::SUCCESS, format!("request {id} approved"))
            } else {
                (ExitCode::FAILURE, format!("request {id} {}", a.status))
            });
        }
    }
    Ok((ExitCode::FAILURE, "lost the connection to starahd".into()))
}

fn resolve(program: &str) -> Option<String> {
    // Absolute, but not canonicalized: multi-call binaries (coreutils, busybox) pick
    // their behaviour from the name they are started under.
    if program.contains('/') {
        let p = std::path::absolute(program).ok()?;
        return p.is_file().then(|| p.to_string_lossy().into_owned());
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join(program))
        .find(|p| p.is_absolute() && p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}
