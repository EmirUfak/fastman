use crate::secrets::{ResolvedVariables, SecretStore};
use anyhow::bail;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_FOLDER_DEPTH: usize = 32;

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyValue {
    pub enabled: bool,
    pub key: String,
    pub value: String,
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum HttpBody {
    #[default]
    None,
    Text(String),
    Json(String),
    FormUrlEncoded(Vec<KeyValue>),
}

/// Authentication is currently session-only. It is intentionally excluded from
/// serialized projects until an OS keychain-backed persistence layer exists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Auth {
    #[default]
    None,
    Basic {
        username: String,
        password: String,
    },
    Bearer {
        token: String,
    },
    ApiKeyHeader {
        name: String,
        value: String,
    },
    ApiKeyQuery {
        name: String,
        value: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiRequest {
    pub id: String,
    pub name: String,
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub params: Vec<KeyValue>,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub body: HttpBody,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_true")]
    pub verify_tls: bool,
    #[serde(skip)]
    pub auth: Auth,
}

fn default_timeout() -> u64 {
    30_000
}

fn default_true() -> bool {
    true
}

impl ApiRequest {
    pub fn new(method: impl Into<String>, name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            method: method.into(),
            url: url.into(),
            params: vec![],
            headers: vec![],
            body: HttpBody::None,
            timeout_ms: default_timeout(),
            verify_tls: default_true(),
            auth: Auth::None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub folders: Vec<Folder>,
    #[serde(default)]
    pub requests: Vec<ApiRequest>,
}

fn find_request_in<'a>(
    requests: &'a [ApiRequest],
    folders: &'a [Folder],
    request_id: &str,
) -> Option<&'a ApiRequest> {
    requests
        .iter()
        .find(|r| r.id == request_id)
        .or_else(|| folders.iter().find_map(|f| f.find_request(request_id)))
}

fn move_item_offset<T>(items: &mut Vec<T>, index: usize, delta: isize) -> bool {
    let target = index as isize + delta;
    if target < 0 || target as usize >= items.len() || target as usize == index {
        return false;
    }
    let item = items.remove(index);
    items.insert(target as usize, item);
    true
}

fn move_item_relative<T>(items: &mut Vec<T>, src_idx: usize, dst_idx: usize, after: bool) -> bool {
    let raw_target = if after { dst_idx + 1 } else { dst_idx };
    let insert_idx = if src_idx < raw_target {
        raw_target - 1
    } else {
        raw_target
    };
    if insert_idx == src_idx {
        return false;
    }
    let item = items.remove(src_idx);
    items.insert(insert_idx, item);
    true
}

fn find_folder_location_in<'a>(
    parent_folder: Option<&'a str>,
    folders: &'a [Folder],
    folder_id: &str,
) -> Option<(Option<&'a str>, usize, usize)> {
    if let Some(index) = folders.iter().position(|f| f.id == folder_id) {
        return Some((parent_folder, index, folders.len()));
    }
    folders
        .iter()
        .find_map(|f| find_folder_location_in(Some(f.id.as_str()), &f.folders, folder_id))
}

fn find_request_vec_mut_in<'a>(
    requests: &'a mut Vec<ApiRequest>,
    folders: &'a mut [Folder],
    request_id: &str,
) -> Option<(&'a mut Vec<ApiRequest>, usize)> {
    if let Some(index) = requests.iter().position(|r| r.id == request_id) {
        return Some((requests, index));
    }
    folders
        .iter_mut()
        .find_map(|f| find_request_vec_mut_in(&mut f.requests, &mut f.folders, request_id))
}

fn find_request_location_in<'a>(
    parent_folder: Option<&'a str>,
    requests: &'a [ApiRequest],
    folders: &'a [Folder],
    request_id: &str,
) -> Option<(Option<&'a str>, usize, usize)> {
    if let Some(index) = requests.iter().position(|r| r.id == request_id) {
        return Some((parent_folder, index, requests.len()));
    }
    folders.iter().find_map(|f| {
        find_request_location_in(Some(f.id.as_str()), &f.requests, &f.folders, request_id)
    })
}

fn find_folder_vec_mut_in<'a>(
    folders: &'a mut Vec<Folder>,
    folder_id: &str,
) -> Option<(&'a mut Vec<Folder>, usize)> {
    if let Some(index) = folders.iter().position(|f| f.id == folder_id) {
        return Some((folders, index));
    }
    folders
        .iter_mut()
        .find_map(|f| find_folder_vec_mut_in(&mut f.folders, folder_id))
}

impl Folder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            folders: vec![],
            requests: vec![],
        }
    }

    pub fn find_request(&self, request_id: &str) -> Option<&ApiRequest> {
        find_request_in(&self.requests, &self.folders, request_id)
    }

    pub fn find_folder(&self, folder_id: &str) -> Option<&Folder> {
        if self.id == folder_id {
            return Some(self);
        }
        self.folders.iter().find_map(|f| f.find_folder(folder_id))
    }

    pub fn find_folder_mut(&mut self, folder_id: &str) -> Option<&mut Folder> {
        if self.id == folder_id {
            return Some(self);
        }
        self.folders
            .iter_mut()
            .find_map(|f| f.find_folder_mut(folder_id))
    }

    pub fn request_count(&self) -> usize {
        self.requests.len() + self.folders.iter().map(Self::request_count).sum::<usize>()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub folders: Vec<Folder>,
    #[serde(default)]
    pub requests: Vec<ApiRequest>,
}

