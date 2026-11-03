//! Append-only, explicitly selected file candidates. Base authorization stays bound.
use crate::*;
use std::collections::BTreeSet;

const REGISTRY: &str = "book-candidates.json";

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Target {
    #[serde(rename = "bookId")]
    book_id: String,
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    publisher: String,
    #[serde(default)]
    isbn: String,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Candidate {
    candidate_id: String,
    title: String,
    author: String,
    target: Target,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    schema_version: u32,
    binding_sha256: String,
    entries: Vec<Candidate>,
}

fn source_key(entry: &Candidate) -> Result<&str> {
    let path = Path::new(&entry.candidate_id);
    let parts: Vec<_> = path.components().collect();
    ensure!(
        parts.len() >= 2
            && parts.len() <= 12
            && parts
                .iter()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            && parts[0].as_os_str() == "documents",
        "invalid candidate path"
    );
    ensure!(
        entry.candidate_id.len() <= 4096
            && !entry.candidate_id.contains('\0')
            && [
                &entry.title,
                &entry.author,
                &entry.target.book_id,
                &entry.target.title,
                &entry.target.author,
                &entry.target.publisher,
                &entry.target.isbn
            ]
            .iter()
            .all(|s| s.len() <= 4096 && !s.contains('\0'))
            && !entry.target.book_id.is_empty()
            && !entry.target.title.is_empty(),
        "invalid candidate metadata"
    );
    ensure!(
        matches!(
            path.extension().and_then(|v| v.to_str()),
            Some("mobi" | "azw" | "azw3" | "pdf" | "kfx" | "epub")
        ),
        "unsupported candidate format"
    );
    path.file_stem()
        .and_then(|v| v.to_str())
        .context("missing candidate file stem")
}
fn validate(registry: &Registry, c: &Config) -> Result<()> {
    ensure!(
        registry.schema_version == 1
            && registry.binding_sha256 == c.binding()?
            && registry.entries.len() <= 200,
        "candidate configuration binding mismatch"
    );
    let mut keys = BTreeSet::new();
    for entry in &registry.entries {
        let key = source_key(entry)?;
        ensure!(keys.insert(key), "ambiguous duplicate candidate source");
        if let Some(existing) = c.mappings.iter().find(|m| m.local_book_id == key) {
            ensure!(
                existing.weread_book_id == entry.target.book_id,
                "candidate cannot replace an existing mapping"
            );
        }
    }
    Ok(())
}
fn load(store: &Store, c: &Config) -> Result<Registry> {
    let path = store.root.join(REGISTRY);
    if !path.try_exists()? {
        return Ok(Registry {
            schema_version: 1,
            binding_sha256: c.binding()?,
            entries: Vec::new(),
        });
    }
    let registry: Registry = serde_json::from_value(private_json(&path)?)?;
    validate(&registry, c)?;
    Ok(registry)
}

/// A registry is account/config bound, contains no credentials, and only appends.
pub fn import(store: &Store, inbox: &Path) -> Result<Value> {
    ensure!(
        !fs::canonicalize(inbox)?.starts_with(fs::canonicalize(&store.root)?),
        "candidate inbox must be outside private root"
    );
    ensure!(
        fs::symlink_metadata(inbox)?.is_dir()
            && !fs::symlink_metadata(inbox)?.file_type().is_symlink(),
        "unsafe candidate inbox"
    );
    let c = Config::load(&store.root)?;
    store.bind(&c)?;
    let path = inbox.join("request.json");
    let raw = limited(&path, 256 * 1024)?;
    let incoming: Registry = serde_json::from_slice(&raw)?;
    validate(&incoming, &c)?;
    let mut registry = load(store, &c)?;
    for entry in incoming.entries {
        if let Some(previous) = registry
            .entries
            .iter()
            .find(|p| p.candidate_id == entry.candidate_id)
        {
            ensure!(
                previous.target == entry.target,
                "candidate target is already fixed"
            );
        } else {
            registry.entries.push(entry);
        }
    }
    validate(&registry, &c)?;
    atomic_json(
        &store.root.join(REGISTRY),
        &serde_json::to_value(&registry)?,
    )?;
    ensure!(
        limited(&path, 256 * 1024)? == raw,
        "candidate inbox changed"
    );
    fs::remove_file(path)?;
    Ok(
        json!({"state":"candidates_imported","candidate_count":registry.entries.len(),
        "authorization_changed":false,"reading_time_posted":false}),
    )
}

/// Resolve only after exact native counter identity, never from similar titles.
/// The sidecar index is already built by normal collection; it avoids scanning
/// the whole library here. Duplicate counter filenames remain unresolved.
pub fn resolved(store: &Store, c: &Config) -> Result<Vec<Mapping>> {
    resolved_at(store, c, Path::new("/mnt/us"))
}
fn resolved_at(store: &Store, c: &Config, userstore: &Path) -> Result<Vec<Mapping>> {
    let registry = load(store, c)?;
    let mut mappings = c.mappings.clone();
    if registry.entries.is_empty() {
        return Ok(mappings);
    }
    let index = private_json(&store.root.join("sidecar-index.json")).unwrap_or(json!({}));
    let cards = index["cards"].as_array();
    for entry in registry.entries {
        let key = source_key(&entry)?;
        if mappings.iter().any(|m| m.local_book_id == key) {
            continue;
        }
        let native: bool = store.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE book=? AND state!='blocked_overlap')",
            [key],
            |r| r.get(0),
        )?;
        if !native || cards.map_or(0, |rows| rows.iter().filter(|r| r["file"] == key).count()) != 1
        {
            continue;
        }
        let mut path = userstore.to_path_buf();
        let mut safe = true;
        for component in Path::new(&entry.candidate_id).components() {
            path.push(component);
            if fs::symlink_metadata(&path).map_or(true, |m| m.file_type().is_symlink()) {
                safe = false;
                break;
            }
        }
        let sidecar = path.with_extension("sdr");
        if !safe || !path.is_file() || !sidecar.is_dir() || sidecar.is_symlink() {
            continue;
        }
        mappings.push(Mapping {
            local_book_id: key.into(),
            weread_book_id: entry.target.book_id,
            title: entry.target.title,
        });
    }
    Ok(mappings)
}

