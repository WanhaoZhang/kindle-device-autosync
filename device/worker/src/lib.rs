//! One-shot device worker. No UI, no background thread, no wake/Wi-Fi control.
//! SPDX-License-Identifier: AGPL-3.0-only
pub mod experimental;
pub mod manual;
pub mod network;
pub mod parsing;
pub mod protocol;
use anyhow::{Context, Result, ensure};
use chrono::{Datelike, FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
};

#[cfg(not(test))]
static CANCEL_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
std::thread_local! {static TEST_CANCEL:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}
pub fn cancelled() -> bool {
    #[cfg(not(test))]
    {
        CANCEL_REQUESTED.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_CANCEL.with(|v| v.get())
    }
}
pub fn request_cancel() {
    #[cfg(not(test))]
    CANCEL_REQUESTED.store(true, std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    TEST_CANCEL.with(|v| v.set(true));
}
extern "C" fn cancel_signal(_: libc::c_int) {
    request_cancel();
}
pub fn install_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        libc::signal(libc::SIGTERM, cancel_signal as libc::sighandler_t);
        libc::signal(libc::SIGINT, cancel_signal as libc::sighandler_t);
    }
}
pub fn check_cancel() -> Result<()> {
    ensure!(!cancelled(), "cancelled before dispatch");
    Ok(())
}