impl Collection {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            folders: vec![],
            requests: vec![],
        }
    }

    pub fn find_request(&self, request_id: &str) -> Option<&ApiRequest> {
        find_request_in(&self.requests, &self.folders, request_id)
    }

    pub fn find_folder(&self, folder_id: &str) -> Option<&Folder> {
        self.folders.iter().find_map(|f| f.find_folder(folder_id))
    }

    pub fn find_folder_mut(&mut self, folder_id: &str) -> Option<&mut Folder> {
        self.folders
            .iter_mut()
            .find_map(|f| f.find_folder_mut(folder_id))
    }

    pub fn request_count(&self) -> usize {
        self.requests.len()
            + self
                .folders
                .iter()
                .map(Folder::request_count)
                .sum::<usize>()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum SecretRef {
    Keychain { key: String },
    Env { var: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secret_variables: BTreeMap<String, SecretRef>,
}

impl Environment {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            variables: BTreeMap::new(),
            secret_variables: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestChain {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub steps: Vec<ChainStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChainStep {
    pub id: String,
    pub request_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secret_variables: BTreeMap<String, SecretRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environments: Vec<Environment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_environment: Option<String>,
    #[serde(default)]
    pub collections: Vec<Collection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chains: Vec<RequestChain>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: new_id(),
            name: name.into(),
            variables: BTreeMap::new(),
            secret_variables: BTreeMap::new(),
            environments: Vec::new(),
            active_environment: None,
            collections: vec![Collection::new("Default Collection")],
            chains: Vec::new(),
        }
    }

    pub fn find_environment(&self, id: &str) -> Option<&Environment> {
        self.environments.iter().find(|env| env.id == id)
    }

    pub fn find_environment_mut(&mut self, id: &str) -> Option<&mut Environment> {
        self.environments.iter_mut().find(|env| env.id == id)
    }

    pub fn find_environment_by_name_or_id(&self, selector: &str) -> anyhow::Result<&Environment> {
        let trimmed = selector.trim();
        if let Some(env) = self.environments.iter().find(|env| env.id == trimmed) {
            return Ok(env);
        }
        let exact: Vec<&Environment> = self
            .environments
            .iter()
            .filter(|env| env.name == trimmed)
            .collect();
        if exact.len() == 1 {
            return Ok(exact[0]);
        }
        if exact.len() > 1 {
            bail!(
                "Environment name '{}' is ambiguous in project '{}'; specify environment ID",
                trimmed,
                self.name
            );
        }
        let fold: Vec<&Environment> = self
            .environments
            .iter()
            .filter(|env| env.name.eq_ignore_ascii_case(trimmed))
            .collect();
        if fold.len() == 1 {
            return Ok(fold[0]);
        }
        if fold.len() > 1 {
            bail!(
                "Environment name '{}' is ambiguous in project '{}'; specify environment ID",
                trimmed,
                self.name
            );
        }
        bail!(
            "Environment '{}' not found in project '{}'",
            trimmed,
            self.name
        );
    }

    pub fn active_env(&self) -> Option<&Environment> {
        self.active_environment
            .as_deref()
            .and_then(|id| self.find_environment(id))
    }

    pub fn set_active_environment(&mut self, env_id: Option<String>) -> bool {
        if let Some(id) = env_id.as_deref()
            && self.find_environment(id).is_none()
        {
            return false;
        }
        if self.active_environment == env_id {
            return false;
        }
        self.active_environment = env_id;
        true
    }

    pub fn add_environment(&mut self, name: impl Into<String>) -> String {
        let raw = name.into();
        let trimmed = raw.trim();
        let clean = if trimmed.is_empty() {
            "New Environment"
        } else {
            trimmed
        };
        let env = Environment::new(clean);
        let id = env.id.clone();
        self.environments.push(env);
        id
    }

    pub fn rename_environment(&mut self, env_id: &str, name: String) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        if let Some(env) = self.find_environment_mut(env_id) {
            env.name = trimmed.to_owned();
            return true;
        }
        false
    }

    pub fn delete_environment(&mut self, env_id: &str) -> Option<Environment> {
        let idx = self.environments.iter().position(|env| env.id == env_id)?;
        let removed = self.environments.remove(idx);
        if self.active_environment.as_deref() == Some(env_id) {
            self.active_environment = None;
        }
        Some(removed)
    }

    pub fn effective_variables(&self) -> BTreeMap<String, String> {
        self.effective_variables_for(self.active_environment.as_deref())
    }

    pub fn effective_variables_for(&self, env_id: Option<&str>) -> BTreeMap<String, String> {
        let mut merged = self.variables.clone();
        if let Some(env) = env_id.and_then(|id| self.find_environment(id)) {
            for key in env.secret_variables.keys() {
                merged.remove(key);
            }
            for (k, v) in &env.variables {
                merged.insert(k.clone(), v.clone());
            }
        }
        merged
    }

    pub fn effective_secret_refs(&self, env_id: Option<&str>) -> BTreeMap<String, SecretRef> {
        let mut merged = self.secret_variables.clone();
        if let Some(env) = env_id.and_then(|id| self.find_environment(id)) {
            for key in env.variables.keys() {
                merged.remove(key);
            }
            for (k, v) in &env.secret_variables {
                merged.insert(k.clone(), v.clone());
            }
        }
        merged
    }

    pub fn effective_variable_count(&self) -> usize {
        let env_id = self.active_environment.as_deref();
        let mut names = BTreeSet::new();
        names.extend(self.effective_variables_for(env_id).into_keys());
        names.extend(self.effective_secret_refs(env_id).into_keys());
        names.len()
    }

    pub fn resolve_variables(
        &self,
        env_id: Option<&str>,
        secret_store: &dyn SecretStore,
    ) -> ResolvedVariables {
        self.resolve_variables_with_env_lookup(env_id, secret_store, |var| std::env::var(var).ok())
    }

    pub fn resolve_active_variables(&self, secret_store: &dyn SecretStore) -> ResolvedVariables {
        self.resolve_variables(self.active_environment.as_deref(), secret_store)
    }

    pub(crate) fn resolve_variables_with_env_lookup(
        &self,
        env_id: Option<&str>,
        secret_store: &dyn SecretStore,
        mut env_lookup: impl FnMut(&str) -> Option<String>,
    ) -> ResolvedVariables {
        let mut resolved = ResolvedVariables::from_plain(self.effective_variables_for(env_id));
        let refs = self.effective_secret_refs(env_id);
        for (name, secret_ref) in refs {
            resolved.secret_names.insert(name.clone());
            match secret_ref {
                SecretRef::Keychain { key } => match secret_store.get(&key) {
                    Ok(Some(value)) => {
                        resolved.values.insert(name, value);
                    }
                    Ok(None) => {
                        resolved
                            .unresolved_secrets
                            .insert(name, "missing from OS credential store".to_owned());
                    }
                    Err(err) => {
                        resolved.unresolved_secrets.insert(name, err.to_string());
                    }
                },
                SecretRef::Env { var } => match env_lookup(&var) {
                    Some(value) => {
                        resolved.values.insert(name, value);
                    }
                    None => {
                        resolved
                            .unresolved_secrets
                            .insert(name, format!("environment variable '{var}' is not set"));
                    }
                },
            }
        }
        resolved
    }

    pub fn first_request_id(&self) -> Option<String> {
        fn from_folder(folder: &Folder) -> Option<String> {
            folder
                .requests
                .first()
                .map(|r| r.id.clone())
                .or_else(|| folder.folders.iter().find_map(from_folder))
        }
        self.collections.iter().find_map(|collection| {
            collection
                .requests
                .first()
                .map(|r| r.id.clone())
                .or_else(|| collection.folders.iter().find_map(from_folder))
        })
    }

    pub fn find_request(&self, request_id: &str) -> Option<&ApiRequest> {
        self.collections
            .iter()
            .find_map(|c| c.find_request(request_id))
    }

    pub fn find_request_mut(&mut self, request_id: &str) -> Option<&mut ApiRequest> {
        self.find_request_vec_mut(request_id)
            .map(|(requests, index)| &mut requests[index])
    }

    pub fn find_folder(&self, folder_id: &str) -> Option<&Folder> {
        self.collections
            .iter()
            .find_map(|c| c.find_folder(folder_id))
    }

    pub fn find_folder_mut(&mut self, folder_id: &str) -> Option<&mut Folder> {
        self.collections
            .iter_mut()
            .find_map(|c| c.find_folder_mut(folder_id))
    }

    pub fn find_collection(&self, collection_id: &str) -> Option<&Collection> {
        self.collections.iter().find(|c| c.id == collection_id)
    }

    pub fn find_collection_mut(&mut self, collection_id: &str) -> Option<&mut Collection> {
        self.collections.iter_mut().find(|c| c.id == collection_id)
    }

    fn ensure_default_collection_mut(&mut self) -> &mut Collection {
        if self.collections.is_empty() {
            self.collections.push(Collection::new("Default Collection"));
        }
        &mut self.collections[0]
    }

    pub fn rename_collection(&mut self, collection_id: &str, name: String) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        if let Some(collection) = self.find_collection_mut(collection_id) {
            collection.name = trimmed.to_owned();
            return true;
        }
        false
    }

    pub fn move_collection_offset(&mut self, collection_id: &str, delta: isize) -> bool {
        let Some(idx) = self.collections.iter().position(|c| c.id == collection_id) else {
            return false;
        };
        move_item_offset(&mut self.collections, idx, delta)
    }

    pub fn delete_collection(&mut self, collection_id: &str) -> bool {
        if self.collections.len() <= 1 {
            return false;
        }
        let before = self.collections.len();
        self.collections.retain(|c| c.id != collection_id);
        self.collections.len() != before
    }

    /// Inserts `folder` inside `parent_folder_id` if provided and found; otherwise inserts at the
    /// root of the first collection (creating a default collection if needed). Returns the ID of
    /// the container (`parent_folder_id` or collection ID) that received the folder.
    pub fn insert_folder(&mut self, parent_folder_id: Option<&str>, folder: Folder) -> String {
        if let Some(parent_id) = parent_folder_id
            && let Some(parent) = self.find_folder_mut(parent_id)
        {
            parent.folders.push(folder);
            return parent_id.to_owned();
        }
        let collection = self.ensure_default_collection_mut();
        collection.folders.push(folder);
        collection.id.clone()
    }

    /// Inserts `request` inside `target_folder_id` if provided and found; otherwise inserts at the
    /// root of the first collection (creating a default collection if needed). Returns the ID of
    /// the container (`target_folder_id` or collection ID) that received the request.
    pub fn insert_request(
        &mut self,
        target_folder_id: Option<&str>,
        request: ApiRequest,
    ) -> String {
        if let Some(folder_id) = target_folder_id
            && let Some(folder) = self.find_folder_mut(folder_id)
        {
            folder.requests.push(request);
            return folder_id.to_owned();
        }
        let collection = self.ensure_default_collection_mut();
        collection.requests.push(request);
        collection.id.clone()
    }

    pub fn rename_folder(&mut self, folder_id: &str, name: String) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        if let Some(folder) = self.find_folder_mut(folder_id) {
            folder.name = trimmed.to_owned();
            return true;
        }
        false
    }

    pub fn delete_folder(&mut self, folder_id: &str) -> bool {
        self.take_folder(folder_id).is_some()
    }

    fn take_folder(&mut self, folder_id: &str) -> Option<Folder> {
        let (folders, index) = self.find_folder_vec_mut(folder_id)?;
        Some(folders.remove(index))
    }

    pub fn delete_request(&mut self, request_id: &str) -> bool {
        self.take_request(request_id).is_some()
    }

    fn take_request(&mut self, request_id: &str) -> Option<ApiRequest> {
        let (requests, index) = self.find_request_vec_mut(request_id)?;
        Some(requests.remove(index))
    }

    pub fn duplicate_request(&mut self, request_id: &str) -> Option<String> {
        let (requests, index) = self.find_request_vec_mut(request_id)?;
        let mut copy = requests[index].clone();
        copy.id = new_id();
        copy.name = format!("{} (copy)", copy.name);
        let new_request_id = copy.id.clone();
        requests.insert(index + 1, copy);
        Some(new_request_id)
    }

    /// Returns `(collection_id, parent_folder_id, index_in_container, container_len)`.
    pub fn find_request_location(
        &self,
        request_id: &str,
    ) -> Option<(&str, Option<&str>, usize, usize)> {
        self.collections.iter().find_map(|c| {
            find_request_location_in(None, &c.requests, &c.folders, request_id)
                .map(|(folder, idx, len)| (c.id.as_str(), folder, idx, len))
        })
    }

    /// Returns the immediate parent folder ID if the request lives inside a folder, or `None` if at root.
    pub fn request_parent_folder(&self, request_id: &str) -> Option<String> {
        self.find_request_location(request_id)
            .and_then(|(_, folder, _, _)| folder.map(str::to_owned))
    }

    /// Returns `(index, total_siblings)` for a request within its immediate folder or collection root.
    pub fn request_order_bounds(&self, request_id: &str) -> Option<(usize, usize)> {
        self.find_request_location(request_id)
            .map(|(_, _, idx, len)| (idx, len))
    }

    /// Returns `(collection_id, parent_folder_id, index_in_container, container_len)`.
    pub fn find_folder_location(
        &self,
        folder_id: &str,
    ) -> Option<(&str, Option<&str>, usize, usize)> {
        self.collections.iter().find_map(|c| {
            find_folder_location_in(None, &c.folders, folder_id)
                .map(|(parent, idx, len)| (c.id.as_str(), parent, idx, len))
        })
    }

    /// Returns the immediate parent folder ID if the folder is nested, or `None` if at root.
    pub fn folder_parent_folder(&self, folder_id: &str) -> Option<String> {
        self.find_folder_location(folder_id)
            .and_then(|(_, parent, _, _)| parent.map(str::to_owned))
    }

    /// Returns `true` if `candidate_id` is `ancestor_id` itself or nested anywhere inside it.
    pub fn is_folder_descendant(&self, ancestor_id: &str, candidate_id: &str) -> bool {
        self.find_folder(ancestor_id)
            .is_some_and(|folder| folder.find_folder(candidate_id).is_some())
    }

    /// Returns `(index, total_siblings)` for a folder within its immediate parent.
    pub fn folder_order_bounds(&self, folder_id: &str) -> Option<(usize, usize)> {
        self.find_folder_location(folder_id)
            .map(|(_, _, index, len)| (index, len))
    }

    /// Flattened `(folder_id, display_path)` list of all folders in the project.
    pub fn folder_targets(&self) -> Vec<(String, String)> {
        fn collect(folders: &[Folder], prefix: &str, out: &mut Vec<(String, String)>) {
            for folder in folders {
                let path = if prefix.is_empty() {
                    folder.name.clone()
                } else {
                    format!("{prefix} / {}", folder.name)
                };
                out.push((folder.id.clone(), path.clone()));
                collect(&folder.folders, &path, out);
            }
        }
        let mut out = Vec::new();
        for collection in &self.collections {
            collect(&collection.folders, "", &mut out);
        }
        out
    }

    fn find_request_vec_mut(&mut self, request_id: &str) -> Option<(&mut Vec<ApiRequest>, usize)> {
        self.collections
            .iter_mut()
            .find_map(|c| find_request_vec_mut_in(&mut c.requests, &mut c.folders, request_id))
    }

    /// Shifts a request by `delta` (`-1` for up, `+1` for down) within its current container.
    pub fn move_request_offset(&mut self, request_id: &str, delta: isize) -> bool {
        let Some((requests, index)) = self.find_request_vec_mut(request_id) else {
            return false;
        };
        move_item_offset(requests, index, delta)
    }

    /// Moves a request into `target_folder_id` (`Some(id)`) or out to its collection root (`None`).
    pub fn move_request_to_folder(
        &mut self,
        request_id: &str,
        target_folder_id: Option<&str>,
    ) -> bool {
        let Some((coll_id, current_folder, _, _)) = self.find_request_location(request_id) else {
            return false;
        };
        if current_folder == target_folder_id {
            return false;
        }
        let coll_id = coll_id.to_owned();
        if let Some(folder_id) = target_folder_id
            && self.find_folder(folder_id).is_none()
        {
            return false;
        }
        let Some(request) = self.take_request(request_id) else {
            return false;
        };
        if let Some(folder_id) = target_folder_id {
            if let Some(folder) = self.find_folder_mut(folder_id) {
                folder.requests.push(request);
                return true;
            }
            false
        } else if let Some(collection) = self.find_collection_mut(&coll_id) {
            collection.requests.push(request);
            true
        } else if let Some(collection) = self.collections.first_mut() {
            collection.requests.push(request);
            true
        } else {
            false
        }
    }

    /// Moves a request to the root of a specific collection.
    pub fn move_request_to_collection(
        &mut self,
        request_id: &str,
        target_collection_id: &str,
    ) -> bool {
        let Some((coll_id, current_folder, _, _)) = self.find_request_location(request_id) else {
            return false;
        };
        if coll_id == target_collection_id && current_folder.is_none() {
            return false;
        }
        if self.find_collection(target_collection_id).is_none() {
            return false;
        }
        let Some(request) = self.take_request(request_id) else {
            return false;
        };
        if let Some(collection) = self.find_collection_mut(target_collection_id) {
            collection.requests.push(request);
            true
        } else {
            false
        }
    }

    /// Reorders/moves `request_id` immediately before (`after == false`) or after (`after == true`)
    /// `target_request_id` (moving across folders/collections if needed).
    pub fn move_request_relative(
        &mut self,
        request_id: &str,
        target_request_id: &str,
        after: bool,
    ) -> bool {
        if request_id == target_request_id {
            return false;
        }
        let Some((src_coll, src_folder, src_idx, _)) = self.find_request_location(request_id)
        else {
            return false;
        };
        let Some((dst_coll, dst_folder, dst_idx, _)) =
            self.find_request_location(target_request_id)
        else {
            return false;
        };
        let same_container = src_coll == dst_coll && src_folder == dst_folder;
        if same_container {
            let Some((requests, _)) = self.find_request_vec_mut(request_id) else {
                return false;
            };
            move_item_relative(requests, src_idx, dst_idx, after)
        } else {
            let Some(req) = self.take_request(request_id) else {
                return false;
            };
            let Some((requests, target_idx)) = self.find_request_vec_mut(target_request_id) else {
                return false;
            };
            let insert_idx = if after { target_idx + 1 } else { target_idx };
            requests.insert(insert_idx, req);
            true
        }
    }

    fn find_folder_vec_mut(&mut self, folder_id: &str) -> Option<(&mut Vec<Folder>, usize)> {
        self.collections
            .iter_mut()
            .find_map(|c| find_folder_vec_mut_in(&mut c.folders, folder_id))
    }

    /// Shifts a folder by `delta` (`-1` for up, `+1` for down) within its current parent.
    pub fn move_folder_offset(&mut self, folder_id: &str, delta: isize) -> bool {
        let Some((folders, index)) = self.find_folder_vec_mut(folder_id) else {
            return false;
        };
        move_item_offset(folders, index, delta)
    }

    /// Moves a folder inside `target_folder_id` (`Some(id)`) or out to its collection root (`None`).
    pub fn move_folder_to_folder(
        &mut self,
        folder_id: &str,
        target_folder_id: Option<&str>,
    ) -> bool {
        let Some((coll_id, current_parent, _, _)) = self.find_folder_location(folder_id) else {
            return false;
        };
        if current_parent == target_folder_id {
            return false;
        }
        let coll_id = coll_id.to_owned();
        if let Some(target_id) = target_folder_id
            && (self.find_folder(target_id).is_none()
                || self.is_folder_descendant(folder_id, target_id))
        {
            return false;
        }
        let Some(folder) = self.take_folder(folder_id) else {
            return false;
        };
        if let Some(target_id) = target_folder_id {
            if let Some(parent) = self.find_folder_mut(target_id) {
                parent.folders.push(folder);
                return true;
            }
            false
        } else if let Some(collection) = self.find_collection_mut(&coll_id) {
            collection.folders.push(folder);
            true
        } else if let Some(collection) = self.collections.first_mut() {
            collection.folders.push(folder);
            true
        } else {
            false
        }
    }

    /// Moves a folder to the root of a specific collection.
    pub fn move_folder_to_collection(
        &mut self,
        folder_id: &str,
        target_collection_id: &str,
    ) -> bool {
        let Some((coll_id, current_parent, _, _)) = self.find_folder_location(folder_id) else {
            return false;
        };
        if coll_id == target_collection_id && current_parent.is_none() {
            return false;
        }
        if self.find_collection(target_collection_id).is_none() {
            return false;
        }
        let Some(folder) = self.take_folder(folder_id) else {
            return false;
        };
        if let Some(collection) = self.find_collection_mut(target_collection_id) {
            collection.folders.push(folder);
            true
        } else {
            false
        }
    }

    /// Reorders or moves `folder_id` immediately before (`after == false`) or after (`after == true`)
    /// `target_folder_id`.
    pub fn move_folder_relative(
        &mut self,
        folder_id: &str,
        target_folder_id: &str,
        after: bool,
    ) -> bool {
        if folder_id == target_folder_id || self.is_folder_descendant(folder_id, target_folder_id) {
            return false;
        }
        let Some((src_coll, src_parent, src_idx, _)) = self.find_folder_location(folder_id) else {
            return false;
        };
        let Some((dst_coll, dst_parent, dst_idx, _)) = self.find_folder_location(target_folder_id)
        else {
            return false;
        };
        let same_parent = src_coll == dst_coll && src_parent == dst_parent;
        if same_parent {
            let Some((folders, _)) = self.find_folder_vec_mut(folder_id) else {
                return false;
            };
            move_item_relative(folders, src_idx, dst_idx, after)
        } else {
            let Some(folder) = self.take_folder(folder_id) else {
                return false;
            };
            let Some((folders, target_idx)) = self.find_folder_vec_mut(target_folder_id) else {
                return false;
            };
            let insert_idx = if after { target_idx + 1 } else { target_idx };
            folders.insert(insert_idx, folder);
            true
        }
    }

    pub fn request_count(&self) -> usize {
        self.collections.iter().map(Collection::request_count).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_request_lookup() {
        let mut project = Project::new("Test");
        let mut outer = Folder::new("outer");
        let mut inner = Folder::new("inner");
        let request = ApiRequest::new("GET", "status", "https://example.com/status");
        let id = request.id.clone();
        inner.requests.push(request);
        outer.folders.push(inner);
        project.collections[0].folders.push(outer);
        assert_eq!(project.find_request(&id).unwrap().name, "status");
        project.find_request_mut(&id).unwrap().method = "PATCH".into();
        assert_eq!(project.find_request(&id).unwrap().method, "PATCH");
    }

    #[test]
    fn credentials_are_not_serialized() {
        let mut request = ApiRequest::new("GET", "test", "https://example.com");
        request.auth = Auth::Bearer {
            token: "sensitive-canary".into(),
        };
        let bytes = serde_json::to_string(&request).unwrap();
        assert!(!bytes.contains("sensitive-canary"));
        assert_eq!(
            serde_json::from_str::<ApiRequest>(&bytes).unwrap().auth,
            Auth::None
        );
    }

    #[test]
    fn tree_crud_operations() {
        let mut project = Project::new("CRUD");
        let coll_id = project.collections[0].id.clone();
        let root_folder = Folder::new("Auth");
        let root_folder_id = root_folder.id.clone();
        assert_eq!(project.insert_folder(None, root_folder), coll_id);

        let sub = Folder::new("OAuth");
        let sub_id = sub.id.clone();
        assert_eq!(
            project.insert_folder(Some(&root_folder_id), sub),
            root_folder_id
        );
        assert!(project.rename_folder(&sub_id, "Tokens".into()));
        assert_eq!(project.find_folder(&sub_id).unwrap().name, "Tokens");

        let req = ApiRequest::new("POST", "Login", "https://example.com/login");
        let req_id = req.id.clone();
        assert_eq!(project.insert_request(Some(&sub_id), req), sub_id);
        assert_eq!(project.request_count(), 1);

        let dup_id = project.duplicate_request(&req_id).unwrap();
        assert_ne!(dup_id, req_id);
        assert_eq!(project.find_request(&dup_id).unwrap().name, "Login (copy)");
        assert_eq!(project.request_count(), 2);

        assert!(project.delete_request(&req_id));
        assert!(project.find_request(&req_id).is_none());
        assert_eq!(project.request_count(), 1);

        assert!(project.delete_folder(&sub_id));
        assert!(project.find_folder(&sub_id).is_none());
        assert_eq!(project.request_count(), 0);
    }

    #[test]
    fn move_and_reorder_requests_and_folders() {
        let mut project = Project::new("Order");
        let folder_a = Folder::new("Alpha");
        let folder_a_id = folder_a.id.clone();
        let folder_b = Folder::new("Beta");
        let folder_b_id = folder_b.id.clone();
        project.collections[0].folders.push(folder_a);
        project.collections[0].folders.push(folder_b);

        let r1 = ApiRequest::new("GET", "One", "https://example.com/1");
        let r1_id = r1.id.clone();
        let r2 = ApiRequest::new("POST", "Two", "https://example.com/2");
        let r2_id = r2.id.clone();
        let r3 = ApiRequest::new("DELETE", "Three", "https://example.com/3");
        let r3_id = r3.id.clone();
        project.collections[0].requests.extend([r1, r2, r3]);

        // Reorder within collection root using offset
        assert!(!project.move_request_offset(&r1_id, -1));
        assert!(project.move_request_offset(&r1_id, 1));
        assert_eq!(
            project.collections[0]
                .requests
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec![r2_id.as_str(), r1_id.as_str(), r3_id.as_str()]
        );

        // Reorder within same container using relative insertion
        assert!(project.move_request_relative(&r3_id, &r2_id, false));
        assert!(!project.move_request_relative(&r3_id, &r2_id, false));
        assert_eq!(
            project.collections[0]
                .requests
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec![r3_id.as_str(), r2_id.as_str(), r1_id.as_str()]
        );
        assert!(project.move_request_relative(&r3_id, &r2_id, true));
        assert_eq!(
            project.collections[0]
                .requests
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec![r2_id.as_str(), r3_id.as_str(), r1_id.as_str()]
        );

        // Move into folder Alpha
        assert!(project.move_request_to_folder(&r1_id, Some(&folder_a_id)));
        assert_eq!(
            project.request_parent_folder(&r1_id).as_deref(),
            Some(folder_a_id.as_str())
        );
        assert_eq!(project.collections[0].requests.len(), 2);
        assert_eq!(project.find_folder(&folder_a_id).unwrap().requests.len(), 1);

        // Drag/move r2 before r1 inside folder Alpha
        assert!(project.move_request_relative(&r2_id, &r1_id, false));
        assert_eq!(
            project.request_parent_folder(&r2_id).as_deref(),
            Some(folder_a_id.as_str())
        );
        assert_eq!(
            project
                .find_folder(&folder_a_id)
                .unwrap()
                .requests
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec![r2_id.as_str(), r1_id.as_str()]
        );

        // Move r1 out of folder back to collection root
        assert!(project.move_request_to_folder(&r1_id, None));
        assert_eq!(project.request_parent_folder(&r1_id), None);
        assert_eq!(project.collections[0].requests.len(), 2);

        // Reorder folders
        assert!(project.move_folder_offset(&folder_b_id, -1));
        assert_eq!(project.collections[0].folders[0].id, folder_b_id);
        assert_eq!(project.collections[0].folders[1].id, folder_a_id);
    }

    #[test]
    fn move_and_nest_folders() {
        let mut project = Project::new("Folders");
        let a = Folder::new("A");
        let a_id = a.id.clone();
        let b = Folder::new("B");
        let b_id = b.id.clone();
        let c = Folder::new("C");
        let c_id = c.id.clone();
        project.collections[0].folders.extend([a, b, c]);

        // Reorder C before A -> [C, A, B]
        assert!(project.move_folder_relative(&c_id, &a_id, false));
        assert_eq!(
            project.collections[0]
                .folders
                .iter()
                .map(|f| f.id.as_str())
                .collect::<Vec<_>>(),
            vec![c_id.as_str(), a_id.as_str(), b_id.as_str()]
        );

        // No-op when already in place
        assert!(!project.move_folder_relative(&c_id, &a_id, false));

        // Reorder C after B -> [A, B, C]
        assert!(project.move_folder_relative(&c_id, &b_id, true));
        assert_eq!(
            project.collections[0]
                .folders
                .iter()
                .map(|f| f.id.as_str())
                .collect::<Vec<_>>(),
            vec![a_id.as_str(), b_id.as_str(), c_id.as_str()]
        );

        // Move B inside A
        assert!(project.move_folder_to_folder(&b_id, Some(&a_id)));
        assert_eq!(
            project.folder_parent_folder(&b_id).as_deref(),
            Some(a_id.as_str())
        );

        // Cannot move A into itself or its descendant B
        assert!(!project.move_folder_to_folder(&a_id, Some(&a_id)));
        assert!(!project.move_folder_to_folder(&a_id, Some(&b_id)));
        assert!(!project.move_folder_relative(&a_id, &b_id, false));
        assert!(project.find_folder(&a_id).is_some());
        assert!(project.find_folder(&b_id).is_some());

        // Move C before B (across parents: root -> inside A)
        assert!(project.move_folder_relative(&c_id, &b_id, false));
        assert_eq!(
            project.folder_parent_folder(&c_id).as_deref(),
            Some(a_id.as_str())
        );
        assert_eq!(
            project
                .find_folder(&a_id)
                .unwrap()
                .folders
                .iter()
                .map(|f| f.id.as_str())
                .collect::<Vec<_>>(),
            vec![c_id.as_str(), b_id.as_str()]
        );

        // Move B out to collection root
        assert!(project.move_folder_to_folder(&b_id, None));
        assert_eq!(project.folder_parent_folder(&b_id), None);
        assert_eq!(project.collections[0].folders.len(), 2);
    }

    #[test]
    fn rename_move_and_delete_multiple_collections() {
        let mut project = Project::new("API");
        let first_id = project.collections[0].id.clone();
        // Cannot delete sole remaining collection
        assert!(!project.delete_collection(&first_id));

        let second = Collection::new("Extra");
        let second_id = second.id.clone();
        project.collections.push(second);

        assert!(project.rename_collection(&second_id, "  V2 API  ".into()));
        assert_eq!(project.find_collection(&second_id).unwrap().name, "V2 API");

        assert!(project.move_collection_offset(&second_id, -1));
        assert_eq!(project.collections[0].id, second_id);

        assert!(project.delete_collection(&first_id));
        assert_eq!(project.collections.len(), 1);
    }

    #[test]
    fn verify_tls_defaults_true_for_new_and_legacy_json_and_roundtrips_false() {
        let mut req = ApiRequest::new("GET", "TLS", "https://example.com");
        assert!(req.verify_tls);

        let legacy_json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "Legacy",
            "method": "GET",
            "url": "https://example.com"
        }"#;
        let legacy: ApiRequest = serde_json::from_str(legacy_json).unwrap();
        assert!(legacy.verify_tls);

        req.verify_tls = false;
        let serialized = serde_json::to_string(&req).unwrap();
        let roundtrip: ApiRequest = serde_json::from_str(&serialized).unwrap();
        assert!(!roundtrip.verify_tls);
    }

    #[test]
    fn environment_overlays_resolve_plain_and_secret_variables_with_precedence() {
        use crate::secrets::MemorySecretStore;

        let mut project = Project::new("Envs");
        project
            .variables
            .insert("host".into(), "http://localhost:8080".into());
        project
            .variables
            .insert("mode".into(), "plain_default".into());
        project.secret_variables.insert(
            "api_key".into(),
            SecretRef::Keychain {
                key: "fastman:base:api_key".into(),
            },
        );
        project.secret_variables.insert(
            "mode_secret".into(),
            SecretRef::Env {
                var: "BASE_MODE_SEC".into(),
            },
        );

        let store = MemorySecretStore::new();
        store.set("fastman:base:api_key", "base_secret").unwrap();
        store.set("fastman:prod:api_key", "prod_secret").unwrap();

        let local_id = project.add_environment("local");
        let prod_id = project.add_environment("prod");
        {
            let prod = project.find_environment_mut(&prod_id).unwrap();
            prod.variables
                .insert("host".into(), "https://api.example.com".into());
            // Environment plain overrides base secret_variable `mode_secret`.
            prod.variables
                .insert("mode_secret".into(), "override_plain".into());
            // Environment secret_variable overrides base plain variable `mode` and base secret `api_key`.
            prod.secret_variables.insert(
                "mode".into(),
                SecretRef::Env {
                    var: "PROD_MODE_TOKEN".into(),
                },
            );
            prod.secret_variables.insert(
                "api_key".into(),
                SecretRef::Keychain {
                    key: "fastman:prod:api_key".into(),
                },
            );
        }

        assert!(project.set_active_environment(Some(prod_id.clone())));
        assert_eq!(project.effective_variable_count(), 4);

        let resolved = project.resolve_variables_with_env_lookup(
            project.active_environment.as_deref(),
            &store,
            |var| match var {
                "PROD_MODE_TOKEN" => Some("env_secret_token".into()),
                _ => None,
            },
        );
        assert_eq!(
            resolved.values.get("host").map(String::as_str),
            Some("https://api.example.com")
        );
        assert_eq!(
            resolved.values.get("mode_secret").map(String::as_str),
            Some("override_plain")
        );
        assert!(!resolved.secret_names.contains("mode_secret"));
        assert!(resolved.secret_names.contains("mode"));
        assert!(resolved.secret_names.contains("api_key"));
        assert_eq!(
            resolved.values.get("mode").map(String::as_str),
            Some("env_secret_token")
        );
        assert_eq!(
            resolved.values.get("api_key").map(String::as_str),
            Some("prod_secret")
        );

        assert_eq!(
            project
                .find_environment_by_name_or_id(&local_id)
                .unwrap()
                .name,
            "local"
        );
        assert_eq!(
            project.find_environment_by_name_or_id("PROD").unwrap().id,
            prod_id
        );
        assert!(project.find_environment_by_name_or_id("missing").is_err());

        // Deleting active environment clears active_environment cleanly.
        assert!(project.delete_environment(&prod_id).is_some());
        assert_eq!(project.active_environment, None);
    }
}
