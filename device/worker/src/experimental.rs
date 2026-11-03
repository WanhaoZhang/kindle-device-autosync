//! Explicit preview authorization. Does not manufacture formal acceptance.
use crate::*;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::Path, sync::OnceLock};

const TLS_PROOF: &str = "validation-tls.json";
const AUTHORIZATION: &str = "experimental-authorization.json";
// An explicit compatibility bridge for the shipped build which performed the
// first real trial. This is not a general binary-identity bypass.
const LEGACY_TRIAL_WORKER: &str =
    "2430ce27e9150ebd06fa99f34d97f1472261d71ed1e3ea2c7512efab35075420";
const LEGACY_PROTOCOL_SOURCE: &str =
    "233bc9ccdd944ef9bc9f0e96ec37f8af028711f7cf4587b00de618b61810dc1e";
const LEGACY_NETWORK_SOURCE: &str =
    "1cc550bab418ae47d30b0c987cc9e4281538d17a65bb5a14de3bfaa9404cda73";
static EXECUTABLE_SHA: OnceLock<String> = OnceLock::new();
fn executable_sha() -> Result<String> {
    if let Some(v) = EXECUTABLE_SHA.get() {
        return Ok(v.clone());
    }
    let mut input = fs::File::open(std::env::current_exe()?)?;
    let mut digest = Sha256::new();
    let mut b = [0u8; 16384];
    loop {
        let n = input.read(&mut b)?;
        if n == 0 {
            break;
        }
        digest.update(&b[..n]);
    }
    let v = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let _ = EXECUTABLE_SHA.set(v.clone());
    Ok(v)
}
fn actual_supported(probe: &Value) -> Result<()> {
    ensure!(
        probe["architecture"] == "arm"
            && probe["supported_hardware"] == true
            && probe["supported_firmware"] == true
            && probe["root"] == true,
        "preview requires actual ARM PW2 firmware/root"
    );
    ensure!(
        probe["kernel_release"]
            .as_str()
            .is_some_and(|v| !v.is_empty() && v.len() <= 128),
        "actual kernel release unavailable"
    );
    Ok(())
}
fn fingerprint(c: &Config, probe: &Value) -> Result<Value> {
    actual_supported(probe)?;
    Ok(
        json!({"schema_version":1,"config_binding_sha256":c.binding()?,"kernel_release":probe["kernel_release"],"worker_sha256":executable_sha()?}),
    )
}
/// Called only after the live anonymous CA/hostname-verified probe completed.
/// No handoff file can set this proof through import-config.
pub fn record_live_tls(root: &Path, c: &Config, probe: &Value, result: &Value) -> Result<()> {
    ensure!(
        result["state"] == "completed"
            && result["https_certificate_validated"] == true
            && result["credentials_sent"] == false
            && result["network_writes"] == false,
        "TLS probe did not complete safely"
    );
    let mut proof = fingerprint(c, probe)?;
    proof["live_tls_verified"] = json!(true);
    proof["observed_unix"] = json!(now());
    atomic_json(&root.join(TLS_PROOF), &proof)
}
fn check_tls(root: &Path, c: &Config, probe: &Value) -> Result<Value> {
    let wanted = fingerprint(c, probe)?;
    let proof = private_json(&root.join(TLS_PROOF))?;
    for key in [
        "schema_version",
        "config_binding_sha256",
        "kernel_release",
        "worker_sha256",
    ] {
        ensure!(
            proof[key] == wanted[key],
            "TLS proof belongs to another config/kernel/worker"
        );
    }
    ensure!(
        proof["live_tls_verified"] == true
            && proof["observed_unix"]
                .as_i64()
                .is_some_and(|at| at > 0 && at <= now() + 5),
        "invalid live TLS proof"
    );
    Ok(wanted)
}
fn checked_authorization(root: &Path, c: &Config, probe: &Value, automatic: bool) -> Result<Value> {
    let wanted = check_tls(root, c, probe)?;
    let proof = private_json(&root.join(AUTHORIZATION))?;
    for key in [
        "schema_version",
        "config_binding_sha256",
        "kernel_release",
        "worker_sha256",
    ] {
        ensure!(
            proof[key] == wanted[key],
            "preview authorization binding changed"
        );
    }
    ensure!(
        proof["accept_unverified_performance"] == true
            && proof["formal_device_validation_claimed"] == false
            && proof["formal_protocol_validation_claimed"] == false,
        "explicit preview acceptance missing"
    );
    ensure!(
        proof["mode"] == "trial" || proof["mode"] == "auto",
        "invalid preview mode"
    );
    if automatic {
        ensure!(
            proof["mode"] == "auto" && proof["accept_unverified_protocol"] == true,
            "preview automatic sync not enabled"
        );
    }
    Ok(proof)
}
pub fn preview_gate(root: &Path, c: &Config, automatic: bool) -> Result<()> {
    checked_authorization(root, c, &actual_device(), automatic)?;
    Ok(())
}
/// Formal acceptance remains distinct from a user's explicit experimental use.
pub fn write_authorized(root: &Path, c: &Config, trial: bool) -> bool {
    let formal = c.validation.device_validation
        && c.validation.binding_sha256 == c.binding().unwrap_or_default()
        && (trial || c.validation.protocol_validation);
    formal || preview_gate(root, c, !trial).is_ok()
}
pub fn authorize_trial(root: &Path, c: &Config, accept: bool) -> Result<Value> {
    authorize_trial_with_probe(root, c, accept, &actual_device())
}
fn authorize_trial_with_probe(
    root: &Path,
    c: &Config,
    accept: bool,
    probe: &Value,
) -> Result<Value> {
    ensure!(
        accept,
        "acceptance of unverified performance must be explicit"
    );
    Auth::load(root, c)?;
    let mut proof = check_tls(root, c, probe)?;
    proof["mode"] = json!("trial");
    proof["accept_unverified_performance"] = json!(true);
    proof["accept_unverified_protocol"] = json!(false);
    proof["formal_device_validation_claimed"] = json!(false);
    proof["formal_protocol_validation_claimed"] = json!(false);
    proof["created_unix"] = json!(now());
    atomic_json(&root.join(AUTHORIZATION), &proof)?;
    Ok(
        json!({"state":"experimental_trial_authorized","automatic_upload_enabled":false,"formal_validation_unchanged":true,"binding_sha256":c.binding()?}),
    )
}
pub fn bind_trial_plan(root: &Path, c: &Config, plan: &mut Value) {
    if let Ok(proof) = checked_authorization(root, c, &actual_device(), false) {
        plan["preview_worker_sha256"] = proof["worker_sha256"].clone();
        plan["preview_authorization_created_unix"] = proof["created_unix"].clone();
    }
}

