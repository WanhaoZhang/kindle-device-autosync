use anyhow::{Context, Result, bail};
use ksync_worker::*;
use serde_json::json;
use std::path::{Path, PathBuf};
fn run() -> Result<serde_json::Value> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|s| s == "--help" || s == "-h") {
        println!(
            "ksync-worker --root DIR self-test [--online-self-test]|collect|compact|sync|status|reconcile --plan-id ID|import-config --inbox DIR|export --out DIR\nDefault sync collects only. Manual trial: sync --trial --prepare --book-id KEY --max-seconds 60; then sync --trial --confirm --plan-id ID. Formal acceptance or explicit experimental trial authorization is required. Preview: operate --inbox DIR --action authorize-trial|prepare|confirm|enable-experimental|disable-experimental|authorize-prepare|confirm-enable|adopt-and-enable."
        );
        return Ok(json!({"state":"help"}));
    }
    let value = |flag: &str| -> Result<String> {
        let i = args
            .iter()
            .position(|s| s == flag)
            .context("required argument missing")?;
        args.get(i + 1).cloned().context("missing argument value")
    };
    let root =
        PathBuf::from(value("--root").unwrap_or_else(|_| "/var/local/kindle-native-sync".into()));
    let command = args
        .iter()
        .find(|s| {
            [
                "self-test",
                "compact",
                "collect",
                "sync",
                "status",
                "import-config",
                "import-book-candidates",
                "export",
                "reconcile",
                "operate",
                "experimental-authorize-trial",
                "experimental-enable",
                "experimental-disable",
                "resume-isolated-autosync",
            ]
            .contains(&s.as_str())
        })
        .context("unknown command")?
        .as_str();
    if command == "self-test" {
        return Ok(self_test(
            &root,
            args.iter().any(|v| v == "--online-self-test"),
        ));
    }
    let store = Store::open(&root)?;
    match command {
        "status" => store.status(),
        "resume-isolated-autosync" => experimental::resume_isolated_autosync(&store),
        "operate" => {
            experimental::operate(&store, Path::new(&value("--inbox")?), &value("--action")?)
        }
        "experimental-authorize-trial" => experimental::authorize_trial(
            &root,
            &Config::load(&root)?,
            args.iter().any(|v| v == "--accept-unverified-performance"),
        ),
        "experimental-enable" => {
            manual::gate_manual(&root, true)?;
            experimental::enable(
                &store,
                &Config::load(&root)?,
                &value("--plan-id")?,
                args.iter().any(|v| v == "--accept-unverified-protocol"),
            )
        }
        "experimental-disable" => experimental::disable(&root),
        "compact" => compact(&store),
        "reconcile" => {
            manual::reconcile_manual(&store, &value("--plan-id")?, &mut network::Live::default())
        }
        "import-book-candidates" => book_mappings::import(&store, Path::new(&value("--inbox")?)),
        "import-config" => import_config(&store, Path::new(&value("--inbox")?)),
        "export" => export(&store, Path::new(&value("--out")?)),
        "collect" => parsing::collect(
            &store,
            Path::new("/mnt/us/documents"),
            Path::new("/var/local/metadata/cc.db"),
        ),
        "sync" => {
            let trial = args.iter().any(|s| s == "--trial");
            if trial {
                if args.iter().any(|s| s == "--prepare") {
                    let book = value("--book-id")?;
                    let max = value("--max-seconds")?.parse()?;
                    return manual::prepare_manual(&store, Some(&book), true, max);
                }
                if args.iter().any(|s| s == "--confirm") {
                    let id = value("--plan-id")?;
                    return manual::commit_manual(&store, &id, true, &mut network::Live::default());
                }
                bail!("trial requires explicit prepare or exact plan confirmation")
            }
            let collected = parsing::collect(
                &store,
                Path::new("/mnt/us/documents"),
                Path::new("/var/local/metadata/cc.db"),
            )?;
            if !root.join("config.json").exists() {
                return Ok(collected);
            }
            let mut result = drain(&store, &mut network::Live::default())?;
            result["collected"] = collected;
            if result["confirmed_batches"].as_u64().unwrap_or(0) > 0 {
                result["compaction"] =
                    compact(&store).unwrap_or_else(|_| json!({"state":"retained"}));
            }
            Ok(result)
        }
        _ => bail!("unknown command"),
    }
}
fn main() {
    install_signal_handlers();
    match run() {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
            let local_failed = v["private_filesystem_ok"] == false;
            let online_failed = v["network_contacted"] == true
                && (v["online_probe"]["state"] != "completed"
                    || v["online_probe"]["https_certificate_validated"] != true);
            let configured_online_proof_failed = v["network_contacted"] == true
                && v["tls_evidence_required"] == true
                && v["tls_evidence_saved"] != true;
            if local_failed || online_failed || configured_online_proof_failed {
                std::process::exit(1);
            }
        }
        Err(e) => {
            if e.downcast_ref::<rusqlite::Error>()
                .is_some_and(|v| v.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull))
            {
                eprintln!("worker stopped: storage_blocked: SQLite ledger hard limit reached");
            } else {
                eprintln!("worker stopped: {e}");
            }
            std::process::exit(1)
        }
    }
}
