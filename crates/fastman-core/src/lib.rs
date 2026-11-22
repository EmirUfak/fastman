//! Fastman domain, portable local storage, and asynchronous HTTP execution.
//! The core crate has no dependency on GPUI and can be tested headlessly.

pub mod curl;
pub mod http;
pub mod import;
pub mod model;
pub mod secrets;
pub mod security;
pub mod storage;
pub mod template;

pub use http::{HttpEngine, HttpError, HttpResponse};
pub use model::{
    ApiRequest, Auth, Collection, Environment, Folder, HttpBody, KeyValue, Project, SecretRef,
};
pub use secrets::{
    MemorySecretStore, OsSecretStore, ResolvedVariables, SecretChangePlan, SecretStore,
    SecretStoreError,
};
pub use storage::{ProjectStore, Workspace};