pub fn sha(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes.as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
pub fn zone() -> FixedOffset {
    FixedOffset::east_opt(28800).unwrap()
}
pub fn epoch(local: &str) -> Result<i64> {
    Ok(zone()
        .from_local_datetime(&NaiveDateTime::parse_from_str(local, "%Y-%m-%dT%H:%M:%S")?)
        .single()
        .context("ambiguous time")?
        .timestamp())
}
pub fn limited(path: &Path, max: u64) -> Result<Vec<u8>> {
    let m = fs::symlink_metadata(path)?;
    ensure!(
        m.is_file() && !m.file_type().is_symlink() && m.len() <= max,
        "not a bounded regular file"
    );
    Ok(fs::read(path)?)
}
pub fn private_json(path: &Path) -> Result<Value> {
    ensure!(
        fs::symlink_metadata(path)?.permissions().mode() & 0o077 == 0,
        "private file permissions required"
    );
    Ok(serde_json::from_slice(&limited(path, 1024 * 1024)?)?)
}
pub const SYSTEM_RESERVE_BYTES: u64 = 20 * 1024 * 1024;
pub const ROUTINE_TRANSACTION_BYTES: u64 = 4 * 1024 * 1024;
pub const LEDGER_LIMIT_BYTES: u64 = 4 * 1024 * 1024;
pub const LEDGER_MAX_PAGES: i64 = 1024;
pub const STORAGE_METADATA_MARGIN: u64 = 64 * 1024;

fn existing_storage_ancestor(path: &Path) -> Result<&Path> {
    let mut at = path;
    while !at.exists() {
        at = at
            .parent()
            .context("storage_blocked: no existing filesystem ancestor")?;
    }
    Ok(at)
}
pub fn required_free_bytes(transaction_bytes: u64, block: u64) -> Result<u64> {
    ensure!(
        block > 0,
        "storage_blocked: invalid filesystem allocation unit"
    );
    let raw = SYSTEM_RESERVE_BYTES
        .checked_add(transaction_bytes)
        .and_then(|n| n.checked_add(STORAGE_METADATA_MARGIN))
        .context("storage_blocked: budget overflow")?;
    let rounded = raw
        .checked_add(block - 1)
        .context("storage_blocked: budget overflow")?
        / block;
    rounded
        .checked_mul(block)
        .context("storage_blocked: budget overflow")
}
pub fn check_available_bytes(free: u64, transaction_bytes: u64, block: u64) -> Result<()> {
    ensure!(
        free >= required_free_bytes(transaction_bytes, block)?,
        "storage_blocked: private filesystem reserve"
    );
    Ok(())
}
pub fn atomic_json(path: &Path, v: &Value) -> Result<()> {
    let parent = path.parent().context("no parent")?;
    let bytes = serde_json::to_vec_pretty(v)?;
    // Existing target bytes are already charged in statvfs. Budget the new
    // complete temporary, so atomic replacement cannot consume the OS reserve.
    storage_budget(existing_storage_ancestor(parent)?, bytes.len() as u64)?;
    fs::create_dir_all(parent)?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut f = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)?;
    let result = (|| {
        f.write_all(&bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
/// Every write is admitted against real free blocks. The C collector uses the
/// same 20MiB OS reserve plus 4MiB worker scratch and a 64KiB metadata margin.
pub fn storage_budget(root: &Path, transaction_bytes: u64) -> Result<()> {
    use std::ffi::CString;
    let path = CString::new(
        existing_storage_ancestor(root)?
            .as_os_str()
            .as_encoded_bytes(),
    )?;
    let mut v = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    ensure!(
        unsafe { libc::statvfs(path.as_ptr(), v.as_mut_ptr()) } == 0,
        "storage_blocked: cannot inspect private filesystem"
    );
    let v = unsafe { v.assume_init() };
    let block = (v.f_frsize as u64).max(1);
    let free = (v.f_bavail as u64).saturating_mul(block);
    check_available_bytes(free, transaction_bytes, block)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Runtime {
    pub schema_version: u32,
    pub reader_state: String,
    pub power_state: String,
    pub usb_export: bool,
    pub network_connected: bool,
    pub idle_since_unix: i64,
    pub bootstrap_unix: i64,
    #[serde(default)]
    pub capture_blocked: bool,
    #[serde(default)]
    pub coverage_gap: bool,
}
pub fn gate(root: &Path, network: bool, at: i64) -> Result<Runtime> {
    check_cancel()?;
    let path = root.join("runtime.json");
    let m = fs::symlink_metadata(&path)?;
    let updated = m
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    ensure!(
        updated <= at + 5 && at - updated <= 120,
        "stale runtime; collector must refresh before spawn"
    );
    let r: Runtime = serde_json::from_slice(&limited(&path, 16384)?)?;
    ensure!(
        r.schema_version == 1
            && r.reader_state == "idle"
            && r.power_state == "awake"
            && !r.usb_export
            && !r.capture_blocked,
        "runtime blocks worker"
    );
    ensure!(
        r.bootstrap_unix > 0
            && r.bootstrap_unix <= r.idle_since_unix
            && r.idle_since_unix <= at - 20,
        "idle window not established"
    );
    ensure!(
        !network || (r.network_connected && !r.coverage_gap),
        "offline or capture coverage gap"
    );
    Ok(r)
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub local_book_id: String,
    pub weread_book_id: String,
    pub title: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Validation {
    pub protocol_validation: bool,
    pub device_validation: bool,
    pub binding_sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub device_id: String,
    pub account_vid: String,
    pub profile: String,
    pub firmware: String,
    pub timezone: String,
    pub bootstrap_cutoff_unix: i64,
    pub mappings: Vec<Mapping>,
    pub validation: Validation,
}
impl Config {
    pub fn binding(&self) -> Result<String> {
        let mut v = serde_json::to_value(self)?;
        v.as_object_mut().unwrap().remove("validation");
        Ok(sha(serde_json::to_vec(&v)?))
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1
                && self.profile == "pw2-5.12.2.1.1"
                && self.firmware == "5.12.2.1.1"
                && self.timezone == "Asia/Shanghai"
                && self.bootstrap_cutoff_unix > 0,
            "unsupported configuration"
        );
        ensure!(
            !self.device_id.is_empty() && !self.account_vid.is_empty(),
            "identity required"
        );
        let mut seen = std::collections::BTreeSet::new();
        for m in &self.mappings {
            ensure!(
                !m.local_book_id.is_empty()
                    && !m.weread_book_id.is_empty()
                    && seen.insert(&m.local_book_id),
                "invalid mapping"
            );
        }
        Ok(())
    }
    pub fn load(root: &Path) -> Result<Self> {
        let c: Self = serde_json::from_value(private_json(&root.join("config.json"))?)?;
        c.validate()?;
        Ok(c)
    }
    pub fn account_hash(&self) -> String {
        sha(self.account_vid.as_bytes())
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Auth {
    pub vid: String,
    #[serde(rename = "deviceId")]
    pub device_id: String,
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
}
impl Auth {
    pub fn load(root: &Path, c: &Config) -> Result<Self> {
        let a: Self = serde_json::from_value(private_json(&root.join("auth.json"))?)?;
        ensure!(
            a.vid == c.account_vid
                && a.device_id == c.device_id
                && !a.access_token.is_empty()
                && !a.refresh_token.is_empty(),
            "credentials do not match installation"
        );
        ensure!(
            [&a.vid, &a.device_id, &a.access_token, &a.refresh_token]
                .iter()
                .all(|s| !s.contains(['\r', '\n'])),
            "invalid credential encoding"
        );
        Ok(a)
    }
}

/// Diagnostic snapshot only: state JSON plus stat sizes, never raw log content.
pub fn capture_diagnostics(root: &Path) -> Value {
    let mut out = json!({"available":false});
    let counts = (|| -> Result<(u64, u64, u64)> {
        let events = root.join("events");
        let md = fs::symlink_metadata(&events)?;
        ensure!(
            md.is_dir() && !md.file_type().is_symlink(),
            "unsafe capture directory"
        );
        let active = events.join("active.log");
        let active_bytes = match fs::symlink_metadata(&active) {
            Ok(m) => {
                ensure!(
                    m.is_file() && !m.file_type().is_symlink(),
                    "unsafe active capture file"
                );
                m.len()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e.into()),
        };
        let sealed = events.join("sealed");
        let m = fs::symlink_metadata(&sealed)?;
        ensure!(
            m.is_dir() && !m.file_type().is_symlink(),
            "unsafe sealed capture directory"
        );
        let (mut bytes, mut files) = (0u64, 0u64);
        for entry in fs::read_dir(sealed)? {
            let p = entry?.path();
            if p.extension().and_then(|s| s.to_str()) != Some("log") {
                continue;
            }
            let m = fs::symlink_metadata(&p)?;
            ensure!(
                m.is_file() && !m.file_type().is_symlink(),
                "unsafe sealed capture file"
            );
            bytes = bytes
                .checked_add(m.len())
                .context("capture size overflow")?;
            files += 1;
        }
        Ok((active_bytes, bytes, files))
    })();
    if let Ok((active, sealed, files)) = counts {
        out["active_bytes"] = json!(active);
        out["sealed_bytes"] = json!(sealed);
        out["sealed_files"] = json!(files);
    } else {
        return out;
    }
    let runtime = (|| -> Result<Runtime> {
        let p = root.join("runtime.json");
        let md = fs::symlink_metadata(&p)?;
        use std::os::unix::fs::MetadataExt;
        ensure!(
            md.is_file()
                && !md.file_type().is_symlink()
                && md.uid() == unsafe { libc::geteuid() }
                && (md.mode() & 0o077) == 0,
            "unsafe runtime diagnostic"
        );
        let r: Runtime = serde_json::from_slice(&limited(&p, 16384)?)?;
        ensure!(
            r.schema_version == 1
                && ["active", "idle", "unknown"].contains(&r.reader_state.as_str())
                && ["awake", "sleeping", "unknown"].contains(&r.power_state.as_str())
                && r.bootstrap_unix >= 0
                && r.idle_since_unix >= 0,
            "invalid capture state"
        );
        Ok(r)
    })();
    if let Ok(r) = runtime {
        out["available"] = json!(true);
        out["reader_state"] = json!(r.reader_state);
        out["power_state"] = json!(r.power_state);
        out["usb_export"] = json!(r.usb_export);
        out["network_connected"] = json!(r.network_connected);
        out["capture_blocked"] = json!(r.capture_blocked);
        out["coverage_gap"] = json!(r.coverage_gap);
        out["bootstrap_unix"] = json!(r.bootstrap_unix);
        out["idle_since_unix"] = json!(r.idle_since_unix);
    }
    out
}

pub struct Store {
    pub root: PathBuf,
    pub db: Connection,
    _lock: fs::File,
}
impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        storage_budget(root, ROUTINE_TRANSACTION_BYTES)?;
        if !root.exists() {
            fs::create_dir_all(root)?;
            fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        }
        let md = fs::symlink_metadata(root)?;
        ensure!(
            md.is_dir() && !md.file_type().is_symlink() && md.permissions().mode() & 0o077 == 0,
            "root must be a private nonsymlink directory"
        );
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join("worker.lock"))?;
        ensure!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "worker already running"
        );
        let path = root.join("ledger.sqlite");
        if path.exists() {
            ensure!(
                fs::symlink_metadata(&path)?.is_file()
                    && !fs::symlink_metadata(&path)?.file_type().is_symlink(),
                "unsafe ledger path"
            );
        }
        if path.exists() {
            ensure!(
                fs::metadata(&path)?.len() <= LEDGER_LIMIT_BYTES,
                "storage_blocked: existing ledger exceeds PW2 4MiB profile; never truncate it"
            );
        }
        let db = Connection::open(&path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        db.execute_batch("PRAGMA page_size=4096;PRAGMA max_page_count=1024;PRAGMA journal_mode=DELETE;PRAGMA synchronous=FULL;PRAGMA busy_timeout=1000;PRAGMA cache_spill=OFF;PRAGMA temp_store=MEMORY;")?;
        let page_size: i64 = db.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        ensure!(
            page_size == 4096,
            "storage_blocked: existing ledger has incompatible page size; never rebuild it"
        );
        db.execute_batch("CREATE TABLE IF NOT EXISTS sources(path TEXT PRIMARY KEY,hash TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS compacted_sources(path TEXT PRIMARY KEY,hash TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS lines(hash TEXT PRIMARY KEY,line TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY,book TEXT NOT NULL,day TEXT NOT NULL,start_ms INTEGER,end_ms INTEGER,extent INTEGER,state TEXT NOT NULL,body TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS plans(id TEXT PRIMARY KEY,state TEXT NOT NULL,body TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS attempts(id TEXT PRIMARY KEY,state TEXT NOT NULL,body TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
   CREATE INDEX IF NOT EXISTS session_counter_range ON sessions(extent,day,start_ms,end_ms);
   CREATE INDEX IF NOT EXISTS session_book_state ON sessions(book,state,day);")?;
        let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        ensure!(
            pages <= LEDGER_MAX_PAGES,
            "storage_blocked: ledger exceeds 4 MiB PW2 limit"
        );
        Ok(Self {
            root: root.to_owned(),
            db,
            _lock: lock,
        })
    }
    pub fn bind(&self, c: &Config) -> Result<()> {
        let identity = json!([
            c.account_hash(),
            c.device_id,
            c.bootstrap_cutoff_unix,
            c.profile,
            c.firmware
        ])
        .to_string();
        let held: Option<String> = self
            .db
            .query_row("SELECT value FROM meta WHERE key='identity'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(v) = held {
            ensure!(
                v == identity,
                "cannot replace installation/account/bootstrap ledger identity"
            );
        } else {
            storage_budget(&self.root, ROUTINE_TRANSACTION_BYTES)?;
            self.db
                .execute("INSERT INTO meta VALUES ('identity',?)", [identity])?;
        }
        Ok(())
    }
    pub fn status(&self) -> Result<Value> {
        let mut states = serde_json::Map::new();
        let mut stmt=self.db.prepare("SELECT state,count(*),coalesce(sum(json_extract(body,'$.seconds')),0) FROM sessions GROUP BY state")?;
        let (mut pending, mut confirmed) = (0i64, 0i64);
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })? {
            let (state, n, seconds) = row?;
            if state == "confirmed" {
                confirmed += seconds
            } else if [
                "pending",
                "dispatching",
                "unknown",
                "needs_review",
                "failed",
            ]
            .contains(&state.as_str())
            {
                pending += seconds;
            }
            states.insert(state, json!({"sessions":n,"seconds":seconds}));
        }
        let unknown:i64=self.db.query_row("SELECT count(*) FROM attempts WHERE state IN ('unknown','needs_review','dispatching','failed')",[],|r|r.get(0))?;
        let attempts: i64 = self
            .db
            .query_row("SELECT count(*) FROM attempts", [], |r| r.get(0))?;
        let mut out = json!({"schema_version":1,"state":"unconfigured","configured":false,"authenticated":false,"states":states,"attempts":attempts,"pending_seconds":pending,"confirmed_seconds":confirmed,"unknown_batches":unknown,"collect_only_default":true,"auto_sync_enabled":false,"validation":{"device_validation":false,"protocol_validation":false}});
        out["capture"] = capture_diagnostics(&self.root);
        // Configuration metadata only; no log lines, content, auth or account ID.
        // Unmatched books remain visible so a first installation can map exact IDs.
        let mut books = Vec::new();
        let mut query = self
            .db
            .prepare("SELECT book,body FROM sessions GROUP BY book ORDER BY book LIMIT 200")?;
        for row in query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (local, body) = row?;
            if local.is_empty() || local.len() > 1024 || local.contains('\0') {
                continue;
            }
            let body: Value = serde_json::from_str(&body)?;
            let title = body["title"].as_str().unwrap_or("");
            if title.len() <= 4096 && !title.contains('\0') {
                books.push(json!({"local_book_id":local,"title":title}));
            }
        }
        out["books"] = json!(books);
        match Config::load(&self.root) {
            Ok(c) => {
                let binding = c.binding()?;
                let bound = c.validation.binding_sha256 == binding;
                let device = c.validation.device_validation
                    && bound
                    && formal_device_gate(&self.root, &c).is_ok();
                let protocol = c.validation.protocol_validation && bound;
                let authenticated = Auth::load(&self.root, &c).is_ok();
                let preview = experimental::status(&self.root, &c);
                let enabled = ((device && protocol) || preview["automatic_authorized"] == true)
                    && authenticated;
                out["experimental"] = preview;
                out["configured"] = json!(true);
                out["authenticated"] = json!(authenticated);
                out["account_hash"] = json!(c.account_hash());
                out["binding_sha256"] = json!(binding);
                out["auto_sync_enabled"] = json!(enabled);
                out["validation"] =
                    json!({"device_validation":device,"protocol_validation":protocol});
                out["state"] = json!(if unknown > 0 {
                    "needs_review"
                } else if enabled {
                    "ready"
                } else {
                    "collect_only"
                });
            }
            Err(_) => {
                if self.root.join("config.json").exists() {
                    out["state"] = json!("configuration_error");
                }
            }
        }
        Ok(out)
    }
}

pub fn import_config(store: &Store, inbox: &Path) -> Result<Value> {
    ensure!(
        !fs::canonicalize(inbox)?.starts_with(fs::canonicalize(&store.root)?),
        "handoff inbox must be outside private root"
    );
    let config_bytes = limited(&inbox.join("config.json"), 1024 * 1024)?;
    let auth_bytes = limited(&inbox.join("auth.json"), 16384)?;
    let manifest: Value = serde_json::from_slice(&limited(&inbox.join("handoff.json"), 16384)?)?;
    ensure!(
        manifest["schema_version"] == 1
            && manifest["kind"] == "kindle-native-sync-handoff"
            && manifest["config_sha256"] == sha(&config_bytes)
            && manifest["auth_sha256"] == sha(&auth_bytes),
        "handoff fingerprint mismatch"
    );
    let c: Config = serde_json::from_slice(&config_bytes)?;
    c.validate()?;
    // Import never manufactures acceptance evidence, and cannot import already-enabled writes.
    ensure!(
        !c.validation.protocol_validation && !c.validation.device_validation,
        "handoff validation must start disabled"
    );
    let a: Auth = serde_json::from_slice(&auth_bytes)?;
    ensure!(
        a.vid == c.account_vid
            && a.device_id == c.device_id
            && !a.access_token.is_empty()
            && !a.refresh_token.is_empty(),
        "handoff credentials mismatch"
    );
    store.bind(&c)?;
    atomic_json(&store.root.join("auth.json"), &serde_json::to_value(a)?)?;
    atomic_json(&store.root.join("config.json"), &serde_json::to_value(c)?)?;
    // Only the three previously hashed handoff artifacts are eligible for cleanup.
    fs::remove_file(inbox.join("auth.json"))?;
    fs::remove_file(inbox.join("config.json"))?;
    fs::remove_file(inbox.join("handoff.json"))?;
    if fs::read_dir(inbox)?.next().is_none() {
        fs::remove_dir(inbox)?;
    }
    Ok(json!({"state":"imported","auto_sync_enabled":false}))
}
/// The file reader is deliberately unbuffered: procfs never supplies more than
/// four requested bytes to this helper, and the remaining serial is not read.
fn model_prefix(mut reader: impl std::io::Read) -> Result<String> {
    let mut prefix = [0u8; 4];
    reader.read_exact(&mut prefix)?;
    Ok(std::str::from_utf8(&prefix)?.to_owned())
}
pub fn actual_device() -> Value {
    let firmware = [
        "/etc/prettyversion.txt",
        "/etc/prettyversion",
        "/etc/version.txt",
    ]
    .iter()
    .filter_map(|p| fs::read_to_string(p).ok())
    .find(|s| {
        s.split(|c: char| !c.is_ascii_digit() && c != '.')
            .any(|v| v == "5.12.2.1.1")
    });
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_default()
        .trim()
        .to_owned();
    let prefix = fs::File::open("/proc/usid")
        .ok()
        .and_then(|file| model_prefix(file).ok())
        .unwrap_or_default();
    json!({"architecture":std::env::consts::ARCH,"serial_prefix":prefix,"supported_hardware":prefix=="B0D4","supported_firmware":firmware.is_some(),"kernel_release":kernel,"root":unsafe{libc::geteuid()}==0})
}
pub fn formal_device_gate(root: &Path, c: &Config) -> Result<()> {
    let probe = actual_device();
    ensure!(
        probe["architecture"] == "arm"
            && probe["supported_hardware"] == true
            && probe["supported_firmware"] == true
            && probe["root"] == true,
        "actual device is not the accepted ARM Kindle firmware/root profile"
    );
    let evidence = private_json(&root.join("validation-device.json"))?;
    ensure!(
        evidence["schema_version"] == 1
            && evidence["profile"] == c.profile
            && evidence["firmware"] == c.firmware
            && evidence["config_binding_sha256"] == c.binding()?
            && evidence["kernel_release"] == probe["kernel_release"],
        "device evidence binding mismatch"
    );
    for k in [
        "elf_verified",
        "power_cycle_verified",
        "usb_cycle_verified",
        "performance_verified",
    ] {
        ensure!(evidence[k] == true, "device acceptance evidence incomplete");
    }
    Ok(())
}

pub fn device_gate(root: &Path, c: &Config) -> Result<()> {
    if formal_device_gate(root, c).is_ok() {
        return Ok(());
    }
    experimental::preview_gate(root, c, false)
}

pub fn day_snapshot(v: &Value, day: &str) -> Result<i64> {
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d")?;
    let mid = zone()
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
        .timestamp();
    let base = mid - date.weekday().num_days_from_monday() as i64 * 86400;
    ensure!(
        v["baseTime"].as_i64() == Some(base),
        "wrong statistics period"
    );
    let mut seconds = 0;
    for (k, n) in v["readTimes"].as_object().context("no daily stats")? {
        let stamp: i64 = k.parse()?;
        let n = n.as_i64().context("daily units invalid")?;
        ensure!(
            stamp >= base && stamp < base + 604800 && n >= 0,
            "daily stats out of period"
        );
        if stamp >= mid && stamp < mid + 86400 {
            seconds += n;
        }
    }
    Ok(seconds)
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Position {
    pub seconds: i64,
    pub progress: Option<i64>,
    pub chapter: Option<i64>,
    pub offset: Option<i64>,
}
pub fn position(v: &Value, bid: &str) -> Result<Position> {
    if let Some(id) = v["bookId"].as_str() {
        ensure!(id == bid, "wrong book response");
    }
    let b = v["book"].as_object().context("missing book baseline")?;
    if b.len() == 1 && b.get("recordReadingTime").and_then(Value::as_i64) == Some(0) {
        return Ok(Position {
            seconds: 0,
            progress: None,
            chapter: None,
            offset: None,
        });
    }
    let seconds = b
        .get("readingTime")
        .and_then(Value::as_i64)
        .filter(|n| *n >= 0)
        .context("invalid readingTime")?;
    let fields =
        ["progress", "chapterUid", "chapterOffset"].map(|key| b.get(key).and_then(Value::as_i64));
    ensure!(
        fields.iter().all(Option::is_some) || fields.iter().all(Option::is_none),
        "partial position"
    );
    ensure!(
        fields.iter().flatten().all(|v| *v >= 0),
        "negative position"
    );
    Ok(Position {
        seconds,
        progress: fields[0],
        chapter: fields[1],
        offset: fields[2],
    })
}
pub fn matched(
    before: &Position,
    after: &Position,
    dbefore: i64,
    dafter: i64,
    seconds: i64,
) -> bool {
    after.seconds - before.seconds == seconds
        && dafter - dbefore == seconds
        && position_unchanged(before, after)
}
fn position_unchanged(before: &Position, after: &Position) -> bool {
    let fields = [
        (before.progress, after.progress),
        (before.chapter, after.chapter),
        (before.offset, after.offset),
    ];
    fields.iter().all(|(b, a)| match b {
        Some(v) => *a == Some(*v),
        None => a.is_none() || *a == Some(0),
    })
}

// Only an unchanged or exactly-applied counter can still be a delayed cache.
// Unexpected growth, a changed position or an invalid response stops polling.
fn readback_may_be_delayed(
    before: &Position,
    after: &Position,
    day_before: i64,
    day_after: i64,
    seconds: i64,
) -> bool {
    [0, seconds].contains(&(after.seconds - before.seconds))
        && [0, seconds].contains(&(day_after - day_before))
        && position_unchanged(before, after)
}

fn pause_for_readback(started: std::time::Instant) -> bool {
    let budget = std::time::Duration::from_secs(30);
    #[cfg(not(test))]
    {
        let pause = std::time::Instant::now();
        while pause.elapsed() < std::time::Duration::from_secs(2) {
            if cancelled() || started.elapsed() >= budget {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    !cancelled() && started.elapsed() < budget
}

/// A zero-byte scheduling hint, never an authorization to resend a POST.
/// Only the live collector consumes it; no timer, wake lock or runtime state is
/// created here. Refuse every unsafe existing object instead of replacing it.
fn save_readonly_recheck_hint(root: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let owner = unsafe { libc::geteuid() };
    let directory = fs::symlink_metadata(root)?;
    ensure!(
        directory.is_dir()
            && !directory.file_type().is_symlink()
            && directory.uid() == owner
            && directory.mode() & 0o077 == 0,
        "unsafe private root for read-only recheck hint"
    );
    storage_budget(root, ROUTINE_TRANSACTION_BYTES)?;
    let path = root.join("readonly-recheck.pending");
    let valid = |md: &fs::Metadata| {
        md.is_file()
            && !md.file_type().is_symlink()
            && md.uid() == owner
            && md.mode() & 0o7777 == 0o600
            && md.nlink() == 1
            && md.len() == 0
    };
    let file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(file) => {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            file
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let before = fs::symlink_metadata(&path)?;
            ensure!(valid(&before), "unsafe existing read-only recheck hint");
            let file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(&path)?;
            let opened = file.metadata()?;
            ensure!(
                valid(&opened) && opened.dev() == before.dev() && opened.ino() == before.ino(),
                "read-only recheck hint changed during open"
            );
            file
        }
        Err(error) => return Err(error.into()),
    };
    ensure!(
        valid(&file.metadata()?),
        "unsafe created read-only recheck hint"
    );
    file.sync_all()?;
    fs::File::open(root)?.sync_all()?;
    Ok(())
}

pub fn prepare(store: &Store, book: Option<&str>, trial: bool, max_seconds: i64) -> Result<Value> {
    let rt = gate(&store.root, false, now())?;
    let c = Config::load(&store.root)?;
    store.bind(&c)?;
    let effective_cutoff = c.bootstrap_cutoff_unix.max(rt.bootstrap_unix);
    if trial {
        ensure!(
            (1..=60).contains(&max_seconds) && book.is_some(),
            "trial needs a book and 1..60 second limit"
        );
    }
    let mut stmt = store
        .db
        .prepare("SELECT body FROM sessions WHERE state='pending' ORDER BY day,id")?;
    let mut all: Vec<Value> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .map(|r| Ok(serde_json::from_str(&r?)?))
        .collect::<Result<_>>()?;
    if trial {
        // Validate the newest whole observed window, not an older short
        // window accidentally selected by its source hash.
        all.sort_by(|a, b| {
            b["started_at"]
                .as_str()
                .cmp(&a["started_at"].as_str())
                .then_with(|| b["ended_at"].as_str().cmp(&a["ended_at"].as_str()))
        });
    }
    let mut chosen = Vec::new();
    let mut group: Option<(String, String)> = None;
    for s in all {
        let local = s["local_book_id"].as_str().context("missing book")?;
        if book.is_some_and(|b| b != local) {
            continue;
        }
        if c.mappings.iter().all(|m| m.local_book_id != local) {
            continue;
        }
        let uncertain:i64=store.db.query_row("SELECT count(*) FROM sessions WHERE book=? AND state IN ('dispatching','unknown','needs_review','failed','blocked_overlap')",[local],|r|r.get(0))?;
        if uncertain > 0 {
            continue;
        }
        let g = (
            local.to_owned(),
            s["day"].as_str().context("missing date")?.to_owned(),
        );
        if group.as_ref().is_some_and(|x| x != &g) {
            continue;
        }
        if trial && s["seconds"].as_i64().unwrap_or(i64::MAX) > max_seconds {
            continue;
        }
        parsing::verify_manifest(&store.root, &s)?;
        ensure!(
            epoch(s["started_at"].as_str().context("missing start")?)? >= effective_cutoff,
            "pre-bootstrap session"
        );
        group = Some(g);
        chosen.push(s);
        if trial {
            break;
        }
    }
    if chosen.is_empty() {
        return Ok(json!({"state":"empty_or_blocked","network_write":false}));
    }
    let (local, day) = group.unwrap();
    let mapping = c
        .mappings
        .iter()
        .find(|m| m.local_book_id == local)
        .unwrap();
    let mut hours = std::collections::BTreeMap::<i64, i64>::new();
    for s in &chosen {
        for h in s["hours"].as_array().context("missing hours")? {
            *hours
                .entry(h["startOfHour"].as_i64().context("invalid hour")?)
                .or_default() += h["readingTime"].as_i64().context("invalid seconds")?;
        }
    }
    ensure!(
        hours.values().all(|n| *n > 0 && *n <= 3600),
        "overlapping session hour totals"
    );
    let hours:Vec<Value>=hours.into_iter().map(|(epoch,seconds)|json!({"startOfHour":epoch,"timeZone":"+08:00","readingTime":seconds,"ttsTime":0})).collect();
    let seconds: i64 = chosen.iter().map(|s| s["seconds"].as_i64().unwrap()).sum();
    let mut plan = json!({"schema_version":1,"created_unix":now(),"trial":trial,"binding_sha256":c.binding()?,"account_hash":c.account_hash(),"local_book_id":local,"weread_book_id":mapping.weread_book_id,"title":mapping.title,"day":day,"seconds":seconds,"hours":hours,"sessions":chosen});
    if trial {
        experimental::bind_trial_plan(&store.root, &c, &mut plan);
    }
    let id = sha(plan.to_string());
    plan["plan_id"] = json!(id);
    storage_budget(&store.root, ROUTINE_TRANSACTION_BYTES)?;
    store.db.execute(
        "INSERT OR IGNORE INTO plans VALUES (?,'prepared',?)",
        params![id, plan.to_string()],
    )?;
    Ok(json!({"state":"prepared","plan":plan,"network_write":false}))
}
fn verify_plan(store: &Store, c: &Config, plan: &Value, trial: bool) -> Result<()> {
    ensure!(
        plan["binding_sha256"] == c.binding()?
            && plan["account_hash"] == c.account_hash()
            && plan["trial"] == trial,
        "plan configuration changed"
    );
    ensure!(
        c.mappings.iter().any(
            |m| Some(m.local_book_id.as_str()) == plan["local_book_id"].as_str()
                && Some(m.weread_book_id.as_str()) == plan["weread_book_id"].as_str()
        ),
        "mapping changed"
    );
    let overlaps: i64 = store.db.query_row(
        "SELECT count(*) FROM sessions WHERE book=? AND state='blocked_overlap'",
        [plan["local_book_id"].as_str().context("missing book")?],
        |r| r.get(0),
    )?;
    ensure!(overlaps == 0, "ambiguous native overlap blocks this book");
    if trial {
        experimental::verify_preview_trial_plan(&store.root, c, plan)?;
        ensure!(
            plan["sessions"]
                .as_array()
                .context("missing sessions")?
                .len()
                == 1
                && plan["seconds"]
                    .as_i64()
                    .is_some_and(|n| (1..=60).contains(&n)),
            "trial must be one whole real session <=60 seconds"
        );
    }
    for s in plan["sessions"].as_array().context("missing sessions")? {
        parsing::verify_manifest(&store.root, s)?;
        let state: String = store.db.query_row(
            "SELECT state FROM sessions WHERE id=?",
            [s["id"].as_str().context("missing id")?],
            |r| r.get(0),
        )?;
        ensure!(state == "pending", "session already sent or blocked");
    }
    Ok(())
}
/// Mockable verification core. Production callers must use `commit`, which also probes hardware.
/// Keeping this separately callable allows tests without faking ARM/firmware state.
pub fn commit_verified<T: network::Transport>(
    store: &Store,
    id: &str,
    trial: bool,
    transport: &mut T,
) -> Result<Value> {
    storage_budget(&store.root, ROUTINE_TRANSACTION_BYTES)?;
    let c = Config::load(&store.root)?;
    store.bind(&c)?;
    let mut a = Auth::load(&store.root, &c)?;
    gate(&store.root, true, now())?;
    let unresolved:i64=store.db.query_row("SELECT count(*) FROM attempts WHERE state IN ('dispatching','unknown','needs_review','failed')",[],|r|r.get(0))?;
    ensure!(
        unresolved == 0,
        "unresolved write blocks new account submissions"
    );
    ensure!(
        experimental::write_authorized(&store.root, &c, trial),
        "acceptance gates disabled"
    );
    let (state, body): (String, String) =
        store
            .db
            .query_row("SELECT state,body FROM plans WHERE id=?", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
    ensure!(state == "prepared", "plan already dispatched");
    let plan: Value = serde_json::from_str(&body)?;
    verify_plan(store, &c, &plan, trial)?;
    let bid = plan["weread_book_id"].as_str().context("missing target")?;
    let mut refreshed = false;
    let baseline = network::preflight(
        transport,
        &store.root,
        &mut a,
        &mut refreshed,
        "/book/getProgress",
        &[("bookId", bid.into())],
    )?;
    let server_time = baseline["timestamp"]
        .as_i64()
        .context("server clock unavailable")?;
    ensure!(
        (server_time - now()).abs() <= 600,
        "device/server clock mismatch; no write"
    );
    let before = position(&baseline, bid)?;
    let cfg = network::preflight(
        transport,
        &store.root,
        &mut a,
        &mut refreshed,
        "/config",
        &[("token", "1".into())],
    )?;
    let features = network::preflight(
        transport,
        &store.root,
        &mut a,
        &mut refreshed,
        "/feature",
        &[("synckey", "0".into())],
    )?;
    let date = plan["day"].as_str().context("missing day")?;
    let epoch = plan["hours"][0]["startOfHour"]
        .as_i64()
        .context("missing epoch")?;
    let query = [("mode", "weekly".into()), ("baseTime", epoch.to_string())];
    let day_before = day_snapshot(
        &network::preflight(
            transport,
            &store.root,
            &mut a,
            &mut refreshed,
            "/readdata/detail",
            &query,
        )?,
        date,
    )?;
    let payload = protocol::packet(
        &a,
        bid,
        plan["hours"].as_array().unwrap(),
        &cfg,
        &features,
        chrono::Utc::now().timestamp_millis(),
        protocol::random()?,
        protocol::random()?,
    )?;
    gate(&store.root, true, now())?;
    let current = Config::load(&store.root)?;
    ensure!(
        current.binding()? == c.binding()?
            && experimental::write_authorized(&store.root, &current, trial),
        "configuration changed during preflight"
    );
    verify_plan(store, &c, &plan, trial)?;
    // The durable claim is the at-most-once boundary. No network request follows a failed claim.
    check_cancel()?;
    storage_budget(&store.root, ROUTINE_TRANSACTION_BYTES)?;
    let tx = store.db.unchecked_transaction()?;
    let changed = tx.execute(
        "UPDATE plans SET state='dispatching' WHERE id=? AND state='prepared'",
        [id],
    )?;
    ensure!(changed == 1, "claim lost");
    let pending = json!({"plan_id":id,"before":before,"day_before":day_before,"plan":plan});
    tx.execute(
        "INSERT INTO attempts VALUES (?,'dispatching',?)",
        params![id, pending.to_string()],
    )?;
    for s in plan["sessions"].as_array().unwrap() {
        ensure!(
            tx.execute(
                "UPDATE sessions SET state='dispatching' WHERE id=? AND state='pending'",
                [s["id"].as_str().unwrap()]
            )? == 1,
            "session claim lost"
        );
    }
    tx.commit()?;
    let response = transport.request("POST", "/book/batchUploadProgress", &[], Some(&payload), &a);
    let ack = response
        .as_ref()
        .is_ok_and(|v| v["succ"] == 1 || v["succ"] == true);
    let mut state = if ack { "needs_review" } else { "unknown" };
    let mut after = None;
    let mut day_after = None;
    let mut readback_rounds = 0;
    let readback_started = std::time::Instant::now();
    let seconds = plan["seconds"].as_i64().unwrap();
    // Finish the first GET pair even after cancellation of an already-sent
    // POST. An acknowledged, plausible delayed cache may get up to eight pairs,
    // spaced two seconds apart. A new pair starts only within a 30s wall budget;
    // the current pair still has the transport's two separate 20s timeouts.
    // No POST replay or token renewal is permitted beyond the durable claim.
    for round in 0..8 {
        if round > 0 && !pause_for_readback(readback_started) {
            break;
        }
        readback_rounds += 1;
        let got = transport
            .request(
                "GET",
                "/book/getProgress",
                &[("bookId", bid.into())],
                None,
                &a,
            )
            .ok()
            .and_then(|v| position(&v, bid).ok());
        let daily = transport
            .request("GET", "/readdata/detail", &query, None, &a)
            .ok()
            .and_then(|v| day_snapshot(&v, date).ok());
        // Keep each successfully observed counter for diagnostics, including
        // when the other response failed; only a complete pair can confirm.
        after = got;
        day_after = daily;
        let (Some(p), Some(d)) = (&after, day_after) else {
            break;
        };
        if ack && matched(&before, p, day_before, d, seconds) {
            state = "confirmed";
            break;
        }
        if !ack || !readback_may_be_delayed(&before, p, day_before, d, seconds) || cancelled() {
            break;
        }
    }
    let mut receipt = json!({"plan_id":id,"state":state,"acknowledged":ack,"seconds":plan["seconds"],"before":before,"after":after,"day_before":day_before,"day_after":day_after,"readback_rounds":readback_rounds,"replay_allowed":false});
    storage_budget(&store.root, ROUTINE_TRANSACTION_BYTES)?;
    let tx = store.db.unchecked_transaction()?;
    tx.execute(
        "UPDATE attempts SET state=?,body=? WHERE id=?",
        params![state, receipt.to_string(), id],
    )?;
    tx.execute("UPDATE plans SET state=? WHERE id=?", params![state, id])?;
    for s in plan["sessions"].as_array().unwrap() {
        tx.execute(
            "UPDATE sessions SET state=? WHERE id=?",
            params![state, s["id"].as_str().unwrap()],
        )?;
    }
    tx.commit()?;
    // A hint failure cannot turn a real receipt into a failed/confirmed upload.
    // Publish it only after the acknowledged ownership is durably recorded.
    receipt["readonly_recheck_hint_saved"] = json!(
        ack && state == "needs_review"
            && after
                .as_ref()
                .zip(day_after)
                .is_some_and(|(p, d)| readback_may_be_delayed(&before, p, day_before, d, seconds))
            && save_readonly_recheck_hint(&store.root).is_ok()
    );
    Ok(receipt)
}
pub fn commit<T: network::Transport>(
    store: &Store,
    id: &str,
    trial: bool,
    transport: &mut T,
) -> Result<Value> {
    let c = Config::load(&store.root)?;
    device_gate(&store.root, &c)?;
    commit_verified(store, id, trial, transport)
}

/// Drain older book/day groups while the actual reader remains idle and the
/// existing network stays connected. Each plan owns exactly one POST; uncertain
/// results stop the entire drain and are never automatically replayed.
pub fn drain<T: network::Transport>(store: &Store, transport: &mut T) -> Result<Value> {
    drain_loop(store, transport, true)
}
fn drain_loop<T: network::Transport>(
    store: &Store,
    transport: &mut T,
    physical: bool,
) -> Result<Value> {
    let mut batches = 0u64;
    let mut seconds = 0i64;
    loop {
        if check_cancel().is_err() {
            return Ok(
                json!({"state":"paused","reason":"cancelled","confirmed_batches":batches,"confirmed_seconds":seconds}),
            );
        }
        let c = Config::load(&store.root)?;
        if !experimental::write_authorized(&store.root, &c, false) {
            return Ok(
                json!({"state":"collect_only","network_write":false,"confirmed_batches":batches,"confirmed_seconds":seconds}),
            );
        }
        if physical {
            device_gate(&store.root, &c)?;
        }
        if manual::gate_manual(&store.root, true).is_err() {
            return Ok(
                json!({"state":"paused","reason":"device_not_idle_awake_connected","confirmed_batches":batches,"confirmed_seconds":seconds}),
            );
        }
        // Acknowledged writes may become visible after the original worker
        // exits. Recheck them using GET only before considering any new POST.
        // Unknown/unacknowledged ownership stops everything without a request.
        let unresolved = {
            let mut stmt = store.db.prepare(
                "SELECT id,state,body FROM attempts WHERE state IN ('dispatching','unknown','needs_review','failed') ORDER BY id",
            )?;
            stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        if unresolved.iter().any(|(_, state, body)| {
            state != "needs_review"
                || serde_json::from_str::<Value>(body).map_or(true, |v| v["acknowledged"] != true)
        }) {
            return Ok(
                json!({"state":"needs_review","reason":"unresolved_write","confirmed_batches":batches,"confirmed_seconds":seconds}),
            );
        }
        for (id, _, _) in unresolved {
            if reconcile(store, &id, transport)?["state"] != "confirmed" {
                return Ok(
                    json!({"state":"needs_review","reason":"acknowledged_write_not_yet_verified","confirmed_batches":batches,"confirmed_seconds":seconds}),
                );
            }
        }
        let unresolved:i64=store.db.query_row("SELECT count(*) FROM attempts WHERE state IN ('dispatching','unknown','needs_review','failed')",[],|r|r.get(0))?;
        if unresolved > 0 {
            return Ok(
                json!({"state":"needs_review","reason":"unresolved_write","confirmed_batches":batches,"confirmed_seconds":seconds}),
            );
        }
        let p = prepare(store, None, false, 86400)?;
        let Some(id) = p["plan"]["plan_id"].as_str() else {
            return Ok(
                json!({"state":"drained_or_blocked","confirmed_batches":batches,"confirmed_seconds":seconds,"remaining":p}),
            );
        };
        let result = if physical {
            commit(store, id, false, transport)?
        } else {
            commit_verified(store, id, false, transport)?
        };
        if result["state"] != "confirmed" {
            return Ok(
                json!({"state":"needs_review","confirmed_batches":batches,"confirmed_seconds":seconds,"last_result":result}),
            );
        }
        batches += 1;
        seconds += result["seconds"]
            .as_i64()
            .context("missing confirmed duration")?;
    }
}

pub fn export(store: &Store, destination: &Path) -> Result<Value> {
    ensure!(
        !destination.starts_with(&store.root),
        "export outside private root"
    );
    fs::create_dir_all(destination)?;
    let mut sessions = Vec::new();
    let mut stmt = store
        .db
        .prepare("SELECT state,body FROM sessions ORDER BY day,id")?;
    for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (state, text) = r?;
        let mut v: Value = serde_json::from_str(&text)?;
        v["state"] = json!(state);
        v.as_object_mut().unwrap().remove("sources");
        sessions.push(v);
    }
    atomic_json(&destination.join("sessions.json"), &json!(sessions))?;
    atomic_json(&destination.join("status.json"), &store.status()?)?;
    Ok(json!({"state":"exported","sessions":sessions.len(),"credentials_exported":false}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{NetError, Transport};
    use tempfile::TempDir;
    fn config() -> Config {
        Config {
            schema_version: 1,
            device_id: "test-device".into(),
            account_vid: "synthetic-account".into(),
            profile: "pw2-5.12.2.1.1".into(),
            firmware: "5.12.2.1.1".into(),
            timezone: "Asia/Shanghai".into(),
            bootstrap_cutoff_unix: 1000,
            mappings: vec![Mapping {
                local_book_id: "A".into(),
                weread_book_id: "example-book".into(),
                title: "Synthetic Book".into(),
            }],
            validation: Validation::default(),
        }
    }
    fn fixture() -> (TempDir, Store, Config) {
        TEST_CANCEL.with(|v| v.set(false));
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
        let s = Store::open(root).unwrap();
        let mut c = config();
        c.validation.device_validation = true;
        c.validation.binding_sha256 = c.binding().unwrap();
        atomic_json(
            &root.join("config.json"),
            &serde_json::to_value(&c).unwrap(),
        )
        .unwrap();
        atomic_json(&root.join("runtime.json"),&json!({"schema_version":1,"reader_state":"idle","power_state":"awake","usb_export":false,"network_connected":true,"idle_since_unix":now()-30,"bootstrap_unix":1000})).unwrap();
        atomic_json(&root.join("auth.json"),&json!({"vid":"synthetic-account","deviceId":"test-device","accessToken":"synthetic-access","refreshToken":"synthetic-refresh"})).unwrap();
        fs::create_dir_all(root.join("events/sealed")).unwrap();
        let path = root.join("events/sealed/synthetic.log");
        fs::write(&path, "synthetic counted evidence\n").unwrap();
        let body = json!({"id":"session-a","local_book_id":"A","title":"Synthetic Book","day":"2026-11-01","started_at":"2026-11-01T22:13:12","ended_at":"2026-11-01T22:13:25","seconds":13,"hours":[{"startOfHour":1793541600i64,"timeZone":"+08:00","readingTime":13,"ttsTime":0}],"clock_verified":true,"sources":[{"path":path,"sha256":sha(fs::read(&path).unwrap())}],"bootstrap_cutoff_unix":1000});
        s.db.execute(
            "INSERT INTO sessions VALUES ('session-a','A','2026-11-01',0,13000,100,'pending',?)",
            [body.to_string()],
        )
        .unwrap();
        (tmp, s, c)
    }
    #[derive(Default)]
    struct Mock {
        posts: usize,
        total: i64,
        day: i64,
        mode: &'static str,
        refreshes: usize,
        auth_fail_once: bool,
        calls: Vec<String>,
        book_readbacks: usize,
        day_readbacks: usize,
    }
    impl Transport for Mock {
        fn request(
            &mut self,
            method: &str,
            path: &str,
            _: &[(&str, String)],
            body: Option<&Value>,
            _: &Auth,
        ) -> std::result::Result<Value, NetError> {
            self.calls.push(format!("{method} {path}"));
            match path {
                "/login" => {
                    self.refreshes += 1;
                    if self.mode == "wrong_account" {
                        Ok(json!({"vid":"wrong-account","accessToken":"fresh"}))
                    } else {
                        Ok(
                            json!({"vid":"synthetic-account","accessToken":"fresh","refreshToken":"fresh-refresh"}),
                        )
                    }
                }
                "/book/getProgress" => {
                    if self.auth_fail_once {
                        self.auth_fail_once = false;
                        return Err(NetError::Auth);
                    }
                    if self.posts > 0 {
                        self.book_readbacks += 1;
                        if self.mode == "post_read_auth" {
                            return Err(NetError::Auth);
                        }
                        if self.mode == "cancel_during_readback" {
                            request_cancel();
                        }
                    }
                    let total =
                        if self.mode == "delayed_book" && self.posts > 0 && self.book_readbacks < 3
                        {
                            0
                        } else {
                            self.total
                        };
                    let progress = if self.mode == "position_changed" && self.posts > 0 {
                        11
                    } else {
                        10
                    };
                    Ok(
                        json!({"bookId":"example-book","timestamp":now(),"book":{"readingTime":100+total,"progress":progress,"chapterUid":2,"chapterOffset":7}}),
                    )
                }
                "/config" => {
                    if self.mode == "cancel_before" {
                        request_cancel();
                    }
                    Ok(json!({"token":"synthetic-config"}))
                }
                "/feature" => Ok(json!({})),
                "/readdata/detail" => {
                    if self.posts > 0 {
                        self.day_readbacks += 1;
                    }
                    let day =
                        if self.mode == "delayed_day" && self.posts > 0 && self.day_readbacks < 3 {
                            0
                        } else {
                            self.day
                        };
                    Ok(
                        json!({"baseTime":if self.mode=="wrong_week"{0}else{1792944000i64},"readTimes":{"1793462400":day}}),
                    )
                }
                "/book/batchUploadProgress" => {
                    self.posts += 1;
                    if self.mode == "cancel_after" || self.mode == "cancel_stale" {
                        request_cancel();
                    }
                    if self.mode == "panic" {
                        panic!("simulated process death after durable claim")
                    };
                    if self.mode == "timeout" {
                        return Err(NetError::Uncertain);
                    }
                    if self.mode != "ack_only"
                        && self.mode != "cancel_stale"
                        && self.mode != "cancel_during_readback"
                    {
                        let secs = body.unwrap()["books"][0]["readingTime"].as_i64().unwrap();
                        if self.mode != "day_only" {
                            self.total += secs;
                        }
                        if self.mode == "unexpected_book_growth" {
                            self.total += 1;
                        }
                        if self.mode != "wrong_day" {
                            self.day += secs;
                            if self.mode == "unexpected_day_growth" {
                                self.day += 1;
                            }
                        }
                    }
                    Ok(json!({"succ":1}))
                }
                _ => Err(NetError::Invalid),
            }
        }
    }
    fn plan(s: &Store) -> String {
        prepare(s, Some("A"), true, 60).unwrap()["plan"]["plan_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    #[test]
    fn signature_reference_vector() {
        let payload = "393727764_35645832171399234841676621530100678335_35645832171399234841676621530100678335_CB_00K1XI1aSDlZ76f75hG9n7JG_0_+08:00_81_0_1792317600_+08:00_81_0";
        assert_eq!(
            protocol::sign(&[
                protocol::DEFAULT_GUEST.into(),
                "821".into(),
                "1792318161".into(),
                payload.into()
            ]),
            "dfd6f2ceb1e4a5d1152c6d19fd9c9f84115cad69989ee799faec377d05d8a833"
        );
    }
    #[test]
    fn runtime_unknown_active_usb_and_short_idle_block() {
        let (_t, s, _) = fixture();
        for (k, v) in [
            ("reader_state", json!("active")),
            ("power_state", json!("unknown")),
            ("usb_export", json!(true)),
            ("idle_since_unix", json!(now() - 5)),
        ] {
            let p = s.root.join("runtime.json");
            let original = private_json(&p).unwrap();
            let mut altered = original.clone();
            altered[k] = v;
            atomic_json(&p, &altered).unwrap();
            assert!(gate(&s.root, false, now()).is_err());
            atomic_json(&p, &original).unwrap();
        }
        assert!(gate(&s.root, false, now()).is_ok());
    }
    #[test]
    fn offline_allows_collect_gate_only() {
        let (_t, s, _) = fixture();
        let p = s.root.join("runtime.json");
        let mut v = private_json(&p).unwrap();
        v["network_connected"] = json!(false);
        atomic_json(&p, &v).unwrap();
        assert!(gate(&s.root, false, now()).is_ok());
        assert!(gate(&s.root, true, now()).is_err());
    }
    #[test]
    fn missing_runtime_fields_block() {
        let (_t, s, _) = fixture();
        let p = s.root.join("runtime.json");
        let mut v = private_json(&p).unwrap();
        v.as_object_mut().unwrap().remove("usb_export");
        atomic_json(&p, &v).unwrap();
        assert!(gate(&s.root, false, now()).is_err());
    }
    #[test]
    fn second_worker_cannot_open_lock() {
        let (_t, s, _) = fixture();
        assert!(Store::open(&s.root).is_err());
    }
    #[test]
    fn config_binding_excludes_acceptance_and_binds_mappings() {
        let mut c = config();
        let before = c.binding().unwrap();
        c.validation.protocol_validation = true;
        assert_eq!(before, c.binding().unwrap());
        c.mappings[0].weread_book_id = "another".into();
        assert_ne!(before, c.binding().unwrap());
    }
    #[test]
    fn one_post_confirmed_and_replay_blocked() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock::default();
        let v = commit_verified(&s, &id, true, &mut m).unwrap();
        assert_eq!(v["state"], "confirmed");
        assert_eq!(m.posts, 1);
        assert!(commit_verified(&s, &id, true, &mut m).is_err());
        assert_eq!(m.posts, 1);
    }
    #[test]
    fn ack_without_increment_blocks_replay() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "ack_only",
            ..Default::default()
        };
        assert_eq!(
            commit_verified(&s, &id, true, &mut m).unwrap()["state"],
            "needs_review"
        );
        assert!(commit_verified(&s, &id, true, &mut m).is_err());
        assert_eq!(m.posts, 1);
        assert_eq!(
            prepare(&s, Some("A"), true, 60).unwrap()["state"],
            "empty_or_blocked"
        );
    }
    #[test]
    fn delayed_book_or_day_cache_confirms_without_another_post() {
        for mode in ["delayed_book", "delayed_day"] {
            let (_t, s, _) = fixture();
            let id = plan(&s);
            let mut m = Mock {
                mode,
                ..Default::default()
            };
            let receipt = commit_verified(&s, &id, true, &mut m).unwrap();
            assert_eq!(receipt["state"], "confirmed", "{mode}");
            assert_eq!(receipt["readback_rounds"], 3);
            assert_eq!(receipt["after"]["seconds"], 113);
            assert_eq!(receipt["day_after"], 13);
            assert_eq!(
                (m.posts, m.refreshes, m.book_readbacks, m.day_readbacks),
                (1, 0, 3, 3)
            );
        }
    }
    #[test]
    fn delayed_readbacks_are_bounded_and_unexpected_changes_stop_immediately() {
        for (mode, rounds) in [
            ("ack_only", 8),
            ("position_changed", 1),
            ("unexpected_book_growth", 1),
            ("unexpected_day_growth", 1),
            ("cancel_stale", 1),
            ("cancel_during_readback", 1),
            ("post_read_auth", 1),
        ] {
            let (_t, s, _) = fixture();
            let id = plan(&s);
            let mut m = Mock {
                mode,
                ..Default::default()
            };
            let receipt = commit_verified(&s, &id, true, &mut m).unwrap();
            assert_eq!(receipt["state"], "needs_review", "{mode}");
            assert_eq!(receipt["readback_rounds"], rounds, "{mode}");
            assert_eq!(m.posts, 1);
            assert_eq!(m.refreshes, 0);
            assert_eq!(m.book_readbacks, rounds);
            assert_eq!(m.day_readbacks, rounds);
        }
    }
    #[test]
    fn readback_budget_or_cancellation_prevents_waiting_for_another_pair() {
        TEST_CANCEL.with(|v| v.set(false));
        assert!(!pause_for_readback(
            std::time::Instant::now() - std::time::Duration::from_secs(30)
        ));
        request_cancel();
        assert!(!pause_for_readback(std::time::Instant::now()));
        TEST_CANCEL.with(|v| v.set(false));
    }
    #[test]
    fn readonly_recheck_hint_is_private_empty_single_link_and_idempotent() {
        use std::os::unix::fs::MetadataExt;
        let (_t, s, _) = fixture();
        save_readonly_recheck_hint(&s.root).unwrap();
        let path = s.root.join("readonly-recheck.pending");
        let before = fs::symlink_metadata(&path).unwrap();
        assert!(before.is_file());
        assert_eq!(
            (before.mode() & 0o7777, before.len(), before.nlink()),
            (0o600, 0, 1)
        );
        assert_eq!(before.uid(), unsafe { libc::geteuid() });
        save_readonly_recheck_hint(&s.root).unwrap();
        assert_eq!(fs::symlink_metadata(path).unwrap().ino(), before.ino());
    }
    #[test]
    fn readonly_recheck_hint_refuses_links_loose_modes_content_and_directories() {
        use std::os::unix::fs::symlink;
        for kind in [
            "symlink",
            "hardlink",
            "loose",
            "content",
            "directory",
            "fifo",
        ] {
            let (_t, s, _) = fixture();
            let path = s.root.join("readonly-recheck.pending");
            let target = s.root.join("untouched-marker-target");
            fs::write(&target, []).unwrap();
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
            match kind {
                "symlink" => symlink(&target, &path).unwrap(),
                "hardlink" => fs::hard_link(&target, &path).unwrap(),
                "directory" => fs::create_dir(&path).unwrap(),
                "fifo" => {
                    use std::os::unix::ffi::OsStrExt;
                    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
                }
                "loose" => {
                    fs::write(&path, []).unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
                }
                "content" => {
                    fs::write(&path, b"not-empty").unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                }
                _ => unreachable!(),
            }
            assert!(save_readonly_recheck_hint(&s.root).is_err(), "{kind}");
            assert_eq!(fs::read(&target).unwrap(), b"");
        }
    }
    #[test]
    fn recheck_hint_only_tracks_plausible_acknowledged_results_and_failures_do_not_replace_receipts()
     {
        for mode in [
            "ack_only",
            "timeout",
            "position_changed",
            "unexpected_book_growth",
            "unexpected_day_growth",
            "post_read_auth",
        ] {
            let (_t, s, _) = fixture();
            let id = plan(&s);
            let mut transport = Mock {
                mode,
                ..Default::default()
            };
            let receipt = commit_verified(&s, &id, true, &mut transport).unwrap();
            let expected = mode == "ack_only";
            assert_eq!(receipt["readonly_recheck_hint_saved"], expected, "{mode}");
            assert_eq!(
                s.root.join("readonly-recheck.pending").exists(),
                expected,
                "{mode}"
            );
        }
        let (_t, s, _) = fixture();
        let id = plan(&s);
        fs::create_dir(s.root.join("readonly-recheck.pending")).unwrap();
        let mut transport = Mock {
            mode: "ack_only",
            ..Default::default()
        };
        let receipt = commit_verified(&s, &id, true, &mut transport).unwrap();
        assert_eq!(receipt["state"], "needs_review");
        assert_eq!(receipt["readonly_recheck_hint_saved"], false);
        assert_eq!(transport.posts, 1);
        assert_eq!(
            s.db.query_row("SELECT state FROM attempts WHERE id=?", [&id], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "needs_review"
        );
    }
    #[test]
    fn readonly_reconcile_renews_hint_only_while_counters_are_plausibly_delayed() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut transport = Mock {
            mode: "day_only",
            ..Default::default()
        };
        assert_eq!(
            commit_verified(&s, &id, true, &mut transport).unwrap()["readonly_recheck_hint_saved"],
            true
        );
        let marker = s.root.join("readonly-recheck.pending");
        fs::remove_file(&marker).unwrap();
        let pending = reconcile(&s, &id, &mut transport).unwrap();
        assert_eq!(pending["state"], "needs_review");
        assert_eq!(pending["readonly_recheck_hint_saved"], true);
        fs::remove_file(&marker).unwrap();
        transport.total = 13;
        let confirmed = reconcile(&s, &id, &mut transport).unwrap();
        assert_eq!(confirmed["state"], "confirmed");
        assert_eq!(confirmed["readonly_recheck_hint_saved"], false);
        assert!(!marker.exists());
        assert_eq!(transport.posts, 1);
    }
    #[test]
    fn timeout_never_retried() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "timeout",
            ..Default::default()
        };
        assert_eq!(
            commit_verified(&s, &id, true, &mut m).unwrap()["state"],
            "unknown"
        );
        assert_eq!(m.posts, 1);
    }
    #[test]
    fn wrong_day_does_not_confirm() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "wrong_day",
            ..Default::default()
        };
        assert_eq!(
            commit_verified(&s, &id, true, &mut m).unwrap()["state"],
            "needs_review"
        );
    }
    #[test]
    fn wrong_week_stops_before_post() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "wrong_week",
            ..Default::default()
        };
        assert!(commit_verified(&s, &id, true, &mut m).is_err());
        assert_eq!(m.posts, 0);
    }
    #[test]
    fn source_modification_stops_before_network() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        fs::write(s.root.join("events/sealed/synthetic.log"), "changed").unwrap();
        let mut m = Mock::default();
        assert!(commit_verified(&s, &id, true, &mut m).is_err());
        assert!(m.calls.is_empty());
    }
    #[test]
    fn auth_permissions_fail_closed() {
        let (_t, s, c) = fixture();
        fs::set_permissions(s.root.join("auth.json"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Auth::load(&s.root, &c).is_err());
    }
    #[test]
    fn refresh_once_before_dispatch_and_identity_checked() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            auth_fail_once: true,
            ..Default::default()
        };
        assert_eq!(
            commit_verified(&s, &id, true, &mut m).unwrap()["state"],
            "confirmed"
        );
        assert_eq!(m.refreshes, 1);
        assert_eq!(m.posts, 1);
    }
    #[test]
    fn wrong_account_refresh_never_saved_or_uploaded() {
        let (_t, s, c) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            auth_fail_once: true,
            mode: "wrong_account",
            ..Default::default()
        };
        assert!(commit_verified(&s, &id, true, &mut m).is_err());
        assert_eq!(m.posts, 0);
        assert_eq!(
            Auth::load(&s.root, &c).unwrap().access_token,
            "synthetic-access"
        );
    }
    #[test]
    fn dispatch_crash_remains_owned() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "panic",
            ..Default::default()
        };
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            commit_verified(&s, &id, true, &mut m)
        }));
        assert!(crashed.is_err());
        assert_eq!(
            s.db.query_row("SELECT state FROM attempts WHERE id=?", [&id], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "dispatching"
        );
        assert!(commit_verified(&s, &id, true, &mut Mock::default()).is_err());
    }
    #[test]
    fn changed_binding_does_not_dispatch() {
        let (_t, s, mut c) = fixture();
        let id = plan(&s);
        c.mappings[0].weread_book_id = "changed-book".into();
        c.validation.binding_sha256 = c.binding().unwrap();
        atomic_json(
            &s.root.join("config.json"),
            &serde_json::to_value(c).unwrap(),
        )
        .unwrap();
        let mut m = Mock::default();
        assert!(commit_verified(&s, &id, true, &mut m).is_err());
        assert_eq!(m.posts, 0);
    }
    #[test]
    fn unvalidated_automatic_write_blocked_even_for_prepared_plan() {
        let (_t, s, _) = fixture();
        let v = prepare(&s, None, false, 86400).unwrap();
        let id = v["plan"]["plan_id"].as_str().unwrap();
        let mut m = Mock::default();
        assert!(commit_verified(&s, id, false, &mut m).is_err());
        assert!(m.calls.is_empty());
    }
    #[test]
    fn export_excludes_tokens_and_account_id() {
        let (_t, s, _) = fixture();
        let out = TempDir::new().unwrap();
        export(&s, out.path()).unwrap();
        for path in [
            out.path().join("status.json"),
            out.path().join("sessions.json"),
        ] {
            let text = fs::read_to_string(path).unwrap();
            for secret in ["synthetic-access", "synthetic-refresh", "synthetic-account"] {
                assert!(!text.contains(secret));
            }
        }
    }
    #[test]
    fn host_config_flags_do_not_bypass_physical_device_gate() {
        let (_t, s, c) = fixture();
        if std::env::consts::ARCH != "arm" {
            assert!(device_gate(&s.root, &c).is_err());
            let id = plan(&s);
            let mut m = Mock::default();
            assert!(commit(&s, &id, true, &mut m).is_err());
            assert!(m.calls.is_empty());
        }
    }
    #[test]
    fn trial_does_not_split_or_invent_seconds() {
        let (_t, s, _) = fixture();
        assert_eq!(
            prepare(&s, Some("A"), true, 12).unwrap()["state"],
            "empty_or_blocked"
        );
        assert_eq!(
            prepare(&s, Some("A"), true, 13).unwrap()["plan"]["seconds"],
            13
        );
    }
    #[test]
    fn trial_selects_newest_complete_window_while_auto_keeps_backlog() {
        let (_t, s, _) = fixture();
        let body: String =
            s.db.query_row("SELECT body FROM sessions WHERE id='session-a'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let mut newer: Value = serde_json::from_str(&body).unwrap();
        newer["id"] = json!("session-z-new");
        newer["started_at"] = json!("2026-11-01T22:14:12");
        newer["ended_at"] = json!("2026-11-01T22:14:25");
        s.db.execute(
            "INSERT INTO sessions VALUES ('session-z-new','A','2026-11-01',13000,26000,200,'pending',?)",
            [newer.to_string()],
        )
        .unwrap();
        let trial = prepare(&s, Some("A"), true, 60).unwrap();
        assert_eq!(trial["plan"]["sessions"][0]["id"], "session-z-new");
        assert_eq!(trial["plan"]["seconds"], 13);
        let backlog = prepare(&s, None, false, 60).unwrap();
        assert_eq!(backlog["plan"]["sessions"].as_array().unwrap().len(), 2);
        assert_eq!(backlog["plan"]["seconds"], 26);
    }
    #[test]
    fn empty_record_is_narrowly_recognized() {
        assert_eq!(
            position(&json!({"book":{"recordReadingTime":0}}), "B")
                .unwrap()
                .seconds,
            0
        );
        assert!(position(&json!({"book":{"recordReadingTime":false}}), "B").is_err());
        assert!(position(&json!({"book":{"readingTime":0,"chapterUid":0}}), "B").is_err());
    }
    #[test]
    fn packet_is_time_only_and_keeps_hours() {
        let (_t, s, c) = fixture();
        let a = Auth::load(&s.root, &c).unwrap();
        let p = prepare(&s, Some("A"), true, 60).unwrap();
        let v = protocol::packet(
            &a,
            "example-book",
            p["plan"]["hours"].as_array().unwrap(),
            &json!({"token":"config"}),
            &json!({}),
            1793544000456,
            7,
            3,
        )
        .unwrap();
        assert_eq!(v["timestamp"], 1793544000456i64);
        assert_eq!(v["books"][0]["readingTime"], 13);
        assert_eq!(v["books"][0]["hours"], p["plan"]["hours"]);
        assert!(v["books"][0].get("chapterUid").is_none());
    }
    #[test]
    fn cancel_before_claim_sends_no_post() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "cancel_before",
            ..Default::default()
        };
        assert!(commit_verified(&s, &id, true, &mut m).is_err());
        assert_eq!(m.posts, 0);
        assert_eq!(
            s.db.query_row("SELECT count(*) FROM attempts", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn cancel_after_claim_finishes_one_post_and_two_readbacks() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "cancel_after",
            ..Default::default()
        };
        assert_eq!(
            commit_verified(&s, &id, true, &mut m).unwrap()["state"],
            "confirmed"
        );
        assert_eq!(m.posts, 1);
        let post = m
            .calls
            .iter()
            .position(|s| s == "POST /book/batchUploadProgress")
            .unwrap();
        assert_eq!(m.calls.len() - post - 1, 2);
    }
    #[test]
    fn readonly_reconcile_confirms_once_without_post() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "day_only",
            ..Default::default()
        };
        assert_eq!(
            commit_verified(&s, &id, true, &mut m).unwrap()["state"],
            "needs_review"
        );
        m.total = 13;
        m.day = 13;
        let posts = m.posts;
        let receipt = reconcile(&s, &id, &mut m).unwrap();
        assert_eq!(receipt["state"], "confirmed");
        assert_eq!(receipt["original_after"]["seconds"], 100);
        assert_eq!(receipt["original_day_after"], 13);
        assert_eq!(receipt["after"]["seconds"], 113);
        assert_eq!(receipt["day_after"], 13);
        assert_eq!(receipt["after"], receipt["recheck_after"]);
        assert_eq!(receipt["day_after"], receipt["recheck_day_after"]);
        assert_eq!(reconcile(&s, &id, &mut m).unwrap(), receipt);
        assert_eq!(m.posts, posts);
    }
    #[test]
    fn reconcile_preserves_existing_original_observations() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "ack_only",
            ..Default::default()
        };
        let mut receipt = commit_verified(&s, &id, true, &mut m).unwrap();
        receipt["original_after"] = json!({"audit":"existing-original"});
        receipt["original_day_after"] = json!(null);
        s.db.execute(
            "UPDATE attempts SET body=? WHERE id=?",
            params![receipt.to_string(), &id],
        )
        .unwrap();
        m.total = 13;
        m.day = 13;
        let actual = reconcile(&s, &id, &mut m).unwrap();
        assert_eq!(actual["original_after"], receipt["original_after"]);
        assert_eq!(actual["original_day_after"], json!(null));
        assert_eq!(actual["after"]["seconds"], 113);
        let saved: String =
            s.db.query_row(
                "SELECT body FROM attempts WHERE id=? AND state='confirmed'",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&saved).unwrap(), actual);
    }
    #[test]
    fn reconcile_never_replays_unknown_or_refreshes() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "timeout",
            ..Default::default()
        };
        commit_verified(&s, &id, true, &mut m).unwrap();
        let before = m.calls.len();
        assert!(reconcile(&s, &id, &mut m).is_err());
        assert_eq!(m.calls.len(), before);
    }
    #[test]
    fn separate_readonly_reconcile_renews_expired_auth_once_without_time_post() {
        let (_t, s, c) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "day_only",
            ..Default::default()
        };
        commit_verified(&s, &id, true, &mut m).unwrap();
        m.total = 13;
        m.auth_fail_once = true;
        let before = m.calls.len();
        let receipt = reconcile(&s, &id, &mut m).unwrap();
        assert_eq!(receipt["state"], "confirmed");
        assert_eq!(receipt["after"]["seconds"], 113);
        assert_eq!(receipt["day_after"], 13);
        assert_eq!(m.refreshes, 1);
        assert_eq!(m.posts, 1);
        assert_eq!(
            m.calls[before..],
            [
                "GET /book/getProgress",
                "POST /login",
                "GET /book/getProgress",
                "GET /readdata/detail"
            ]
        );
        assert_eq!(Auth::load(&s.root, &c).unwrap().access_token, "fresh");
    }
    #[test]
    fn readonly_reconcile_wrong_account_refresh_stops_without_saving_or_replaying() {
        let (_t, s, c) = fixture();
        let id = plan(&s);
        let mut m = Mock {
            mode: "day_only",
            ..Default::default()
        };
        commit_verified(&s, &id, true, &mut m).unwrap();
        m.total = 13;
        m.mode = "wrong_account";
        m.auth_fail_once = true;
        let before = m.calls.len();
        assert!(reconcile(&s, &id, &mut m).is_err());
        assert_eq!(m.calls[before..], ["GET /book/getProgress", "POST /login"]);
        assert_eq!(m.posts, 1);
        assert_eq!(m.refreshes, 1);
        assert_eq!(
            Auth::load(&s.root, &c).unwrap().access_token,
            "synthetic-access"
        );
        assert_eq!(
            s.db.query_row("SELECT state FROM attempts WHERE id=?", [&id], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "needs_review"
        );
    }
    #[test]
    fn database_has_explicit_four_mib_ceiling() {
        let (_t, s, _) = fixture();
        assert_eq!(
            s.db.query_row("PRAGMA max_page_count", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1024
        );
        assert_eq!(
            s.db.query_row("PRAGMA page_size", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            4096
        );
        assert!(storage_budget(&s.root, u64::MAX - 20 * 1024 * 1024).is_err());
    }
    #[test]
    fn handoff_import_cleans_only_verified_files_and_starts_disabled() {
        let (_t, s, mut c) = fixture();
        c.validation = Validation::default();
        let parent = TempDir::new().unwrap();
        let inbox = parent.path().join("inbox");
        fs::create_dir(&inbox).unwrap();
        let cb = serde_json::to_vec(&c).unwrap();
        let ab=serde_json::to_vec(&json!({"vid":"synthetic-account","deviceId":"test-device","accessToken":"new-synthetic-access","refreshToken":"new-synthetic-refresh"})).unwrap();
        fs::write(inbox.join("config.json"), &cb).unwrap();
        fs::write(inbox.join("auth.json"), &ab).unwrap();
        fs::write(inbox.join("handoff.json"),json!({"schema_version":1,"kind":"kindle-native-sync-handoff","config_sha256":sha(&cb),"auth_sha256":sha(&ab)}).to_string()).unwrap();
        assert_eq!(import_config(&s, &inbox).unwrap()["state"], "imported");
        assert!(!inbox.exists());
        assert!(!Config::load(&s.root).unwrap().validation.device_validation);
        assert_eq!(
            Auth::load(&s.root, &c).unwrap().access_token,
            "new-synthetic-access"
        );
    }
    #[test]
    fn compact_never_deletes_pending_or_unknown_sources() {
        let (_t, s, _) = fixture();
        s.db.execute("INSERT INTO meta VALUES('last_collect_blocked','0')", [])
            .unwrap();
        assert!(compact(&s).is_err());
        assert!(s.root.join("events/sealed/synthetic.log").exists());
    }
    #[test]
    fn compact_confirmed_sources_preserves_identity_and_prevents_replay() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        commit_verified(&s, &id, true, &mut Mock::default()).unwrap();
        let path = s.root.join("events/sealed/synthetic.log");
        let hash = sha(fs::read(&path).unwrap());
        s.db.execute(
            "INSERT INTO sources VALUES (?,?)",
            params![path.to_string_lossy(), hash],
        )
        .unwrap();
        s.db.execute("INSERT INTO meta VALUES('last_collect_blocked','0')", [])
            .unwrap();
        atomic_json(
            &s.root.join("collection-state.json"),
            &json!({"complete":true,"blocked":0}),
        )
        .unwrap();
        assert_eq!(compact(&s).unwrap()["state"], "compacted");
        assert!(!path.exists());
        assert_eq!(s.status().unwrap()["confirmed_seconds"], 13);
        assert!(commit_verified(&s, &id, true, &mut Mock::default()).is_err());
        assert_eq!(
            s.db.query_row(
                "SELECT count(*) FROM sessions WHERE id='session-a'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
    #[test]
    fn automatic_drain_uploads_multiple_books_and_days_without_resending() {
        #[derive(Default)]
        struct Multi {
            books: std::collections::BTreeMap<String, i64>,
            days: std::collections::BTreeMap<i64, i64>,
            posts: usize,
        }
        impl Transport for Multi {
            fn request(
                &mut self,
                method: &str,
                path: &str,
                query: &[(&str, String)],
                body: Option<&Value>,
                _: &Auth,
            ) -> std::result::Result<Value, NetError> {
                match path {
                    "/book/getProgress" => {
                        let bid = &query.iter().find(|(k, _)| *k == "bookId").unwrap().1;
                        Ok(
                            json!({"bookId":bid,"timestamp":now(),"book":{"readingTime":100+self.books.get(bid).copied().unwrap_or(0),"progress":10,"chapterUid":2,"chapterOffset":7}}),
                        )
                    }
                    "/config" => Ok(json!({"token":"synthetic"})),
                    "/feature" => Ok(json!({})),
                    "/readdata/detail" => {
                        let at: i64 = query
                            .iter()
                            .find(|(k, _)| *k == "baseTime")
                            .unwrap()
                            .1
                            .parse()
                            .unwrap();
                        let local = zone().timestamp_opt(at, 0).unwrap();
                        let midnight = at - local.time().num_seconds_from_midnight() as i64;
                        let base = midnight - local.weekday().num_days_from_monday() as i64 * 86400;
                        let days: serde_json::Map<String, Value> = self
                            .days
                            .iter()
                            .filter(|(k, _)| **k >= base && **k < base + 604800)
                            .map(|(k, v)| (k.to_string(), json!(v)))
                            .collect();
                        Ok(json!({"baseTime":base,"readTimes":days}))
                    }
                    "/book/batchUploadProgress" if method == "POST" => {
                        self.posts += 1;
                        let b = &body.unwrap()["books"][0];
                        *self
                            .books
                            .entry(b["bookId"].as_str().unwrap().into())
                            .or_default() += b["readingTime"].as_i64().unwrap();
                        for h in b["hours"].as_array().unwrap() {
                            let at = h["startOfHour"].as_i64().unwrap();
                            let day = (at + 28800).div_euclid(86400) * 86400 - 28800;
                            *self.days.entry(day).or_default() +=
                                h["readingTime"].as_i64().unwrap();
                        }
                        Ok(json!({"succ":1}))
                    }
                    _ => Err(NetError::Invalid),
                }
            }
        }
        use chrono::Timelike;
        let (_t, s, mut c) = fixture();
        c.validation.protocol_validation = true;
        c.mappings.push(Mapping {
            local_book_id: "B".into(),
            weread_book_id: "second-book".into(),
            title: "Second synthetic book".into(),
        });
        c.validation.binding_sha256 = c.binding().unwrap();
        atomic_json(
            &s.root.join("config.json"),
            &serde_json::to_value(&c).unwrap(),
        )
        .unwrap();
        let mut b: Value = serde_json::from_str(
            &s.db
                .query_row("SELECT body FROM sessions", [], |r| r.get::<_, String>(0))
                .unwrap(),
        )
        .unwrap();
        b["id"] = json!("session-b");
        b["local_book_id"] = json!("B");
        b["day"] = json!("2026-11-02");
        b["started_at"] = json!("2026-11-02T09:00:00");
        b["ended_at"] = json!("2026-11-02T09:00:07");
        b["seconds"] = json!(7);
        b["hours"] = json!([{"startOfHour":epoch("2026-11-02T09:00:00").unwrap(),"timeZone":"+08:00","readingTime":7,"ttsTime":0}]);
        s.db.execute(
            "INSERT INTO sessions VALUES ('session-b','B','2026-11-02',0,7000,101,'pending',?)",
            [b.to_string()],
        )
        .unwrap();
        let mut transport = Multi::default();
        let result = drain_loop(&s, &mut transport, false).unwrap();
        assert_eq!(result["confirmed_batches"], 2);
        assert_eq!(result["confirmed_seconds"], 20);
        assert_eq!(transport.posts, 2);
        let again = drain_loop(&s, &mut transport, false).unwrap();
        assert_eq!(again["confirmed_batches"], 0);
        assert_eq!(transport.posts, 2);
    }
    #[test]
    fn automatic_drain_stops_after_first_uncertain_post() {
        let (_t, s, mut c) = fixture();
        c.validation.protocol_validation = true;
        c.validation.binding_sha256 = c.binding().unwrap();
        atomic_json(
            &s.root.join("config.json"),
            &serde_json::to_value(c).unwrap(),
        )
        .unwrap();
        let mut transport = Mock {
            mode: "ack_only",
            ..Default::default()
        };
        let result = drain_loop(&s, &mut transport, false).unwrap();
        assert_eq!(result["state"], "needs_review");
        assert_eq!(transport.posts, 1);
        drain_loop(&s, &mut transport, false).unwrap();
        assert_eq!(transport.posts, 1);
    }
    #[test]
    fn automatic_drain_reconciles_acknowledged_cache_delay_before_new_writes() {
        let (_t, s, mut c) = fixture();
        c.validation.protocol_validation = true;
        c.validation.binding_sha256 = c.binding().unwrap();
        atomic_json(
            &s.root.join("config.json"),
            &serde_json::to_value(c).unwrap(),
        )
        .unwrap();
        let id = plan(&s);
        let mut transport = Mock {
            mode: "ack_only",
            ..Default::default()
        };
        assert_eq!(
            commit_verified(&s, &id, true, &mut transport).unwrap()["state"],
            "needs_review"
        );
        transport.total = 13;
        transport.day = 13;
        let result = drain_loop(&s, &mut transport, false).unwrap();
        assert_eq!(result["state"], "drained_or_blocked");
        assert_eq!(result["confirmed_batches"], 0);
        assert_eq!(transport.posts, 1);
        let saved: String =
            s.db.query_row("SELECT state FROM attempts WHERE id=?", [&id], |r| r.get(0))
                .unwrap();
        assert_eq!(saved, "confirmed");
        assert_eq!(s.status().unwrap()["confirmed_seconds"], 13);
    }
    #[test]
    fn automatic_drain_does_not_contact_network_for_unknown_write() {
        let (_t, s, mut c) = fixture();
        c.validation.protocol_validation = true;
        c.validation.binding_sha256 = c.binding().unwrap();
        atomic_json(
            &s.root.join("config.json"),
            &serde_json::to_value(c).unwrap(),
        )
        .unwrap();
        let id = plan(&s);
        let mut transport = Mock {
            mode: "timeout",
            ..Default::default()
        };
        assert_eq!(
            commit_verified(&s, &id, true, &mut transport).unwrap()["state"],
            "unknown"
        );
        let before = transport.calls.len();
        assert_eq!(
            drain_loop(&s, &mut transport, false).unwrap()["state"],
            "needs_review"
        );
        assert_eq!(transport.calls.len(), before);
        assert_eq!(transport.posts, 1);
        assert_eq!(transport.refreshes, 0);
    }
    #[test]
    fn model_prefix_reads_exactly_four_bytes_without_reading_serial_tail() {
        struct Tracked {
            position: usize,
        }
        impl std::io::Read for Tracked {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                assert!(buf.len() <= 4);
                let source = b"B0D4SYNTHETIC-PRIVATE-TAIL";
                let n = buf.len().min(source.len() - self.position);
                buf[..n].copy_from_slice(&source[self.position..self.position + n]);
                self.position += n;
                Ok(n)
            }
        }
        let mut input = Tracked { position: 0 };
        assert_eq!(model_prefix(&mut input).unwrap(), "B0D4");
        assert_eq!(input.position, 4);
    }
    #[test]
    fn storage_profile_boundaries_include_journal_and_metadata_margin() {
        let required = required_free_bytes(ROUTINE_TRANSACTION_BYTES, 4096).unwrap();
        assert_eq!(required, 24 * 1024 * 1024 + 64 * 1024);
        assert!(check_available_bytes(required, ROUTINE_TRANSACTION_BYTES, 4096).is_ok());
        assert!(check_available_bytes(required - 1, ROUTINE_TRANSACTION_BYTES, 4096).is_err());
        assert!(required_free_bytes(u64::MAX, 4096).is_err());
        assert!(required_free_bytes(1, 0).is_err());
    }
    #[test]
    fn diagnostics_report_capture_bytes_without_reading_log_contents() {
        let (_t, s, _) = fixture();
        let active = s.root.join("events/active.log");
        let f = fs::File::create(&active).unwrap();
        f.set_len(12345).unwrap();
        let out = capture_diagnostics(&s.root);
        assert_eq!(out["available"], true);
        assert_eq!(out["active_bytes"], 12345);
        assert_eq!(out["sealed_files"], 1);
        assert_eq!(out["sealed_bytes"], 27);
        assert_eq!(out["reader_state"], "idle");
        assert!(!out.to_string().contains("synthetic counted evidence"));
    }
    #[test]
    fn status_lists_exact_unmatched_book_identity_for_configuration_only() {
        let (_t, s, _) = fixture();
        s.db.execute("UPDATE sessions SET state='unmatched'", [])
            .unwrap();
        let out = s.status().unwrap();
        assert_eq!(
            out["books"],
            json!([{"local_book_id":"A","title":"Synthetic Book"}])
        );
        assert!(!out["books"].to_string().contains("sources"));
        assert!(!out["books"].to_string().contains("synthetic-access"));
        assert_eq!(
            s.db.query_row("SELECT state FROM sessions", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "unmatched"
        );
    }
    #[test]
    fn diagnostics_fail_closed_for_missing_or_unsafe_runtime() {
        let (_t, s, _) = fixture();
        let p = s.root.join("runtime.json");
        fs::remove_file(&p).unwrap();
        assert_eq!(capture_diagnostics(&s.root)["available"], false);
        std::os::unix::fs::symlink(s.root.join("config.json"), &p).unwrap();
        assert_eq!(capture_diagnostics(&s.root)["available"], false);
        fs::remove_file(&p).unwrap();
        atomic_json(&p,&json!({"schema_version":1,"reader_state":"invented","power_state":"awake","usb_export":false,"network_connected":false,"idle_since_unix":0,"bootstrap_unix":1000})).unwrap();
        assert_eq!(capture_diagnostics(&s.root)["available"], false);
    }
    #[test]
    fn logical_compact_reuses_db_pages_without_vacuum() {
        let (_t, s, _) = fixture();
        let id = plan(&s);
        commit_verified(&s, &id, true, &mut Mock::default()).unwrap();
        let p = s.root.join("events/sealed/synthetic.log");
        s.db.execute(
            "INSERT INTO sources VALUES (?,?)",
            params![p.to_string_lossy(), sha(fs::read(&p).unwrap())],
        )
        .unwrap();
        s.db.execute("INSERT INTO meta VALUES ('last_collect_blocked','0')", [])
            .unwrap();
        atomic_json(
            &s.root.join("collection-state.json"),
            &json!({"complete":true,"blocked":0}),
        )
        .unwrap();
        let before = fs::metadata(s.root.join("ledger.sqlite")).unwrap().len();
        let out = compact(&s).unwrap();
        assert_eq!(out["vacuum_performed"], false);
        assert_eq!(out["database_file_shrunk"], false);
        assert!(fs::metadata(s.root.join("ledger.sqlite")).unwrap().len() >= before);
        assert_eq!(s.status().unwrap()["confirmed_seconds"], 13);
    }
    #[test]
    fn oversized_existing_ledger_is_never_truncated_or_reinitialized() {
        let t = TempDir::new().unwrap();
        fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = t.path().join("ledger.sqlite");
        let f = fs::File::create(&path).unwrap();
        f.set_len(LEDGER_LIMIT_BYTES + 4096).unwrap();
        drop(f);
        let before = fs::metadata(&path).unwrap().len();
        assert!(Store::open(t.path()).is_err());
        assert_eq!(fs::metadata(path).unwrap().len(), before);
    }
    #[test]
    fn incompatible_ledger_page_size_cannot_bypass_four_mib_cap() {
        let t = TempDir::new().unwrap();
        fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = t.path().join("ledger.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA page_size=8192; CREATE TABLE existing(data TEXT); INSERT INTO existing VALUES('preserve');")
            .unwrap();
        drop(db);
        assert!(Store::open(t.path()).is_err());
        let db = Connection::open(path).unwrap();
        assert_eq!(
            db.query_row("SELECT data FROM existing", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "preserve"
        );
        let created: i64 = db
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='sources'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(created, 0);
    }
    #[test]
    fn sqlite_full_rolls_back_without_erasing_pending_identity() {
        let (_t, s, _) = fixture();
        s.db.execute_batch("CREATE TABLE budget_payload(data BLOB)")
            .unwrap();
        let err =
            s.db.execute(
                "INSERT INTO budget_payload VALUES(zeroblob(?))",
                [LEDGER_LIMIT_BYTES as i64],
            )
            .unwrap_err();
        assert_eq!(err.sqlite_error_code(), Some(rusqlite::ErrorCode::DiskFull));
        assert_eq!(
            s.db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(s.status().unwrap()["pending_seconds"], 13);
        assert_eq!(
            s.db.query_row("PRAGMA cache_spill", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            s.db.query_row("PRAGMA temp_store", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}

/// Separate read-only reconciliation. One identity-checked token renewal is
/// allowed for these GETs; the original time POST is never sent again.
pub fn reconcile<T: network::Transport>(
    store: &Store,
    id: &str,
    transport: &mut T,
) -> Result<Value> {
    gate(&store.root, true, now())?;
    let c = Config::load(&store.root)?;
    store.bind(&c)?;
    let mut a = Auth::load(&store.root, &c)?;
    let (state, text): (String, String) =
        store
            .db
            .query_row("SELECT state,body FROM attempts WHERE id=?", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
    let mut receipt: Value = serde_json::from_str(&text)?;
    if state == "confirmed" {
        return Ok(receipt);
    }
    ensure!(
        state == "needs_review" && receipt["acknowledged"] == true,
        "only acknowledged pending results can be reconciled; never replay unknown writes"
    );
    let body: String = store
        .db
        .query_row("SELECT body FROM plans WHERE id=?", [id], |r| r.get(0))?;
    let plan: Value = serde_json::from_str(&body)?;
    ensure!(
        plan["binding_sha256"] == c.binding()? && plan["account_hash"] == c.account_hash(),
        "reconcile account/config mismatch"
    );
    let bid = plan["weread_book_id"].as_str().context("missing book")?;
    let date = plan["day"].as_str().context("missing date")?;
    let mut refreshed = false;
    let after = position(
        &network::preflight(
            transport,
            &store.root,
            &mut a,
            &mut refreshed,
            "/book/getProgress",
            &[("bookId", bid.into())],
        )?,
        bid,
    )?;
    let day_after = day_snapshot(
        &network::preflight(
            transport,
            &store.root,
            &mut a,
            &mut refreshed,
            "/readdata/detail",
            &[
                ("mode", "weekly".into()),
                (
                    "baseTime",
                    plan["hours"][0]["startOfHour"]
                        .as_i64()
                        .context("invalid epoch")?
                        .to_string(),
                ),
            ],
        )?,
        date,
    )?;
    let before: Position = serde_json::from_value(receipt["before"].clone())?;
    let day_before = receipt["day_before"].as_i64().context("missing baseline")?;
    let seconds = plan["seconds"].as_i64().context("missing seconds")?;
    receipt["recheck_after"] = serde_json::to_value(&after)?;
    receipt["recheck_day_after"] = json!(day_after);
    receipt["readonly_recheck_hint_saved"] = json!(false);
    if matched(&before, &after, day_before, day_after, seconds) {
        storage_budget(&store.root, ROUTINE_TRANSACTION_BYTES)?;
        let tx = store.db.unchecked_transaction()?;
        ensure!(
            tx.execute(
                "UPDATE attempts SET state='confirmed' WHERE id=? AND state='needs_review'",
                [id]
            )? == 1,
            "reconcile ownership changed"
        );
        // Preserve the original possibly-stale observations as evidence, while
        // making the canonical confirmed receipt describe the actual GETs that
        // established both counters and the unchanged reading position.
        if receipt.get("original_after").is_none() {
            receipt["original_after"] = receipt["after"].clone();
        }
        if receipt.get("original_day_after").is_none() {
            receipt["original_day_after"] = receipt["day_after"].clone();
        }
        receipt["after"] = serde_json::to_value(&after)?;
        receipt["day_after"] = json!(day_after);
        receipt["state"] = json!("confirmed");
        receipt["verification"] = json!("matched_on_readonly_recheck");
        tx.execute(
            "UPDATE attempts SET body=? WHERE id=?",
            params![receipt.to_string(), id],
        )?;
        tx.execute("UPDATE plans SET state='confirmed' WHERE id=?", [id])?;
        for s in plan["sessions"].as_array().context("missing sessions")? {
            tx.execute(
                "UPDATE sessions SET state='confirmed' WHERE id=? AND state='needs_review'",
                [s["id"].as_str().context("missing id")?],
            )?;
        }
        tx.commit()?;
    } else {
        receipt["readonly_recheck_hint_saved"] = json!(
            readback_may_be_delayed(&before, &after, day_before, day_after, seconds)
                && save_readonly_recheck_hint(&store.root).is_ok()
        );
    }
    Ok(receipt)
}

/// Explicit local filesystem probe only. It does not create an acceptance certificate.
pub fn self_test(root: &Path, online: bool) -> Value {
    let filesystem = (|| -> Result<()> {
        let store = Store::open(root)?;
        let path = store.root.join("self-test-probe.json");
        atomic_json(&path, &json!({"kind":"temporary-filesystem-probe"}))?;
        ensure!(
            private_json(&path)?["kind"] == "temporary-filesystem-probe",
            "filesystem readback mismatch"
        );
        fs::remove_file(path)?;
        Ok(())
    })();
    let mut out = json!({"schema_version":1,"actual_device":actual_device(),"private_filesystem_ok":filesystem.is_ok(),"network_contacted":online,"acceptance_created":false});
    if online {
        out["online_probe"] = match network::Live::default().anonymous_probe() {
            Ok(v) => v,
            Err(_) => json!({"state":"failed","certificate_validation_disabled":false}),
        };
        let saved = (|| -> Result<()> {
            let c = Config::load(root)?;
            experimental::record_live_tls(root, &c, &actual_device(), &out["online_probe"])
        })();
        out["tls_evidence_saved"] = json!(saved.is_ok());
        out["tls_evidence_required"] = json!(root.join("config.json").exists());
    }
    out
}

/// Only confirmed and completely attributable sealed journals can be compacted.
/// IDs, counters, outcomes and hash archives survive; active.log is never opened here.
pub fn compact(store: &Store) -> Result<Value> {
    ensure!(
        private_json(&store.root.join("collection-state.json"))?["complete"] == true,
        "compaction_blocked: collection incomplete"
    );
    gate(&store.root, false, now())?;
    storage_budget(&store.root, ROUTINE_TRANSACTION_BYTES)?;
    let blocked: Option<String> = store
        .db
        .query_row(
            "SELECT value FROM meta WHERE key='last_collect_blocked'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    ensure!(
        blocked.as_deref() == Some("0"),
        "compaction_blocked: last collection has unmatched/unverified data"
    );
    let unconfirmed: i64 = store.db.query_row(
        "SELECT count(*) FROM sessions WHERE state!='confirmed'",
        [],
        |r| r.get(0),
    )?;
    let count: i64 = store
        .db
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))?;
    let uncertain: i64 = store.db.query_row(
        "SELECT count(*) FROM attempts WHERE state!='confirmed'",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        count > 0 && unconfirmed == 0 && uncertain == 0,
        "compaction_blocked: unconfirmed or uncertain data must be retained"
    );
    let base = fs::canonicalize(store.root.join("events/sealed"))?;
    let mut stmt = store.db.prepare("SELECT path,hash FROM sources")?;
    let candidates: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    for entry in fs::read_dir(&base)? {
        let p = entry?.path();
        if p.extension().and_then(|s| s.to_str()) == Some("log") {
            ensure!(
                candidates
                    .iter()
                    .any(|(v, _)| fs::canonicalize(v).ok() == fs::canonicalize(&p).ok()),
                "compaction_blocked: unprocessed sealed journal"
            );
        }
    }
    if candidates.is_empty() {
        return Ok(json!({"state":"nothing_to_compact"}));
    }
    for (path, hash) in &candidates {
        check_cancel()?;
        let p = Path::new(path);
        ensure!(
            p.extension().and_then(|s| s.to_str()) == Some("log")
                && fs::canonicalize(p)?.starts_with(&base)
                && !fs::symlink_metadata(p)?.file_type().is_symlink(),
            "compaction source outside sealed journal"
        );
        ensure!(
            sha(limited(p, 16 * 1024 * 1024)?) == *hash,
            "compaction source changed"
        );
    }
    // Verify every referenced original; missing files are allowed only when THIS
    // ledger previously archived that exact path/hash during a successful compact.
    let mut sessions = store.db.prepare("SELECT body FROM sessions")?;
    for row in sessions.query_map([], |r| r.get::<_, String>(0))? {
        let v: Value = serde_json::from_str(&row?)?;
        for src in v["sources"].as_array().context("missing proof")? {
            let path = src["path"].as_str().context("missing path")?;
            let hash = src["sha256"].as_str().context("missing hash")?;
            if Path::new(path).exists() {
                ensure!(
                    sha(limited(Path::new(path), 16 * 1024 * 1024)?) == hash,
                    "confirmed proof changed"
                );
            } else {
                let prior: Option<String> = store
                    .db
                    .query_row(
                        "SELECT hash FROM compacted_sources WHERE path=?",
                        [path],
                        |r| r.get(0),
                    )
                    .optional()?;
                ensure!(prior.as_deref() == Some(hash), "unarchived proof missing");
            }
        }
    }
    drop(sessions);
    gate(&store.root, false, now())?;
    storage_budget(&store.root, ROUTINE_TRANSACTION_BYTES)?;
    let tx = store.db.unchecked_transaction()?;
    for (path, hash) in &candidates {
        tx.execute("INSERT INTO compacted_sources VALUES (?,?) ON CONFLICT(path) DO UPDATE SET hash=excluded.hash",params![path,hash])?;
    }
    tx.execute("DELETE FROM lines", [])?;
    tx.execute("DELETE FROM sources", [])?;
    tx.commit()?;
    // A crash here leaves duplicate source files, but retained session IDs still
    // prevent a second upload. Never delete the ledger to recover this condition.
    for (path, hash) in &candidates {
        let p = Path::new(path);
        if p.exists() && sha(limited(p, 16 * 1024 * 1024)?) == *hash {
            fs::remove_file(p)?;
        }
    }
    fs::File::open(&base)?.sync_all()?;
    // DELETE reuses freed SQLite pages. Do not run VACUUM in an automatic
    // pipeline: it can need two database copies beyond the routine scratch
    // budget and cannot race the collector under this PW2 profile.
    Ok(
        json!({"state":"compacted","sealed_files_removed":candidates.len(),"session_ids_preserved":true,"database_file_shrunk":false,"vacuum_performed":false}),
    )
}