pub fn verify_preview_trial_plan(root: &Path, c: &Config, plan: &Value) -> Result<()> {
    if c.validation.device_validation && c.validation.binding_sha256 == c.binding()? {
        return Ok(());
    }
    let proof = checked_authorization(root, c, &actual_device(), false)?;
    ensure!(
        plan["preview_worker_sha256"] == proof["worker_sha256"]
            && plan["preview_authorization_created_unix"] == proof["created_unix"],
        "trial authorization changed after preview"
    );
    Ok(())
}

fn unchanged_legacy_wire() -> Result<()> {
    ensure!(
        sha(include_bytes!("protocol.rs")) == LEGACY_PROTOCOL_SOURCE
            && sha(include_bytes!("network.rs")) == LEGACY_NETWORK_SOURCE,
        "legacy trial adoption requires byte-identical protocol and transport sources"
    );
    Ok(())
}
fn legacy_authorization(c: &Config, probe: &Value, previous: &Value) -> Result<()> {
    actual_supported(probe)?;
    unchanged_legacy_wire()?;
    ensure!(
        previous["schema_version"] == 1
            && previous["config_binding_sha256"] == c.binding()?
            && previous["kernel_release"] == probe["kernel_release"]
            && previous["worker_sha256"] == LEGACY_TRIAL_WORKER
            && previous["accept_unverified_performance"] == true
            && previous["formal_device_validation_claimed"] == false
            && previous["formal_protocol_validation_claimed"] == false
            && (previous["mode"] == "trial" || previous["mode"] == "auto")
            && previous["created_unix"]
                .as_i64()
                .is_some_and(|t| t > 0 && t <= now() + 5),
        "legacy authorization does not match the actual installation"
    );
    Ok(())
}
fn trial_authorization_matches(
    c: &Config,
    plan: &Value,
    receipt: &Value,
    auth: &Value,
) -> Result<()> {
    if plan["preview_worker_sha256"] == auth["worker_sha256"]
        && plan["preview_authorization_created_unix"] == auth["created_unix"]
    {
        return Ok(());
    }
    unchanged_legacy_wire()?;
    let proof = &auth["trial_adoption"];
    let previous = &proof["prior_authorization"];
    ensure!(
        proof["schema_version"] == 1
            && proof["kind"] == "verified_legacy_trial_adoption"
            && proof["plan_id"] == plan["plan_id"]
            && proof["original_post_worker_sha256"] == LEGACY_TRIAL_WORKER
            && proof["current_worker_executed_original_post"] == false
            && proof["current_worker_sha256"] == auth["worker_sha256"]
            && proof["current_worker_sha256"] == executable_sha()?
            && proof["config_binding_sha256"] == c.binding()?
            && proof["kernel_release"] == auth["kernel_release"]
            && proof["protocol_source_sha256"] == LEGACY_PROTOCOL_SOURCE
            && proof["network_source_sha256"] == LEGACY_NETWORK_SOURCE
            && proof["verified_receipt_sha256"] == sha(receipt.to_string())
            && previous["config_binding_sha256"] == c.binding()?
            && previous["kernel_release"] == auth["kernel_release"]
            && previous["worker_sha256"] == LEGACY_TRIAL_WORKER
            && previous["accept_unverified_performance"] == true
            && previous["formal_device_validation_claimed"] == false
            && previous["formal_protocol_validation_claimed"] == false
            && plan["preview_worker_sha256"] == LEGACY_TRIAL_WORKER
            && plan["preview_authorization_created_unix"] == previous["created_unix"],
        "trial was not executed by this worker or its explicitly verified predecessor"
    );
    Ok(())
}

