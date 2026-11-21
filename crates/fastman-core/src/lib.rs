//! Fastman domain, portable local storage, and asynchronous HTTP execution.
//! The core crate has no dependency on GPUI and can be tested headlessly.

pub mod curl;
pub mod http;
pub mod import;
pub mod model;
pub mod security;
pub mod storage;
mod template;

pub use http::{HttpEngine, HttpError, HttpResponse};
pub use model::{ApiRequest, Auth, Collection, Folder, HttpBody, KeyValue, Project};
pub use storage::{ProjectStore, Workspace};