pub fn summary(store: &Store, c: &Config) -> Result<Value> {
    let registry = load(store, c)?;
    Ok(json!({"saved_candidates":registry.entries.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn fixture() -> (TempDir, Store, Config) {
        let t = TempDir::new().unwrap();
        fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(t.path()).unwrap();
        let c: Config = serde_json::from_value(json!({"schema_version":1,"device_id":"synthetic-device",
            "account_vid":"synthetic-account","profile":"pw2-5.12.2.1.1","firmware":"5.12.2.1.1",
            "timezone":"Asia/Shanghai","bootstrap_cutoff_unix":1000,"mappings":[],
            "validation":{"device_validation":false,"protocol_validation":false,"binding_sha256":""}})).unwrap();
        atomic_json(
            &store.root.join("config.json"),
            &serde_json::to_value(&c).unwrap(),
        )
        .unwrap();
        (t, store, c)
    }
    fn request(c: &Config) -> Value {
        json!({"schema_version":1,"binding_sha256":c.binding().unwrap(),"entries":[{
            "candidate_id":"documents/Author/Book.mobi","title":"Local book","author":"Author",
            "target":{"bookId":"new-target","title":"Chosen version"}}]})
    }
    #[test]
    fn import_is_idempotent_bound_and_cannot_retarget() {
        let (_t, store, c) = fixture();
        let inbox = TempDir::new().unwrap();
        let original = fs::read(store.root.join("config.json")).unwrap();
        for _ in 0..2 {
            fs::write(inbox.path().join("request.json"), request(&c).to_string()).unwrap();
            assert_eq!(import(&store, inbox.path()).unwrap()["candidate_count"], 1);
        }
        assert_eq!(fs::read(store.root.join("config.json")).unwrap(), original);
        let mut bad = request(&c);
        bad["entries"][0]["target"]["bookId"] = json!("different-target");
        fs::write(inbox.path().join("request.json"), bad.to_string()).unwrap();
        assert!(import(&store, inbox.path()).is_err());
        bad["binding_sha256"] = json!("different-account-config");
        fs::write(inbox.path().join("request.json"), bad.to_string()).unwrap();
        assert!(import(&store, inbox.path()).is_err());
    }
    #[test]
    fn candidate_path_and_native_evidence_are_required() {
        let (_t, store, c) = fixture();
        let inbox = TempDir::new().unwrap();
        fs::write(inbox.path().join("request.json"), request(&c).to_string()).unwrap();
        import(&store, inbox.path()).unwrap();
        let usb = TempDir::new().unwrap();
        let docs = usb.path().join("documents/Author");
        fs::create_dir_all(docs.join("Book.sdr")).unwrap();
        fs::write(docs.join("Book.mobi"), b"synthetic").unwrap();
        assert!(resolved_at(&store, &c, usb.path()).unwrap().is_empty());
        store
            .db
            .execute(
                "INSERT INTO sessions VALUES('s','Book','2026-11-03',0,10,1,'pending','{}')",
                [],
            )
            .unwrap();
        atomic_json(
            &store.root.join("sidecar-index.json"),
            &json!({"cards":[{"file":"Book"}]}),
        )
        .unwrap();
        assert_eq!(
            resolved_at(&store, &c, usb.path()).unwrap()[0].weread_book_id,
            "new-target"
        );
        atomic_json(
            &store.root.join("sidecar-index.json"),
            &json!({"cards":[{"file":"Book"},{"file":"Book"}]}),
        )
        .unwrap();
        assert!(resolved_at(&store, &c, usb.path()).unwrap().is_empty());
        let mut bad = request(&c);
        bad["entries"][0]["candidate_id"] = json!("documents/../secret.mobi");
        assert!(validate(&serde_json::from_value(bad).unwrap(), &c).is_err());
    }
}