fn verified_trial(store: &Store, c: &Config, id: &str, authorization: &Value) -> Result<Value> {
    let (plan_state, text): (String, String) =
        store
            .db
            .query_row("SELECT state,body FROM plans WHERE id=?", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
    let plan: Value = serde_json::from_str(&text)?;
    let (attempt_state, receipt): (String, String) =
        store
            .db
            .query_row("SELECT state,body FROM attempts WHERE id=?", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
    let receipt: Value = serde_json::from_str(&receipt)?;
    let mut hash_body = plan.clone();
    hash_body
        .as_object_mut()
        .context("invalid trial plan")?
        .remove("plan_id");
    ensure!(
        plan["plan_id"] == id && sha(hash_body.to_string()) == id,
        "trial plan hash changed"
    );
    ensure!(
        plan_state == "confirmed"
            && attempt_state == "confirmed"
            && receipt["state"] == "confirmed"
            && receipt["plan_id"] == id
            && receipt["acknowledged"] == true,
        "trial has not been confirmed by exact readback"
    );
    ensure!(
        plan["trial"] == true
            && plan["binding_sha256"] == c.binding()?
            && plan["account_hash"] == c.account_hash()
            && plan["seconds"]
                .as_i64()
                .is_some_and(|n| (1..=60).contains(&n))
            && plan["sessions"].as_array().is_some_and(|v| v.len() == 1),
        "confirmed trial belongs to another config/account or is not a whole short trial"
    );
    trial_authorization_matches(c, &plan, &receipt, authorization)?;
    let before: Position = serde_json::from_value(receipt["before"].clone())?;
    let after: Position = serde_json::from_value(receipt["after"].clone())?;
    ensure!(
        matched(
            &before,
            &after,
            receipt["day_before"]
                .as_i64()
                .context("missing day baseline")?,
            receipt["day_after"]
                .as_i64()
                .context("missing day result")?,
            plan["seconds"].as_i64().unwrap()
        ),
        "trial receipt increments/position do not match"
    );
    Ok(plan)
}

fn require_clean_activation(store: &Store) -> Result<()> {
    let unresolved: i64 = store.db.query_row(
        "SELECT count(*) FROM attempts WHERE state IN ('dispatching','unknown','needs_review','failed')",
        [], |r| r.get(0),
    )?;
    ensure!(
        unresolved == 0,
        "unresolved writes prevent enabling automatic sync"
    );
    Ok(())
}

/// Carry forward an already enabled installation across this queue-only upgrade.
/// This does not create an authorization for a previously disabled installation.
pub fn resume_isolated_autosync(store: &Store) -> Result<Value> {
    resume_isolated_with_probe(store, &actual_device())
}

fn resume_isolated_with_probe(store: &Store, probe: &Value) -> Result<Value> {
    const PRIOR_WORKER: &str = "f327c8bfc6605093a413aff88462c2056f36c54f74cb3e6e3669268803d22350";
    let c = Config::load(&store.root)?;
    actual_supported(probe)?;
    store.bind(&c)?;
    Auth::load(&store.root, &c)?;
    let current = check_tls(&store.root, &c, probe)?;
    let mut prior = private_json(&store.root.join(AUTHORIZATION))?;
    if prior["worker_sha256"] == current["worker_sha256"] {
        checked_authorization(&store.root, &c, &probe, true)?;
        ensure!(
            prior["queue_upgrade"]["previous_worker_sha256"] == PRIOR_WORKER,
            "existing authorization is not from this upgrade"
        );
        return Ok(json!({"state":"isolated_autosync_resumed"}));
    }
    unchanged_legacy_wire()?;
    for key in ["schema_version", "config_binding_sha256", "kernel_release"] {
        ensure!(
            prior[key] == current[key],
            "installed authorization belongs to another device configuration"
        );
    }
    ensure!(
        prior["worker_sha256"] == PRIOR_WORKER
            && prior["mode"] == "auto"
            && prior["accept_unverified_performance"] == true
            && prior["accept_unverified_protocol"] == true
            && prior["formal_device_validation_claimed"] == false
            && prior["formal_protocol_validation_claimed"] == false,
        "a previously enabled preview installation is required"
    );
    let id = prior["confirmed_trial_plan_id"]
        .as_str()
        .context("missing confirmed trial")?;
    let adoption = &prior["trial_adoption"];
    ensure!(
        adoption["plan_id"] == id && adoption["current_worker_sha256"] == PRIOR_WORKER,
        "prior verified trial evidence does not match the installed worker"
    );
    let original = &adoption["prior_authorization"];
    legacy_authorization(&c, &probe, original)?;
    verified_trial(store, &c, id, original)?;
    prior["worker_sha256"] = current["worker_sha256"].clone();
    prior["queue_upgrade"] = json!({"previous_worker_sha256":PRIOR_WORKER,"enabled_unix":now()});
    atomic_json(&store.root.join(AUTHORIZATION), &prior)?;
    Ok(json!({"state":"isolated_autosync_resumed"}))
}

fn adopt_authorization(
    store: &Store,
    c: &Config,
    id: &str,
    previous: &Value,
    probe: &Value,
) -> Result<Value> {
    legacy_authorization(c, probe, previous)?;
    require_clean_activation(store)?;
    let plan = verified_trial(store, c, id, previous)?;
    for session in plan["sessions"].as_array().unwrap() {
        parsing::verify_manifest(&store.root, session)?;
    }
    let text: String = store
        .db
        .query_row("SELECT body FROM attempts WHERE id=?", [id], |r| r.get(0))?;
    let receipt: Value = serde_json::from_str(&text)?;
    let mut current = check_tls(&store.root, c, probe)?;
    current["mode"] = json!("trial");
    current["created_unix"] = json!(now());
    current["accept_unverified_performance"] = json!(true);
    current["accept_unverified_protocol"] = json!(false);
    current["formal_device_validation_claimed"] = json!(false);
    current["formal_protocol_validation_claimed"] = json!(false);
    current["trial_adoption"] = json!({"schema_version":1,"kind":"verified_legacy_trial_adoption","plan_id":id,"original_post_worker_sha256":LEGACY_TRIAL_WORKER,"current_worker_executed_original_post":false,"current_worker_sha256":current["worker_sha256"],"config_binding_sha256":c.binding()?,"kernel_release":current["kernel_release"],"protocol_source_sha256":LEGACY_PROTOCOL_SOURCE,"network_source_sha256":LEGACY_NETWORK_SOURCE,"verified_receipt_sha256":sha(receipt.to_string()),"readonly_verified_unix":now(),"prior_authorization":previous});
    // One private atomic file keeps the original execution evidence and the new
    // authorization together, and remains covered by permission restoration.
    atomic_json(&store.root.join(AUTHORIZATION), &current)?;
    Ok(current)
}

/// Renew authentication only before read-only rechecking. Never resends the
/// predecessor's trial, rewrites its plan, or claims its POST for the new ELF.
pub fn adopt_and_enable(
    store: &Store,
    c: &Config,
    id: &str,
    performance: bool,
    protocol: bool,
) -> Result<Value> {
    ensure!(
        performance && protocol,
        "explicit adoption and experimental acceptance required"
    );
    let probe = actual_device();
    let held = private_json(&store.root.join(AUTHORIZATION))?;
    let previous = if held["worker_sha256"] == LEGACY_TRIAL_WORKER {
        held
    } else {
        held["trial_adoption"]["prior_authorization"].clone()
    };
    legacy_authorization(c, &probe, &previous)?;
    check_tls(&store.root, c, &probe)?;
    let text: String = store
        .db
        .query_row("SELECT body FROM plans WHERE id=?", [id], |r| r.get(0))?;
    let plan: Value = serde_json::from_str(&text)?;
    let mut hash_body = plan.clone();
    hash_body
        .as_object_mut()
        .context("invalid legacy trial plan")?
        .remove("plan_id");
    ensure!(
        plan["plan_id"] == id
            && sha(hash_body.to_string()) == id
            && plan["trial"] == true
            && plan["seconds"]
                .as_i64()
                .is_some_and(|s| (1..=60).contains(&s))
            && plan["sessions"].as_array().is_some_and(|s| s.len() == 1)
            && plan["preview_worker_sha256"] == LEGACY_TRIAL_WORKER
            && plan["preview_authorization_created_unix"] == previous["created_unix"]
            && plan["binding_sha256"] == c.binding()?
            && plan["account_hash"] == c.account_hash(),
        "legacy trial origin mismatch"
    );
    manual::gate_manual(&store.root, true)?;
    let mut transport = network::Live::default();
    let mut auth = Auth::load(&store.root, c)?;
    let mut refreshed = false;
    let baseline = network::preflight(
        &mut transport,
        &store.root,
        &mut auth,
        &mut refreshed,
        "/book/getProgress",
        &[(
            "bookId",
            plan["weread_book_id"]
                .as_str()
                .context("missing legacy book")?
                .into(),
        )],
    )?;
    ensure!(
        baseline["timestamp"]
            .as_i64()
            .is_some_and(|t| (t - now()).abs() <= 600),
        "device/server clock mismatch; adoption blocked"
    );
    let receipt = manual::reconcile_manual(store, id, &mut transport)?;
    ensure!(
        receipt["state"] == "confirmed",
        "legacy trial still awaiting exact live book/day/position readback"
    );
    manual::gate_manual(&store.root, true)?;
    ensure!(
        Config::load(&store.root)?.binding()? == c.binding()?,
        "configuration changed during adoption"
    );
    let collected = parsing::collect(
        store,
        Path::new("/mnt/us/documents"),
        Path::new("/var/local/metadata/cc.db"),
    )?;
    manual::gate_manual(&store.root, true)?;
    let adopted = adopt_authorization(store, c, id, &previous, &probe)?;
    let activation = enable(store, c, id, true)?;
    let drained=drain(store,&mut transport).unwrap_or_else(|_|json!({"state":"paused","reason":"automatic_drain_stopped","retry_on_future_safe_event":true}));
    Ok(
        json!({"state":"legacy_trial_adopted","original_trial_post_replayed":false,"trial_plan_unchanged":true,"adoption":adopted["trial_adoption"],"reconciled_receipt":receipt,"experimental_activation":activation,"collected":collected,"automatic_drain":drained}),
    )
}
pub fn enable(store: &Store, c: &Config, id: &str, accept: bool) -> Result<Value> {
    enable_with_probe(store, c, id, accept, &actual_device())
}
fn enable_with_probe(
    store: &Store,
    c: &Config,
    id: &str,
    accept: bool,
    probe: &Value,
) -> Result<Value> {
    ensure!(
        accept,
        "acceptance of incomplete protocol matrix must be explicit"
    );
    Auth::load(&store.root, c)?;
    let mut proof = checked_authorization(&store.root, c, probe, false)?;
    require_clean_activation(store)?;
    verified_trial(store, c, id, &proof)?;
    proof["mode"] = json!("auto");
    proof["accept_unverified_protocol"] = json!(true);
    proof["confirmed_trial_plan_id"] = json!(id);
    proof["enabled_unix"] = json!(now());
    atomic_json(&store.root.join(AUTHORIZATION), &proof)?;
    let refresh_requested = manual::request_refresh(&store.root).is_ok();
    Ok(
        json!({"state":"experimental_automatic_sync_enabled","automatic_upload_enabled":true,"formal_validation_unchanged":true,"refresh_requested":refresh_requested,"binding_sha256":c.binding()?}),
    )
}
pub fn disable(root: &Path) -> Result<Value> {
    let path = root.join(AUTHORIZATION);
    if path.exists() {
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "unsafe preview authorization"
        );
        fs::remove_file(path)?;
        fs::File::open(root)?.sync_all()?;
    }
    Ok(
        json!({"state":"experimental_upload_disabled","automatic_upload_enabled":false,"ledger_preserved":true}),
    )
}
pub fn status(root: &Path, c: &Config) -> Value {
    let probe = actual_device();
    let trial = checked_authorization(root, c, &probe, false).is_ok();
    let automatic = checked_authorization(root, c, &probe, true).is_ok();
    json!({"trial_authorized":trial,"automatic_authorized":automatic,"formal_validation_claimed":false})
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    action: String,
    config_binding_sha256: String,
    #[serde(default)]
    local_book_id: Option<String>,
    #[serde(default)]
    max_seconds: Option<i64>,
    #[serde(default)]
    plan_id: Option<String>,
    #[serde(default)]
    accept_unverified_performance: bool,
    #[serde(default)]
    accept_unverified_protocol: bool,
}
pub fn operate(store: &Store, inbox: &Path, action: &str) -> Result<Value> {
    ensure!(
        !fs::symlink_metadata(inbox)?.file_type().is_symlink()
            && fs::symlink_metadata(inbox)?.is_dir(),
        "unsafe action inbox"
    );
    let request: Request = serde_json::from_slice(&limited(&inbox.join("request.json"), 16384)?)?;
    let c = Config::load(&store.root)?;
    ensure!(
        request.schema_version == 1
            && request.action == action
            && request.config_binding_sha256 == c.binding()?,
        "action/request configuration mismatch"
    );
    match action {
        "adopt-and-enable" => adopt_and_enable(
            store,
            &c,
            request
                .plan_id
                .as_deref()
                .context("missing exact legacy trial plan")?,
            request.accept_unverified_performance,
            request.accept_unverified_protocol,
        ),
        "authorize-prepare" => {
            authorize_trial(&store.root, &c, request.accept_unverified_performance)?;
            manual::gate_manual(&store.root, false)?;
            let collected = parsing::collect(
                store,
                Path::new("/mnt/us/documents"),
                Path::new("/var/local/metadata/cc.db"),
            )?;
            let mut prepared = manual::prepare_manual(
                store,
                Some(
                    request
                        .local_book_id
                        .as_deref()
                        .context("missing local book")?,
                ),
                true,
                request.max_seconds.context("missing trial duration cap")?,
            )?;
            prepared["collected"] = collected;
            Ok(prepared)
        }
        "confirm-enable" => {
            ensure!(
                request.accept_unverified_protocol,
                "automatic preview use must be explicit before confirming the trial"
            );
            let id = request
                .plan_id
                .as_deref()
                .context("missing exact plan id")?;
            let mut transport = network::Live::default();
            let mut receipt = manual::commit_manual(store, id, true, &mut transport)?;
            if receipt["state"] == "confirmed" {
                receipt["experimental_activation"] = enable(store, &c, id, true)?;
                // We still own the Store lock: an adapter-triggered second
                // process cannot drain it. Continue here after activation,
                // with the same cancellation and real idle/network gates.
                receipt["automatic_drain"] = drain(store, &mut transport).unwrap_or_else(|_| {
                    json!({"state":"paused","reason":"automatic_drain_stopped","retry_on_future_safe_event":true})
                });
            } else {
                receipt["experimental_activation"] =
                    json!({"automatic_upload_enabled":false,"state":"trial_not_verified"});
            }
            Ok(receipt)
        }
        "authorize-trial" => {
            authorize_trial(&store.root, &c, request.accept_unverified_performance)
        }
        "prepare" => {
            preview_gate(&store.root, &c, false)?;
            manual::prepare_manual(
                store,
                Some(
                    request
                        .local_book_id
                        .as_deref()
                        .context("missing local book")?,
                ),
                true,
                request.max_seconds.context("missing trial duration cap")?,
            )
        }
        "confirm" => manual::commit_manual(
            store,
            request
                .plan_id
                .as_deref()
                .context("missing exact plan id")?,
            true,
            &mut network::Live::default(),
        ),
        "enable-experimental" => {
            manual::gate_manual(&store.root, true)?;
            enable(
                store,
                &c,
                request
                    .plan_id
                    .as_deref()
                    .context("missing confirmed plan id")?,
                request.accept_unverified_protocol,
            )
        }
        "disable-experimental" => disable(&store.root),
        _ => anyhow::bail!("unsupported preview action"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn fixture() -> (TempDir, Store, Config, Value) {
        let t = TempDir::new().unwrap();
        fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(t.path()).unwrap();
        let c:Config=serde_json::from_value(json!({"schema_version":1,"device_id":"test-device","account_vid":"synthetic-account","profile":"pw2-5.12.2.1.1","firmware":"5.12.2.1.1","timezone":"Asia/Shanghai","bootstrap_cutoff_unix":1000,"mappings":[],"validation":{"device_validation":false,"protocol_validation":false,"binding_sha256":""}})).unwrap();
        atomic_json(
            &t.path().join("config.json"),
            &serde_json::to_value(&c).unwrap(),
        )
        .unwrap();
        atomic_json(&t.path().join("auth.json"),&json!({"vid":"synthetic-account","deviceId":"test-device","accessToken":"synthetic-access","refreshToken":"synthetic-refresh"})).unwrap();
        let probe = json!({"architecture":"arm","supported_hardware":true,"supported_firmware":true,"root":true,"kernel_release":"synthetic-kernel"});
        (t, store, c, probe)
    }
    fn tls(root: &Path, c: &Config, p: &Value) {
        record_live_tls(root,c,p,&json!({"state":"completed","https_certificate_validated":true,"credentials_sent":false,"network_writes":false})).unwrap();
    }
    fn confirmed_trial(s: &Store, c: &Config) -> String {
        fs::create_dir_all(s.root.join("events/sealed")).unwrap();
        let source = s.root.join("events/sealed/synthetic.log");
        fs::write(&source, "synthetic immutable native trial evidence\n").unwrap();
        let mut p = json!({"trial":true,"binding_sha256":c.binding().unwrap(),"account_hash":c.account_hash(),"seconds":13,"sessions":[{"id":"synthetic-session","sources":[{"path":source,"sha256":sha(fs::read(&source).unwrap())}]}]});
        let authorization = private_json(&s.root.join(AUTHORIZATION)).unwrap();
        p["preview_worker_sha256"] = authorization["worker_sha256"].clone();
        p["preview_authorization_created_unix"] = authorization["created_unix"].clone();
        let id = sha(p.to_string());
        p["plan_id"] = json!(id);
        let r = json!({"plan_id":id,"state":"confirmed","acknowledged":true,"before":{"seconds":100,"progress":10,"chapter":2,"offset":7},"after":{"seconds":113,"progress":10,"chapter":2,"offset":7},"day_before":100,"day_after":113});
        s.db.execute(
            "INSERT INTO plans VALUES (?,'confirmed',?)",
            params![id, p.to_string()],
        )
        .unwrap();
        s.db.execute(
            "INSERT INTO attempts VALUES (?,'confirmed',?)",
            params![id, r.to_string()],
        )
        .unwrap();
        id
    }
    #[test]
    fn explicit_preview_cannot_fake_formal_acceptance_or_skip_live_tls() {
        let (t, _s, c, p) = fixture();
        assert!(authorize_trial_with_probe(t.path(), &c, true, &p).is_err());
        tls(t.path(), &c, &p);
        assert!(authorize_trial_with_probe(t.path(), &c, false, &p).is_err());
        authorize_trial_with_probe(t.path(), &c, true, &p).unwrap();
        assert!(!Config::load(t.path()).unwrap().validation.device_validation);
        assert!(
            !Config::load(t.path())
                .unwrap()
                .validation
                .protocol_validation
        );
        assert!(checked_authorization(t.path(), &c, &p, false).is_ok());
        assert!(checked_authorization(t.path(), &c, &p, true).is_err());
        assert!(authorize_trial(t.path(), &c, true).is_err()); // actual host is not a PW2
    }
    #[test]
    fn changed_binary_config_or_kernel_invalidates_preview() {
        let (t, _s, c, p) = fixture();
        tls(t.path(), &c, &p);
        authorize_trial_with_probe(t.path(), &c, true, &p).unwrap();
        let mut changed = c.clone();
        changed.bootstrap_cutoff_unix += 1;
        assert!(checked_authorization(t.path(), &changed, &p, false).is_err());
        let mut q = p.clone();
        q["kernel_release"] = json!("different");
        assert!(checked_authorization(t.path(), &c, &q, false).is_err());
        let mut proof = private_json(&t.path().join(TLS_PROOF)).unwrap();
        proof["worker_sha256"] = json!("different");
        atomic_json(&t.path().join(TLS_PROOF), &proof).unwrap();
        assert!(checked_authorization(t.path(), &c, &p, false).is_err());
    }
    #[test]
    fn enable_requires_one_exact_confirmed_trial_and_no_unknown_write() {
        let (t, s, c, p) = fixture();
        tls(t.path(), &c, &p);
        authorize_trial_with_probe(t.path(), &c, true, &p).unwrap();
        assert!(enable_with_probe(&s, &c, "missing", true, &p).is_err());
        let id = confirmed_trial(&s, &c);
        assert!(enable_with_probe(&s, &c, &id, false, &p).is_err());
        s.db.execute("UPDATE attempts SET state='needs_review'", [])
            .unwrap();
        assert!(enable_with_probe(&s, &c, &id, true, &p).is_err());
        s.db.execute("UPDATE attempts SET state='confirmed'", [])
            .unwrap();
        enable_with_probe(&s, &c, &id, true, &p).unwrap();
        assert!(checked_authorization(t.path(), &c, &p, true).is_ok());
        assert_eq!(
            Config::load(t.path())
                .unwrap()
                .validation
                .protocol_validation,
            false
        );
        disable(t.path()).unwrap();
        assert!(checked_authorization(t.path(), &c, &p, true).is_err());
        assert_eq!(
            s.db.query_row("SELECT count(*) FROM attempts", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn success_ack_or_wrong_position_is_never_a_confirmed_trial() {
        let (t, s, c, p) = fixture();
        tls(t.path(), &c, &p);
        authorize_trial_with_probe(t.path(), &c, true, &p).unwrap();
        let id = confirmed_trial(&s, &c);
        let mut receipt: Value = serde_json::from_str(
            &s.db
                .query_row("SELECT body FROM attempts", [], |r| r.get::<_, String>(0))
                .unwrap(),
        )
        .unwrap();
        receipt["after"]["offset"] = json!(8);
        s.db.execute("UPDATE attempts SET body=?", [receipt.to_string()])
            .unwrap();
        assert!(enable_with_probe(&s, &c, &id, true, &p).is_err());
    }
    fn legacy_trial_fixture() -> (TempDir, Store, Config, Value, Value, String) {
        let (t, s, c, p) = fixture();
        tls(t.path(), &c, &p);
        authorize_trial_with_probe(t.path(), &c, true, &p).unwrap();
        let mut prior = private_json(&t.path().join(AUTHORIZATION)).unwrap();
        prior["worker_sha256"] = json!(LEGACY_TRIAL_WORKER);
        atomic_json(&t.path().join(AUTHORIZATION), &prior).unwrap();
        let id = confirmed_trial(&s, &c);
        (t, s, c, p, prior, id)
    }
    #[test]
    fn legacy_adoption_preserves_original_execution_and_requires_verified_receipt() {
        let (t, s, c, p, prior, id) = legacy_trial_fixture();
        let original_plan: String =
            s.db.query_row("SELECT body FROM plans WHERE id=?", [&id], |r| r.get(0))
                .unwrap();
        let original_receipt: String =
            s.db.query_row("SELECT body FROM attempts WHERE id=?", [&id], |r| r.get(0))
                .unwrap();
        let adopted = adopt_authorization(&s, &c, &id, &prior, &p).unwrap();
        assert_eq!(
            adopted["trial_adoption"]["original_post_worker_sha256"],
            LEGACY_TRIAL_WORKER
        );
        assert_eq!(
            adopted["trial_adoption"]["current_worker_executed_original_post"],
            false
        );
        assert_eq!(adopted["trial_adoption"]["prior_authorization"], prior);
        assert_ne!(adopted["worker_sha256"], LEGACY_TRIAL_WORKER);
        assert_eq!(
            s.db.query_row("SELECT body FROM plans WHERE id=?", [&id], |r| r
                .get::<_, String>(0))
                .unwrap(),
            original_plan
        );
        assert_eq!(
            s.db.query_row("SELECT body FROM attempts WHERE id=?", [&id], |r| r
                .get::<_, String>(0))
                .unwrap(),
            original_receipt
        );
        enable_with_probe(&s, &c, &id, true, &p).unwrap();
        assert!(checked_authorization(t.path(), &c, &p, true).is_ok());
        assert!(!Config::load(t.path()).unwrap().validation.device_validation);
        assert!(
            !Config::load(t.path())
                .unwrap()
                .validation
                .protocol_validation
        );
    }
    #[test]
    fn queue_upgrade_checks_new_tls_and_old_trial_without_requiring_new_authorization() {
        let (t, s, c, p, original, id) = legacy_trial_fixture();
        let mut adopted = adopt_authorization(&s, &c, &id, &original, &p).unwrap();
        adopted["worker_sha256"] =
            json!("f327c8bfc6605093a413aff88462c2056f36c54f74cb3e6e3669268803d22350");
        adopted["trial_adoption"]["current_worker_sha256"] = adopted["worker_sha256"].clone();
        adopted["mode"] = json!("auto");
        adopted["accept_unverified_protocol"] = json!(true);
        adopted["confirmed_trial_plan_id"] = json!(id);
        atomic_json(&t.path().join(AUTHORIZATION), &adopted).unwrap();
        assert!(checked_authorization(t.path(), &c, &p, true).is_err());
        assert!(resume_isolated_autosync(&s).is_err()); // Actual Mac is not an ARM PW2.
        fs::remove_file(t.path().join(TLS_PROOF)).unwrap();
        assert!(resume_isolated_with_probe(&s, &p).is_err());
        tls(t.path(), &c, &p);
        resume_isolated_with_probe(&s, &p).unwrap();
        assert!(checked_authorization(t.path(), &c, &p, true).is_ok());
        resume_isolated_with_probe(&s, &p).unwrap();
        assert_eq!(s.status().unwrap()["attempts"], 1);
        let state: String = s.db.query_row("SELECT state FROM attempts WHERE id=?", [&id], |r| r.get(0)).unwrap();
        assert_eq!(state, "confirmed");
        adopted["mode"] = json!("trial");
        atomic_json(&t.path().join(AUTHORIZATION), &adopted).unwrap();
        assert!(resume_isolated_with_probe(&s, &p).is_err());
    }
    #[test]
    fn legacy_adoption_never_promotes_partial_unknown_or_foreign_execution() {
        for cause in ["partial_book", "unknown", "other_worker", "other_account"] {
            let (t, s, c, p, mut prior, id) = legacy_trial_fixture();
            match cause {
                "partial_book" => {
                    let mut r: Value = serde_json::from_str(
                        &s.db
                            .query_row("SELECT body FROM attempts WHERE id=?", [&id], |r| {
                                r.get::<_, String>(0)
                            })
                            .unwrap(),
                    )
                    .unwrap();
                    r["after"]["seconds"] = json!(100);
                    s.db.execute(
                        "UPDATE attempts SET body=? WHERE id=?",
                        params![r.to_string(), id],
                    )
                    .unwrap();
                }
                "unknown" => {
                    s.db.execute("INSERT INTO attempts VALUES ('another','unknown','{}')", [])
                        .unwrap();
                }
                "other_worker" => prior["worker_sha256"] = json!("unsupported-worker"),
                "other_account" => prior["config_binding_sha256"] = json!("foreign-binding"),
                _ => unreachable!(),
            }
            assert!(
                adopt_authorization(&s, &c, &id, &prior, &p).is_err(),
                "{cause}"
            );
            assert_ne!(
                private_json(&t.path().join(AUTHORIZATION)).unwrap()["mode"],
                "auto"
            );
        }
    }
    #[test]
    fn adopted_trial_cannot_hide_wire_changes_or_later_receipt_changes() {
        let (t, s, c, p, prior, id) = legacy_trial_fixture();
        let adopted = adopt_authorization(&s, &c, &id, &prior, &p).unwrap();
        let mut tampered = adopted.clone();
        tampered["trial_adoption"]["protocol_source_sha256"] = json!("changed-protocol");
        atomic_json(&t.path().join(AUTHORIZATION), &tampered).unwrap();
        assert!(enable_with_probe(&s, &c, &id, true, &p).is_err());
        atomic_json(&t.path().join(AUTHORIZATION), &adopted).unwrap();
        let mut r: Value = serde_json::from_str(
            &s.db
                .query_row("SELECT body FROM attempts WHERE id=?", [&id], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
        )
        .unwrap();
        r["after"]["offset"] = json!(8);
        s.db.execute(
            "UPDATE attempts SET body=? WHERE id=?",
            params![r.to_string(), id],
        )
        .unwrap();
        assert!(enable_with_probe(&s, &c, &id, true, &p).is_err());
    }
    #[test]
    fn legacy_adoption_requires_current_tls_and_unchanged_native_provenance() {
        let (t, s, c, p, prior, id) = legacy_trial_fixture();
        fs::remove_file(t.path().join(TLS_PROOF)).unwrap();
        assert!(adopt_authorization(&s, &c, &id, &prior, &p).is_err());
        tls(t.path(), &c, &p);
        fs::write(
            s.root.join("events/sealed/synthetic.log"),
            "changed native provenance",
        )
        .unwrap();
        assert!(adopt_authorization(&s, &c, &id, &prior, &p).is_err());
    }
    #[test]
    fn action_request_rejects_symlink_unknown_fields_and_binding_changes_before_network() {
        let (t, s, c, _p) = fixture();
        let inbox = t.path().join("inbox");
        fs::create_dir(&inbox).unwrap();
        let request = json!({"schema_version":1,"action":"confirm","config_binding_sha256":"wrong","plan_id":"arbitrary"});
        fs::write(inbox.join("request.json"), request.to_string()).unwrap();
        assert!(operate(&s, &inbox, "confirm").is_err());
        let request = json!({"schema_version":1,"action":"confirm","config_binding_sha256":c.binding().unwrap(),"plan_id":"arbitrary","shell":"not-allowed"});
        fs::write(inbox.join("request.json"), request.to_string()).unwrap();
        assert!(operate(&s, &inbox, "confirm").is_err());
        fs::remove_file(inbox.join("request.json")).unwrap();
        std::os::unix::fs::symlink(t.path().join("auth.json"), inbox.join("request.json")).unwrap();
        assert!(operate(&s, &inbox, "confirm").is_err());
    }
}
