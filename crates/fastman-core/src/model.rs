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
        self.requests
            .iter()
            .find(|r| r.id == request_id)
            .or_else(|| self.folders.iter().find_map(|f| f.find_request(request_id)))
    }

    pub fn find_request_mut(&mut self, request_id: &str) -> Option<&mut ApiRequest> {
        if let Some(index) = self.requests.iter().position(|r| r.id == request_id) {
            return self.requests.get_mut(index);
        }
        self.folders
            .iter_mut()
            .find_map(|f| f.find_request_mut(request_id))
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
        self.requests
            .iter()
            .find(|r| r.id == request_id)
            .or_else(|| self.folders.iter().find_map(|f| f.find_request(request_id)))
    }

    pub fn find_request_mut(&mut self, request_id: &str) -> Option<&mut ApiRequest> {
        if let Some(index) = self.requests.iter().position(|r| r.id == request_id) {
            return self.requests.get_mut(index);
        }
        self.folders
            .iter_mut()
            .find_map(|f| f.find_request_mut(request_id))
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
}
