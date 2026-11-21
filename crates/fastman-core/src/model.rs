use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;

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
    if let Some(index) = folders.iter().position(|f| f.id == folder_id) {
        folders.remove(index);
        return true;
    }
    folders.iter_mut().any(|f| f.delete_folder(folder_id))
}

fn delete_request_in(
    requests: &mut Vec<ApiRequest>,
    folders: &mut [Folder],
    request_id: &str,
) -> bool {
    if let Some(index) = requests.iter().position(|r| r.id == request_id) {
        requests.remove(index);
        return true;
    }
    folders.iter_mut().any(|f| f.delete_request(request_id))
}

fn duplicate_request_in(
    requests: &mut Vec<ApiRequest>,
    folders: &mut [Folder],
    request_id: &str,
) -> Option<String> {
    if let Some(index) = requests.iter().position(|r| r.id == request_id) {
        let mut copy = requests[index].clone();
        copy.id = new_id();
        copy.name = format!("{} (copy)", copy.name);
        let new_request_id = copy.id.clone();
        requests.insert(index + 1, copy);
        return Some(new_request_id);
    }
    folders
        .iter_mut()
        .find_map(|f| f.duplicate_request(request_id))
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

    pub fn delete_request(&mut self, request_id: &str) -> bool {
        self.collections
            .iter_mut()
            .any(|c| c.delete_request(request_id))
    }

    pub fn duplicate_request(&mut self, request_id: &str) -> Option<String> {
        self.collections
            .iter_mut()
            .find_map(|c| c.duplicate_request(request_id))
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
}
