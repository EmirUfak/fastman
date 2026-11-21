use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
    #[serde(skip)]
    pub auth: Auth,
}

fn default_timeout() -> u64 {
    30_000
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

fn find_request_mut_in<'a>(
    requests: &'a mut [ApiRequest],
    folders: &'a mut [Folder],
    request_id: &str,
) -> Option<&'a mut ApiRequest> {
    if let Some(index) = requests.iter().position(|r| r.id == request_id) {
        return requests.get_mut(index);
    }
    folders
        .iter_mut()
        .find_map(|f| f.find_request_mut(request_id))
}

fn delete_folder_in(folders: &mut Vec<Folder>, folder_id: &str) -> bool {
    take_folder_in(folders, folder_id).is_some()
}

fn take_folder_in(folders: &mut Vec<Folder>, folder_id: &str) -> Option<Folder> {
    let (folders, index) = find_folder_vec_mut_in(folders, folder_id)?;
    Some(folders.remove(index))
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

fn delete_request_in(
    requests: &mut Vec<ApiRequest>,
    folders: &mut [Folder],
    request_id: &str,
) -> bool {
    take_request_in(requests, folders, request_id).is_some()
}

fn take_request_in(
    requests: &mut Vec<ApiRequest>,
    folders: &mut [Folder],
    request_id: &str,
) -> Option<ApiRequest> {
    let (requests, index) = find_request_vec_mut_in(requests, folders, request_id)?;
    Some(requests.remove(index))
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

fn duplicate_request_in(
    requests: &mut Vec<ApiRequest>,
    folders: &mut [Folder],
    request_id: &str,
) -> Option<String> {
    let (requests, index) = find_request_vec_mut_in(requests, folders, request_id)?;
    let mut copy = requests[index].clone();
    copy.id = new_id();
    copy.name = format!("{} (copy)", copy.name);
    let new_request_id = copy.id.clone();
    requests.insert(index + 1, copy);
    Some(new_request_id)
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

    pub fn find_request_mut(&mut self, request_id: &str) -> Option<&mut ApiRequest> {
        find_request_mut_in(&mut self.requests, &mut self.folders, request_id)
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

    pub fn delete_folder(&mut self, folder_id: &str) -> bool {
        delete_folder_in(&mut self.folders, folder_id)
    }

    pub fn delete_request(&mut self, request_id: &str) -> bool {
        delete_request_in(&mut self.requests, &mut self.folders, request_id)
    }

    pub fn take_request(&mut self, request_id: &str) -> Option<ApiRequest> {
        take_request_in(&mut self.requests, &mut self.folders, request_id)
    }

    pub fn duplicate_request(&mut self, request_id: &str) -> Option<String> {
        duplicate_request_in(&mut self.requests, &mut self.folders, request_id)
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

    pub fn find_request_mut(&mut self, request_id: &str) -> Option<&mut ApiRequest> {
        find_request_mut_in(&mut self.requests, &mut self.folders, request_id)
    }

    pub fn find_folder(&self, folder_id: &str) -> Option<&Folder> {
        self.folders.iter().find_map(|f| f.find_folder(folder_id))
    }

    pub fn find_folder_mut(&mut self, folder_id: &str) -> Option<&mut Folder> {
        self.folders
            .iter_mut()
            .find_map(|f| f.find_folder_mut(folder_id))
    }

    pub fn delete_folder(&mut self, folder_id: &str) -> bool {
        delete_folder_in(&mut self.folders, folder_id)
    }

    pub fn delete_request(&mut self, request_id: &str) -> bool {
        delete_request_in(&mut self.requests, &mut self.folders, request_id)
    }

    pub fn take_request(&mut self, request_id: &str) -> Option<ApiRequest> {
        take_request_in(&mut self.requests, &mut self.folders, request_id)
    }

    pub fn duplicate_request(&mut self, request_id: &str) -> Option<String> {
        duplicate_request_in(&mut self.requests, &mut self.folders, request_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(default)]
    pub collections: Vec<Collection>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: new_id(),
            name: name.into(),
            variables: BTreeMap::new(),
            collections: vec![Collection::new("Default Collection")],
        }
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
        self.collections
            .iter_mut()
            .find_map(|c| c.find_request_mut(request_id))
    }

    pub fn find_folder(&self, folder_id: &str) -> Option<&Folder> {
        self.collections
            .iter()
            .find_map(|c| c.find_folder(folder_id))
    }

    pub fn find_collection(&self, collection_id: &str) -> Option<&Collection> {
        self.collections.iter().find(|c| c.id == collection_id)
    }

    pub fn rename_collection(&mut self, collection_id: &str, name: String) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        if let Some(collection) = self.collections.iter_mut().find(|c| c.id == collection_id) {
            collection.name = trimmed.to_owned();
            return true;
        }
        false
    }

    pub fn move_collection_offset(&mut self, collection_id: &str, delta: isize) -> bool {
        let Some(idx) = self.collections.iter().position(|c| c.id == collection_id) else {
            return false;
        };
        let target = idx as isize + delta;
        if target < 0 || target as usize >= self.collections.len() || target as usize == idx {
            return false;
        }
        let coll = self.collections.remove(idx);
        self.collections.insert(target as usize, coll);
        true
    }

    pub fn delete_collection(&mut self, collection_id: &str) -> bool {
        if self.collections.len() <= 1 {
            return false;
        }
        let before = self.collections.len();
        self.collections.retain(|c| c.id != collection_id);
        self.collections.len() != before
    }

    pub fn add_subfolder(&mut self, parent_folder_id: &str, folder: Folder) -> bool {
        for collection in &mut self.collections {
            if let Some(parent) = collection.find_folder_mut(parent_folder_id) {
                parent.folders.push(folder);
                return true;
            }
        }
        false
    }

    pub fn rename_folder(&mut self, folder_id: &str, name: String) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        for collection in &mut self.collections {
            if let Some(folder) = collection.find_folder_mut(folder_id) {
                folder.name = trimmed.to_owned();
                return true;
            }
        }
        false
    }

    pub fn delete_folder(&mut self, folder_id: &str) -> bool {
        self.collections
            .iter_mut()
            .any(|c| c.delete_folder(folder_id))
    }

    pub fn take_folder(&mut self, folder_id: &str) -> Option<Folder> {
        self.collections
            .iter_mut()
            .find_map(|c| take_folder_in(&mut c.folders, folder_id))
    }

    pub fn delete_request(&mut self, request_id: &str) -> bool {
        self.collections
            .iter_mut()
            .any(|c| c.delete_request(request_id))
    }

    pub fn take_request(&mut self, request_id: &str) -> Option<ApiRequest> {
        self.collections
            .iter_mut()
            .find_map(|c| c.take_request(request_id))
    }

    pub fn duplicate_request(&mut self, request_id: &str) -> Option<String> {
        self.collections
            .iter_mut()
            .find_map(|c| c.duplicate_request(request_id))
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
        let target = index as isize + delta;
        if target < 0 || target as usize >= requests.len() || target as usize == index {
            return false;
        }
        let req = requests.remove(index);
        requests.insert(target as usize, req);
        true
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
            for collection in &mut self.collections {
                if let Some(folder) = collection.find_folder_mut(folder_id) {
                    folder.requests.push(request);
                    return true;
                }
            }
            false
        } else if let Some(collection) = self.collections.iter_mut().find(|c| c.id == coll_id) {
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
        if !self
            .collections
            .iter()
            .any(|c| c.id == target_collection_id)
        {
            return false;
        }
        let Some(request) = self.take_request(request_id) else {
            return false;
        };
        if let Some(collection) = self
            .collections
            .iter_mut()
            .find(|c| c.id == target_collection_id)
        {
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
            let raw_target = if after { dst_idx + 1 } else { dst_idx };
            let insert_idx = if src_idx < raw_target {
                raw_target - 1
            } else {
                raw_target
            };
            if insert_idx == src_idx {
                return false;
            }
            let Some((requests, _)) = self.find_request_vec_mut(request_id) else {
                return false;
            };
            let req = requests.remove(src_idx);
            requests.insert(insert_idx, req);
            true
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

    /// Reorders/moves `request_id` to the position of `target_request_id` (moving across folders if needed).
    pub fn move_request_to_target(&mut self, request_id: &str, target_request_id: &str) -> bool {
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
            let req = requests.remove(src_idx);
            requests.insert(dst_idx, req);
            true
        } else {
            let Some(req) = self.take_request(request_id) else {
                return false;
            };
            let Some((requests, target_idx)) = self.find_request_vec_mut(target_request_id) else {
                return false;
            };
            requests.insert(target_idx, req);
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
        let target = index as isize + delta;
        if target < 0 || target as usize >= folders.len() || target as usize == index {
            return false;
        }
        let folder = folders.remove(index);
        folders.insert(target as usize, folder);
        true
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
            for collection in &mut self.collections {
                if let Some(parent) = collection.find_folder_mut(target_id) {
                    parent.folders.push(folder);
                    return true;
                }
            }
            false
        } else if let Some(collection) = self.collections.iter_mut().find(|c| c.id == coll_id) {
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
        if !self
            .collections
            .iter()
            .any(|c| c.id == target_collection_id)
        {
            return false;
        }
        let Some(folder) = self.take_folder(folder_id) else {
            return false;
        };
        if let Some(collection) = self
            .collections
            .iter_mut()
            .find(|c| c.id == target_collection_id)
        {
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
            let raw_target = if after { dst_idx + 1 } else { dst_idx };
            let insert_idx = if src_idx < raw_target {
                raw_target - 1
            } else {
                raw_target
            };
            if insert_idx == src_idx {
                return false;
            }
            let Some((folders, _)) = self.find_folder_vec_mut(folder_id) else {
                return false;
            };
            let folder = folders.remove(src_idx);
            folders.insert(insert_idx, folder);
            true
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
        fn count_folder(folder: &Folder) -> usize {
            folder.requests.len() + folder.folders.iter().map(count_folder).sum::<usize>()
        }
        self.collections
            .iter()
            .map(|c| c.requests.len() + c.folders.iter().map(count_folder).sum::<usize>())
            .sum()
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
        let root_folder = Folder::new("Auth");
        let root_folder_id = root_folder.id.clone();
        project.collections[0].folders.push(root_folder);

        let sub = Folder::new("OAuth");
        let sub_id = sub.id.clone();
        assert!(project.add_subfolder(&root_folder_id, sub));
        assert!(project.rename_folder(&sub_id, "Tokens".into()));
        assert_eq!(project.find_folder(&sub_id).unwrap().name, "Tokens");

        let req = ApiRequest::new("POST", "Login", "https://example.com/login");
        let req_id = req.id.clone();
        project.collections[0]
            .find_folder_mut(&sub_id)
            .unwrap()
            .requests
            .push(req);
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
        assert!(project.move_request_to_target(&r3_id, &r2_id));
        assert_eq!(
            project.collections[0]
                .requests
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec![r3_id.as_str(), r2_id.as_str(), r1_id.as_str()]
        );

        // Move into folder Alpha
        assert!(project.move_request_to_folder(&r1_id, Some(&folder_a_id)));
        assert_eq!(
            project.request_parent_folder(&r1_id).as_deref(),
            Some(folder_a_id.as_str())
        );
        assert_eq!(project.collections[0].requests.len(), 2);
        assert_eq!(project.find_folder(&folder_a_id).unwrap().requests.len(), 1);

        // Drag/move r2 onto r1 inside folder Alpha
        assert!(project.move_request_to_target(&r2_id, &r1_id));
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
}
