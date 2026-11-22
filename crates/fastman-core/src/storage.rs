use crate::model::{ApiRequest, Folder, MAX_FOLDER_DEPTH, Project, SCHEMA_VERSION, SecretRef};
use crate::secrets::{validate_env_var_name, validate_secret_key};
use crate::security::{is_valid_http_method, sha256};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const STALE_LOCK_AGE: Duration = Duration::from_secs(30);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(5);
const MAX_LOCK_FILE_BYTES: usize = 256;

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
            match self.try_load_project_dir(&dir, &id_str) {
                Ok(Some(project)) => projects.push(project),
                Ok(None) => {}
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
        let path = dir.join("project.json");
        if !dir.is_dir() {
            bail!("Read {}: project directory not found", path.display());
        }
        let lock = DirLockGuard::acquire(&dir)?;
        recover_project_files_locked(&lock, &id_str)?;
        self.read_and_track_project(&path, &id_str)
    }

    fn try_load_project_dir(&self, dir: &Path, id_str: &str) -> Result<Option<Project>> {
        let lock = DirLockGuard::acquire(dir)?;
        recover_project_files_locked(&lock, id_str)?;
        let path = dir.join("project.json");
        if !path.exists() {
            return Ok(None);
        }
        self.read_and_track_project(&path, id_str).map(Some)
    }

    fn read_and_track_project(&self, path: &Path, id_str: &str) -> Result<Project> {
        let bytes = read_bounded(path, MAX_PROJECT_BYTES)?;
        let project: Project = serde_json::from_slice(&bytes).context("Decode project JSON")?;
        validate_project(&project)?;
        if project.id != id_str {
            bail!("Project ID disagrees with directory");
        }
        let digest = sha256(&bytes);
        if let Ok(mut map) = self.known_hashes.lock() {
            map.insert(id_str.to_owned(), digest);
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

        let lock = DirLockGuard::acquire(&dir)?;
        recover_project_files_locked(&lock, &id_str)?;

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
        drop(lock);
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
    dir: PathBuf,
    path: PathBuf,
    token: String,
}

impl DirLockGuard {
    fn dir(&self) -> &Path {
        &self.dir
    }

    fn acquire(dir: &Path) -> Result<Self> {
        Self::acquire_with_options(dir, LOCK_WAIT_TIMEOUT, STALE_LOCK_AGE)
    }

    fn acquire_with_options(dir: &Path, timeout: Duration, stale_age: Duration) -> Result<Self> {
        let lock_path = dir.join(".save.lock");
        let deadline = Instant::now() + timeout;
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
                    let token = Uuid::new_v4().to_string();
                    let payload = format!("pid={}\ntoken={token}\n", std::process::id());
                    let _ = file
                        .write_all(payload.as_bytes())
                        .and_then(|()| file.flush());
                    return Ok(Self {
                        dir: dir.to_path_buf(),
                        path: lock_path,
                        token,
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Some((expected_modified, expected_token)) =
                        stale_lock_snapshot(&lock_path, stale_age)
                    {
                        // Re-verify lock identity right before removal to narrow TOCTOU with a concurrent reaper.
                        let still_same = fs::metadata(&lock_path)
                            .and_then(|m| m.modified())
                            .is_ok_and(|m| m == expected_modified)
                            && read_lock_info(&lock_path).and_then(|(_, t)| t) == expected_token;
                        if still_same {
                            let _ = fs::remove_file(&lock_path);
                            continue;
                        }
                    }
                    if Instant::now() >= deadline {
                        bail!(
                            "Timed out waiting for project save lock at {}",
                            dir.display()
                        );
                    }
                    thread::sleep(LOCK_RETRY_INTERVAL);
                }
                // On Windows, opening a path marked delete-pending (unlinked while another thread
                // holds a metadata/read handle) raises PermissionDenied briefly until closed.
                Err(err) if cfg!(windows) && err.kind() == std::io::ErrorKind::PermissionDenied => {
                    if Instant::now() >= deadline {
                        bail!(
                            "Timed out waiting for project save lock at {}",
                            dir.display()
                        );
                    }
                    thread::sleep(LOCK_RETRY_INTERVAL);
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
        if let Some((_, Some(token))) = read_lock_info(&self.path)
            && token == self.token
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn read_lock_info(path: &Path) -> Option<(Option<u32>, Option<String>)> {
    let bytes = read_bounded(path, MAX_LOCK_FILE_BYTES).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let mut pid = None;
    let mut token = None;
    for line in text.lines() {
        if let Some(raw_pid) = line.strip_prefix("pid=") {
            pid = raw_pid.trim().parse::<u32>().ok();
        } else if let Some(raw_token) = line.strip_prefix("token=") {
            let trimmed = raw_token.trim();
            if !trimmed.is_empty() {
                token = Some(trimmed.to_owned());
            }
        }
    }
    Some((pid, token))
}

fn stale_lock_snapshot(path: &Path, stale_age: Duration) -> Option<(SystemTime, Option<String>)> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    if age > stale_age {
        let (_, token) = read_lock_info(path).unwrap_or((None, None));
        return Some((modified, token));
    }
    // Windows and macOS rely solely on `stale_age` to avoid unsafe FFI or subprocess spawning.
    #[cfg(target_os = "linux")]
    if age >= Duration::from_millis(200)
        && let Some((Some(pid), token)) = read_lock_info(path)
        && pid != std::process::id()
        && Path::new("/proc").is_dir()
        && !Path::new(&format!("/proc/{pid}")).exists()
    {
        return Some((modified, token));
    }
    None
}

/// Clean up orphaned temp files and recover interrupted `project.json.previous` backups
/// while holding the directory save lock.
fn recover_project_files_locked(lock: &DirLockGuard, expected_id: &str) -> Result<()> {
    let dir = lock.dir();
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
            sync_directory(dir);
        }
        return Ok(());
    }

    if is_valid_project_file(&dest, expected_id) {
        let _ = fs::remove_file(&backup);
        sync_directory(dir);
    } else if backup_valid {
        let corrupt = dir.join("project.json.corrupt");
        let _ = fs::remove_file(&corrupt);
        fs::rename(&dest, &corrupt).context("Quarantine corrupt project.json")?;
        fs::rename(&backup, &dest).context("Restore backup over corrupt project.json")?;
        enforce_private_file_perms(&dest);
        sync_directory(dir);
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

    validate_variable_scope("project", &project.variables, &project.secret_variables)?;

    let mut seen_env_ids = HashSet::new();
    for env in &project.environments {
        register_id(&mut seen_env_ids, &env.id, "environment")?;
        if env.name.trim().is_empty() {
            bail!("Environment name is empty");
        }
        validate_variable_scope(
            &format!("environment '{}'", env.name),
            &env.variables,
            &env.secret_variables,
        )?;
    }
    if let Some(active_id) = &project.active_environment
        && !seen_env_ids.contains(active_id.as_str())
    {
        bail!("Active environment '{active_id}' does not exist in project");
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

fn validate_variable_scope(
    scope: &str,
    plain: &BTreeMap<String, String>,
    secrets: &BTreeMap<String, SecretRef>,
) -> Result<()> {
    for (name, secret_ref) in secrets {
        if name.trim().is_empty() {
            bail!("Empty secret variable name in {scope}");
        }
        if plain.contains_key(name) {
            bail!("Variable '{name}' in {scope} cannot be both plain and secret");
        }
        match secret_ref {
            SecretRef::Keychain { key } => {
                validate_secret_key(key).map_err(|err| {
                    anyhow::anyhow!("Invalid keychain reference for '{name}' in {scope}: {err}")
                })?;
            }
            SecretRef::Env { var } => {
                if !validate_env_var_name(var) {
                    bail!("Invalid environment variable reference '{var}' for '{name}' in {scope}");
                }
            }
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
        let load_err = first.load(&project.id).unwrap_err();
        assert!(
            load_err
                .to_string()
                .contains("waiting for project save lock")
        );

        drop(lock);
        assert_eq!(first.load(&project.id).unwrap().name, "Original");
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

    #[test]
    fn locked_writer_temp_and_backup_survive_concurrent_load_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(dir.path());
        let project = store.create("Locked").unwrap();
        let project_dir = dir.path().join(&project.id);
        let project_bytes = fs::read(project_dir.join("project.json")).unwrap();

        let lock = DirLockGuard::acquire(&project_dir).unwrap();
        let active_tmp = project_dir.join(".active-save.tmp");
        let backup = project_dir.join("project.json.previous");
        fs::write(&active_tmp, b"in-flight").unwrap();
        fs::write(&backup, &project_bytes).unwrap();

        let load_err = store.load(&project.id).unwrap_err();
        assert!(
            load_err
                .to_string()
                .contains("waiting for project save lock"),
            "{load_err:#}"
        );

        let (listed, warnings) = store.list_resilient().unwrap();
        assert!(listed.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(active_tmp.exists());
        assert!(backup.exists());

        drop(lock);

        let (listed, warnings) = store.list_resilient().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Locked");
        assert!(warnings.is_empty());
        assert!(!active_tmp.exists());
        assert!(!backup.exists());
        assert!(project_dir.join("project.json").exists());
    }

    #[test]
    fn concurrent_save_load_and_list_resilient_stress() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let writer_store = ProjectStore::at(dir.path());
        let mut project = writer_store.create("Init").unwrap();
        let project_id = project.id.clone();
        let project_dir = dir.path().join(&project_id);
        let done = Arc::new(AtomicBool::new(false));

        let readers: Vec<_> = (0..3)
            .map(|_| {
                let store = ProjectStore::at(dir.path());
                let id = project_id.clone();
                let done = Arc::clone(&done);
                thread::spawn(move || {
                    while !done.load(Ordering::Relaxed) {
                        let loaded = store.load(&id).unwrap();
                        assert!(
                            loaded.name == "Init" || loaded.name.starts_with("Rev "),
                            "unexpected name {}",
                            loaded.name
                        );
                        let (listed, warnings) = store.list_resilient().unwrap();
                        assert!(warnings.is_empty(), "warnings: {warnings:?}");
                        assert_eq!(listed.len(), 1);
                        assert!(
                            listed[0].name == "Init" || listed[0].name.starts_with("Rev "),
                            "unexpected listed name {}",
                            listed[0].name
                        );
                    }
                })
            })
            .collect();

        for i in 1..=25 {
            project.name = format!("Rev {i}");
            writer_store.save(&project).unwrap();
        }
        done.store(true, Ordering::Relaxed);
        let results: Vec<_> = readers.into_iter().map(|handle| handle.join()).collect();
        for res in results {
            res.unwrap();
        }

        let files: Vec<_> = fs::read_dir(&project_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, ["project.json"]);
    }

    #[test]
    fn recovers_corrupt_project_from_valid_previous_backup() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(dir.path());
        let project = store.create("Healthy").unwrap();
        let project_dir = dir.path().join(&project.id);
        let dest = project_dir.join("project.json");
        let backup = project_dir.join("project.json.previous");
        let orphan_tmp = project_dir.join(".orphan.tmp");

        fs::copy(&dest, &backup).unwrap();
        fs::write(&dest, b"{truncated").unwrap();
        fs::write(&orphan_tmp, b"partial").unwrap();

        let loaded = store.load(&project.id).unwrap();
        assert_eq!(loaded.name, "Healthy");
        assert!(dest.exists());
        assert!(project_dir.join("project.json.corrupt").exists());
        assert!(!backup.exists());
        assert!(!orphan_tmp.exists());
    }

    #[test]
    fn stale_lock_is_reaped_and_old_guard_drop_preserves_new_lock() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = dir.path().join(Uuid::new_v4().to_string());
        fs::create_dir_all(&project_dir).unwrap();

        let old_guard = DirLockGuard::acquire_with_options(
            &project_dir,
            LOCK_WAIT_TIMEOUT,
            Duration::from_millis(20),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(30));
        let new_guard = DirLockGuard::acquire_with_options(
            &project_dir,
            LOCK_WAIT_TIMEOUT,
            Duration::from_millis(20),
        )
        .unwrap();
        let new_token = new_guard.token.clone();

        drop(old_guard);
        let lock_path = project_dir.join(".save.lock");
        assert!(lock_path.exists());
        let (_, current_token) = read_lock_info(&lock_path).unwrap();
        assert_eq!(current_token.as_deref(), Some(new_token.as_str()));

        drop(new_guard);
        assert!(!lock_path.exists());
    }

    #[test]
    fn legacy_schema_v1_project_loads_and_resaves_with_empty_environments() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(dir.path());
        let project_id = Uuid::new_v4().to_string();
        let coll_id = Uuid::new_v4().to_string();
        let legacy_dir = dir.path().join(&project_id);
        fs::create_dir_all(&legacy_dir).unwrap();
        let legacy_json = format!(
            r#"{{
                "schema_version": 1,
                "id": "{project_id}",
                "name": "Legacy API",
                "variables": {{"base": "https://legacy.example.com"}},
                "collections": [{{"id": "{coll_id}", "name": "Default", "folders": [], "requests": []}}]
            }}"#
        );
        fs::write(legacy_dir.join("project.json"), legacy_json).unwrap();

        let loaded = store.load(&project_id).unwrap();
        assert!(loaded.secret_variables.is_empty());
        assert!(loaded.environments.is_empty());
        assert_eq!(loaded.active_environment, None);
        assert_eq!(
            loaded.effective_variables().get("base").map(String::as_str),
            Some("https://legacy.example.com")
        );
        store.save(&loaded).unwrap();
    }

    #[test]
    fn secret_refs_persist_without_plaintext_secrets_and_validate_environments() {
        use crate::secrets::{MemorySecretStore, SecretStore};

        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(dir.path());
        let secret_store = MemorySecretStore::new();
        let mut project = store.create("Secrets & Envs").unwrap();

        let base_key = format!("fastman:{}:base:api_key", project.id);
        secret_store
            .set(&base_key, "plaintext_secret_canary_value")
            .unwrap();
        project.secret_variables.insert(
            "api_key".into(),
            SecretRef::Keychain {
                key: base_key.clone(),
            },
        );
        let staging_id = project.add_environment("staging");
        let staging_key = format!("fastman:{}:staging:db_pass", project.id);
        secret_store
            .set(&staging_key, "staging_secret_canary_value")
            .unwrap();
        project
            .find_environment_mut(&staging_id)
            .unwrap()
            .secret_variables
            .insert(
                "db_pass".into(),
                SecretRef::Keychain {
                    key: staging_key.clone(),
                },
            );
        project
            .find_environment_mut(&staging_id)
            .unwrap()
            .secret_variables
            .insert(
                "env_secret".into(),
                SecretRef::Env {
                    var: "FASTMAN_ENV_CANARY".into(),
                },
            );
        assert!(project.set_active_environment(Some(staging_id.clone())));
        store.save(&project).unwrap();

        let disk_path = dir.path().join(&project.id).join("project.json");
        let raw = fs::read_to_string(&disk_path).unwrap();
        assert!(!raw.contains("plaintext_secret_canary_value"));
        assert!(!raw.contains("staging_secret_canary_value"));
        assert!(raw.contains(&base_key));
        assert!(raw.contains(&staging_key));

        let reloaded = store.load(&project.id).unwrap();
        assert_eq!(
            reloaded.active_environment.as_deref(),
            Some(staging_id.as_str())
        );
        let resolved = reloaded.resolve_active_variables(&secret_store);
        assert_eq!(
            resolved.values.get("api_key").map(String::as_str),
            Some("plaintext_secret_canary_value")
        );
        assert_eq!(
            resolved.values.get("db_pass").map(String::as_str),
            Some("staging_secret_canary_value")
        );

        // Validation rejects dangling active_environment, duplicate env IDs, overlaps, and bad refs.
        let mut bad_active = reloaded.clone();
        bad_active.active_environment = Some(Uuid::new_v4().to_string());
        assert!(store.save(&bad_active).is_err());

        let mut dup_env = reloaded.clone();
        let dup = dup_env.environments[0].clone();
        dup_env.environments.push(dup);
        assert!(store.save(&dup_env).is_err());

        let mut overlap = reloaded.clone();
        overlap
            .variables
            .insert("api_key".into(), "plain_overlap".into());
        assert!(store.save(&overlap).is_err());

        let mut bad_ref = reloaded.clone();
        bad_ref.secret_variables.insert(
            "bad".into(),
            SecretRef::Keychain {
                key: "bad/../key".into(),
            },
        );
        assert!(store.save(&bad_ref).is_err());

        let mut bad_env_var = reloaded.clone();
        bad_env_var.secret_variables.insert(
            "bad_env".into(),
            SecretRef::Env {
                var: "123_INVALID".into(),
            },
        );
        assert!(store.save(&bad_env_var).is_err());
    }
}
