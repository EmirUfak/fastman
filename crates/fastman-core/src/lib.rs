//! Fastman domain, portable local storage, and asynchronous HTTP execution.
//! The core crate has no dependency on GPUI and can be tested headlessly.

pub mod assertions;
pub mod captures;
pub mod chain_runner;
pub mod chain_runtime;
pub mod curl;
pub mod http;
pub mod import;
pub mod model;
pub mod secrets;
pub mod security;
pub mod storage;
pub mod template;

pub use assertions::{AssertionError, AssertionFailure, evaluate_assertions};
pub use captures::{CaptureError, CaptureFailure, CapturedValue, capture_step_response};
pub use chain_runner::{
    ChainExecutionError, ChainFailure, ChainResponsePreview, ChainRunOptions, ChainStepOutcome,
    ChainStepReport, run_chain, run_chain_with_progress, run_chain_with_report,
    validate_chain_inputs,
};
pub use chain_runtime::{ChainRunContext, ChainRunError};
pub use http::{HttpEngine, HttpError, HttpResponse};
pub use model::{
    ApiRequest, AssertionRule, Auth, CaptureRule, CaptureSource, ChainInput, ChainInputTarget,
    ChainStep, Collection, Environment, Folder, HttpBody, KeyValue, Project, RequestChain,
    SecretRef,
};
pub use secrets::{
    MemorySecretStore, OsSecretStore, ResolvedVariables, SecretChangePlan, SecretStore,
    SecretStoreError,
};
pub use storage::{ProjectStore, Workspace};
