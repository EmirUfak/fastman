use crate::model::{ApiRequest, Folder, MAX_FOLDER_DEPTH, Project, SCHEMA_VERSION};
use crate::security::{is_valid_http_method, sha256};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime},
};
use uuid::Uuid;

pub const MAX_PROJECT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_WORKSPACE_BYTES: usize = 64 * 1024;
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_millis(500);
const STALE_LOCK_AGE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
}

/// Local folder structure: `<root>/<uuid>/project.json`. Folder/request hierarchy
/// is represented in the JSON tree, not as executable filesystem paths.
#[derive(Debug, Clone)]
pub struct ProjectStore {
    root: PathBuf,
    known_hashes: Arc<Mutex<HashMap<String, [u8; 32]>>>,
}

impl ProjectStore {
    /// The original project directory remains the default workspace for compatibility.
    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        let (workspaces, errors) = self.workspaces_resilient()?;
        if let Some(first) = errors.into_iter().next() {
            bail!("{first}");
        }
        Ok(workspaces)
    }

    /// Loads valid workspaces while isolating any corrupt metadata files as warnings.
    pub fn workspaces_resilient(&self) -> Result<(Vec<Workspace>, Vec<String>)> {
        let mut workspaces = vec![Workspace {
            id: "default".into(),
            name: "My workspace".into(),
        }];
        let mut warnings = Vec::new();
        let path = self.root.join("workspaces");
        if !path.exists() {
            return Ok((workspaces, warnings));
        }
        for entry in fs::read_dir(&path).with_context(|| format!("Read {}", path.display()))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if Uuid::parse_str(&id).is_err() {
                continue;
            }
            let metadata = entry.path().join("workspace.json");
            if !metadata.exists() {
                continue;
            }
            let parsed = (|| -> Result<Workspace> {
                let bytes = read_bounded(&metadata, MAX_WORKSPACE_BYTES)?;
                let workspace: Workspace = serde_json::from_slice(&bytes)
                    .with_context(|| format!("Decode {}", metadata.display()))?;
                if workspace.id != id || workspace.name.trim().is_empty() {
                    bail!("Invalid workspace metadata in {}", metadata.display());
                }
                Ok(workspace)
            })();
            match parsed {
                Ok(ws) => workspaces.push(ws),
                Err(err) => warnings.push(format!("Skipped workspace {id}: {err:#}")),
            }
        }
        workspaces[1..].sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok((workspaces, warnings))
    }

    pub fn create_workspace(&self, name: &str) -> Result<Workspace> {
        if name.trim().is_empty() {
            bail!("Workspace name is empty");
        }
        let workspace = Workspace {
            id: Uuid::new_v4().to_string(),
            name: name.trim().into(),
        };
        let workspaces_dir = self.root.join("workspaces");
        ensure_private_dir(&self.root)?;
        ensure_private_dir(&workspaces_dir)?;
        let dir = workspaces_dir.join(&workspace.id);
        ensure_private_dir(&dir)?;

        let encoded = serde_json::to_vec_pretty(&workspace)?;
        let dest = dir.join("workspace.json");
        let temp_path = dir.join(format!(".{}.tmp", Uuid::new_v4()));
        let mut temp = TempFileGuard::new(temp_path);
        write_private_new_file(temp.path(), &encoded).context("Write workspace metadata")?;
        fs::rename(temp.path(), &dest).context("Commit workspace metadata")?;
        temp.disarm();
        enforce_private_file_perms(&dest);
        Ok(workspace)
    }

    pub fn for_workspace(&self, id: &str) -> Result<Self> {
        if id == "default" {
            return Ok(self.clone());
        }
        let id = Uuid::parse_str(id).context("Invalid workspace ID")?;
        let dir = self.root.join("workspaces").join(id.to_string());
        if !dir.join("workspace.json").is_file() {
            bail!("Workspace not found");
        }
        Ok(Self::at(dir.join("projects")))
    }

    pub fn system_default() -> Result<Self> {
        let root = std::env::var_os("FASTMAN_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::data_local_dir().map(|p| p.join("fastman")))
            .context("No application data directory; set FASTMAN_HOME")?;
        Ok(Self::at(root))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            known_hashes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn create(&self, name: impl Into<String>) -> Result<Project> {
        let project = Project::new(name);
        self.save(&project)?;
        Ok(project)
    }

    pub fn list(&self) -> Result<Vec<Project>> {
        let (projects, errors) = self.list_resilient()?;
        if let Some(first) = errors.into_iter().next() {
            bail!("{first}");
        }
        Ok(projects)
    }

    /// Loads valid projects (recovering any interrupted atomic saves) while isolating
    /// corrupt project files as warnings instead of failing the entire workspace.
    pub fn list_resilient(&self) -> Result<(Vec<Project>, Vec<String>)> {
        if !self.root.exists() {
            return Ok((vec![], vec![]));
        }
        let mut projects = Vec::new();
        let mut warnings = Vec::new();
        for entry in
            fs::read_dir(&self.root).with_context(|| format!("Read {}", self.root.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name();
            let id_str = id.to_string_lossy();
            // The folder name must be a UUID; ignore unrelated directories.
            if Uuid::parse_str(&id_str).is_err() {
                continue;
            }
            let dir = entry.path();
            if let Err(err) = recover_project_files(&dir, &id_str) {
                warnings.push(format!("Recovery notice for {id_str}: {err:#}"));
            }
            let path = dir.join("project.json");
            if !path.exists() {
                continue;
            }
            match self.load(&id_str) {
                Ok(project) => projects.push(project),
                Err(err) => warnings.push(format!("Failed to load project {id_str}: {err:#}")),
            }
        }
        projects.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok((projects, warnings))
    }

    pub fn load(&self, id: &str) -> Result<Project> {
        let id = Uuid::parse_str(id).context("Invalid project ID")?;
        let id_str = id.to_string();
        let dir = self.root.join(&id_str);
        recover_project_files(&dir, &id_str)?;
        let path = dir.join("project.json");
        let bytes = read_bounded(&path, MAX_PROJECT_BYTES)?;
        let project: Project = serde_json::from_slice(&bytes).context("Decode project JSON")?;
        validate_project(&project)?;
        if project.id != id_str {
            bail!("Project ID disagrees with directory");
        }
        let digest = sha256(&bytes);
        if let Ok(mut map) = self.known_hashes.lock() {
            map.insert(id_str, digest);
        }
        Ok(project)
    }

    pub fn save(&self, project: &Project) -> Result<()> {
        validate_project(project)?;
        let id = Uuid::parse_str(&project.id).context("Invalid project ID")?;
        let id_str = id.to_string();
        ensure_private_dir(&self.root)?;
        let dir = self.root.join(&id_str);
        ensure_private_dir(&dir)?;

        let _lock = DirLockGuard::acquire(&dir)?;
        recover_project_files(&dir, &id_str)?;

        let dest = dir.join("project.json");
        if dest.exists() {
            let disk_bytes = read_bounded(&dest, MAX_PROJECT_BYTES)?;
            let disk_digest = sha256(&disk_bytes);
            let known = self
                .known_hashes
                .lock()
                .ok()
                .and_then(|map| map.get(&id_str).copied());
            match known {
                Some(expected) if expected != disk_digest => {
                    bail!(
                        "Project '{}' was modified on disk by another instance; reload before saving",
                        project.name
                    );
                }
                None => {
                    bail!(
                        "Project file for '{}' already exists on disk and was not loaded by this session",
                        project.name
                    );
                }
                Some(_) => {}
            }
        }

        let encoded = serde_json::to_vec_pretty(project)?;
        if encoded.len() > MAX_PROJECT_BYTES {
            bail!("Serialized project exceeds maximum size ({MAX_PROJECT_BYTES} bytes)");
        }
        let new_digest = sha256(&encoded);

        let temp_path = dir.join(format!(".{}.tmp", Uuid::new_v4()));
        let mut temp = TempFileGuard::new(temp_path);
        write_private_new_file(temp.path(), &encoded).context("Write temporary project file")?;

        if let Err(error) = fs::rename(temp.path(), &dest) {
            if dest.exists() {
                let backup = dir.join("project.json.previous");
                if backup.exists() {
                    let _ = fs::remove_file(&backup);
                }
                fs::rename(&dest, &backup).context("Back up previous project")?;
                if let Err(replace_error) = fs::rename(temp.path(), &dest) {
                    let _ = fs::rename(&backup, &dest);
                    return Err(replace_error).context("Replace project file");
                }
                let _ = fs::remove_file(&backup);
            } else {
                return Err(error).context("Commit project file");
            }
        }
        temp.disarm();
        enforce_private_file_perms(&dest);
        sync_directory(&dir);

        if let Ok(mut map) = self.known_hashes.lock() {
            map.insert(id_str, new_digest);
        }
        Ok(())
    }
}

pub(crate) fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("Read {}", path.display()))?;
    if let Ok(meta) = file.metadata()
        && meta.len() > max_bytes as u64
    {
        bail!(
            "File {} exceeds maximum allowed size ({max_bytes} bytes)",
            path.display()
        );
    }
    let mut bytes = Vec::new();
    file.take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("Read {}", path.display()))?;
    if bytes.len() > max_bytes {
        bail!(
            "File {} exceeds maximum allowed size ({max_bytes} bytes)",
            path.display()
        );
    }
    Ok(bytes)
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .with_context(|| format!("Create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn write_private_new_file(path: &Path, data: &[u8]) -> Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .with_context(|| format!("Create {}", path.display()))?;
    file.write_all(data)
        .with_context(|| format!("Write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("Sync {}", path.display()))?;
    Ok(())
}

fn enforce_private_file_perms(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn sync_directory(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

struct TempFileGuard {
    path: PathBuf,
    armed: bool,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }
    fn path(&self) -> &Path {
        &self.path
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct DirLockGuard {
    path: PathBuf,
}

impl DirLockGuard {
    fn acquire(dir: &Path) -> Result<Self> {
        let lock_path = dir.join(".save.lock");
        let deadline = Instant::now() + LOCK_WAIT_TIMEOUT;
        loop {
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            match opts.open(&lock_path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "pid={}", std::process::id());
                    return Ok(Self { path: lock_path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Ok(meta) = fs::metadata(&lock_path)
                        && let Ok(modified) = meta.modified()
                        && SystemTime::now()
                            .duration_since(modified)
                            .is_ok_and(|age| age > STALE_LOCK_AGE)
                    {
                        let _ = fs::remove_file(&lock_path);
                        continue;
                    }
                    if Instant::now() >= deadline {
                        bail!(
                            "Timed out waiting for project save lock at {}",
                            dir.display()
                        );
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => {
                    return Err(err).context("Acquire project save lock");
                }
            }
        }
    }
}

impl Drop for DirLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Clean up orphaned temp files and recover interrupted `project.json.previous` backups.
fn recover_project_files(dir: &Path, expected_id: &str) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if s.starts_with('.') && s.ends_with(".tmp") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    let dest = dir.join("project.json");
    let backup = dir.join("project.json.previous");
    if !backup.exists() {
        return Ok(());
    }

    let backup_valid = is_valid_project_file(&backup, expected_id);
    if !dest.exists() {
        if backup_valid {
            fs::rename(&backup, &dest).context("Recover interrupted project backup")?;
            enforce_private_file_perms(&dest);
        }
        return Ok(());
    }

    if is_valid_project_file(&dest, expected_id) {
        let _ = fs::remove_file(&backup);
    } else if backup_valid {
        let corrupt = dir.join("project.json.corrupt");
        let _ = fs::remove_file(&corrupt);
        let _ = fs::rename(&dest, &corrupt);
        fs::rename(&backup, &dest).context("Restore backup over corrupt project.json")?;
        enforce_private_file_perms(&dest);
    }
    Ok(())
}

fn is_valid_project_file(path: &Path, expected_id: &str) -> bool {
    let Ok(bytes) = read_bounded(path, MAX_PROJECT_BYTES) else {
        return false;
    };
    let Ok(project) = serde_json::from_slice::<Project>(&bytes) else {
        return false;
    };
    project.id == expected_id && validate_project(&project).is_ok()
}

fn validate_project(project: &Project) -> Result<()> {
    if project.schema_version != SCHEMA_VERSION {
        bail!(
            "Unsupported project schema version {}",
            project.schema_version
        );
    }
    Uuid::parse_str(&project.id).context("Invalid project ID")?;
    if project.name.trim().is_empty() {
        bail!("Project name is empty");
    }

    let mut seen_ids = HashSet::new();
    for coll in &project.collections {
        register_id(&mut seen_ids, &coll.id, "collection")?;
        validate_requests(&coll.requests, &mut seen_ids)?;
        for folder in &coll.folders {
            validate_folder(folder, 1, &mut seen_ids)?;
        }
    }
    Ok(())
}

fn validate_folder<'a>(
    folder: &'a Folder,
    depth: usize,
    seen_ids: &mut HashSet<&'a str>,
) -> Result<()> {
    if depth > MAX_FOLDER_DEPTH {
        bail!("Project folder nesting exceeds maximum depth ({MAX_FOLDER_DEPTH})");
    }
    register_id(seen_ids, &folder.id, "folder")?;
    validate_requests(&folder.requests, seen_ids)?;
    for sub in &folder.folders {
        validate_folder(sub, depth + 1, seen_ids)?;
    }
    Ok(())
}

fn validate_requests<'a>(
    requests: &'a [ApiRequest],
    seen_ids: &mut HashSet<&'a str>,
) -> Result<()> {
    for req in requests {
        register_id(seen_ids, &req.id, "request")?;
        if !is_valid_http_method(&req.method) {
            bail!(
                "Invalid HTTP method '{}' in request '{}'",
                req.method,
                req.name
            );
        }
    }
    Ok(())
}

fn register_id<'a>(seen: &mut HashSet<&'a str>, id: &'a str, kind: &str) -> Result<()> {
    if id.trim().is_empty() {
        bail!("Empty {kind} ID");
    }
    if !seen.insert(id) {
        bail!("Duplicate {kind} ID '{id}' in project");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_path_validation() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(dir.path());
        let project = store.create("API").unwrap();
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, project.id);
        assert!(store.load("../../etc/passwd").is_err());
        let mut updated = project.clone();
        updated.name = "API renamed".into();
        store.save(&updated).unwrap();
        assert_eq!(store.load(&project.id).unwrap().name, "API renamed");
    }

    #[test]
    fn workspaces_isolate_projects_and_preserve_legacy_store() {
        let dir = tempfile::tempdir().unwrap();
        let root = ProjectStore::at(dir.path());
        let legacy = root.create("Existing").unwrap();
        let workspace = root.create_workspace("Team").unwrap();
        let other = root.for_workspace(&workspace.id).unwrap();
        assert!(other.list().unwrap().is_empty());
        let project = other.create("Other").unwrap();
        assert_eq!(root.list().unwrap()[0].id, legacy.id);
        assert_eq!(other.list().unwrap()[0].id, project.id);
        assert_eq!(root.workspaces().unwrap().len(), 2);
        assert_eq!(
            root.for_workspace("default")
                .unwrap()
                .load(&legacy.id)
                .unwrap()
                .name,
            "Existing"
        );
        assert!(root.for_workspace("../bad").is_err());
        assert!(root.create_workspace(" ").is_err());
    }

    #[test]
    fn contending_save_leaves_project_unchanged_and_can_retry() {
        let root = tempfile::tempdir().unwrap();
        let first = ProjectStore::at(root.path());
        let second = ProjectStore::at(root.path());
        let mut project = first.create("Original").unwrap();
        second.load(&project.id).unwrap();
        let project_dir = root.path().join(&project.id);
        let lock = DirLockGuard::acquire(&project_dir).unwrap();

        project.name = "Updated".into();
        let error = second.save(&project).unwrap_err();
        assert!(error.to_string().contains("waiting for project save lock"));
        assert!(project_dir.join(".save.lock").exists());
        assert_eq!(first.load(&project.id).unwrap().name, "Original");

        drop(lock);
        second.save(&project).unwrap();
        assert_eq!(first.load(&project.id).unwrap().name, "Updated");
    }

    #[cfg(windows)]
    #[test]
    fn failed_replacement_preserves_previous_project_and_cleans_up_temp_file() {
        use std::os::windows::fs::OpenOptionsExt;

        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        let mut project = store.create("Original").unwrap();
        let project_dir = root.path().join(&project.id);
        // Permit reads and writes but deny renaming the open destination.
        let open_project = OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2) // FILE_SHARE_READ | FILE_SHARE_WRITE
            .open(project_dir.join("project.json"))
            .unwrap();

        project.name = "Updated".into();
        let error = store.save(&project).unwrap_err();
        assert!(error.to_string().contains("Back up previous project"));
        assert_eq!(store.load(&project.id).unwrap().name, "Original");
        let files: Vec<_> = fs::read_dir(&project_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, ["project.json"]);

        drop(open_project);
        store.save(&project).unwrap();
        assert_eq!(store.load(&project.id).unwrap().name, "Updated");
    }

    #[test]
    fn detects_concurrent_save_conflicts_and_recovers_interrupted_backup() {
        let dir = tempfile::tempdir().unwrap();
        let store_a = ProjectStore::at(dir.path());
        let store_b = ProjectStore::at(dir.path());

        let mut p_a = store_a.create("Shared").unwrap();
        let mut p_b = store_b.load(&p_a.id).unwrap();

        p_a.name = "Update A".into();
        store_a.save(&p_a).unwrap();

        p_b.name = "Update B".into();
        assert!(
            store_b.save(&p_b).is_err(),
            "stale store_b save must detect external modification"
        );
        assert_eq!(store_a.load(&p_a.id).unwrap().name, "Update A");

        // Simulate interrupted save: project.json missing, .previous + orphan .tmp exist.
        let p_dir = dir.path().join(&p_a.id);
        let dest = p_dir.join("project.json");
        let backup = p_dir.join("project.json.previous");
        let orphan_tmp = p_dir.join(".orphan.tmp");
        fs::rename(&dest, &backup).unwrap();
        fs::write(&orphan_tmp, b"partial").unwrap();

        let recovered = store_a.list().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].name, "Update A");
        assert!(dest.exists());
        assert!(!backup.exists());
        assert!(!orphan_tmp.exists());
    }

    #[test]
    fn rejects_duplicate_child_ids_and_isolates_corrupt_projects_in_resilient_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(dir.path());
        let mut bad = Project::new("Dup");
        let req = ApiRequest::new("GET", "One", "https://example.com");
        let dup = req.clone();
        bad.collections[0].requests.push(req);
        bad.collections[0].requests.push(dup);
        assert!(store.save(&bad).is_err());

        let good = store.create("Healthy").unwrap();
        let corrupt_id = Uuid::new_v4().to_string();
        let corrupt_dir = dir.path().join(&corrupt_id);
        fs::create_dir_all(&corrupt_dir).unwrap();
        fs::write(corrupt_dir.join("project.json"), b"{not valid json").unwrap();

        let (projects, warnings) = store.list_resilient().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, good.id);
        assert_eq!(warnings.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn creates_private_permissions_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(dir.path());
        let project = store.create("Private").unwrap();
        let p_dir = dir.path().join(&project.id);
        let p_file = p_dir.join("project.json");
        assert_eq!(
            fs::metadata(&p_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&p_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
