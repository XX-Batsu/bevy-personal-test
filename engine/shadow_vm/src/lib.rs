//! Shadow VM crate — 提供 Web Worker 端重播驗證與主線程端通訊。
//!
//! ## cfg gate 使用規範
//!
//! WASM-only 模組（worker.rs、client.rs）須以 `#[cfg(target_arch = "wasm32")]`
//! 包裹，避免 native build 拉入 wasm-bindgen/web-sys 依賴。
//!
//! 全平台模組（native + WASM 均可用）：
//! `error`、`executor`、`validator`、`scheduler`、`trace_collector`、`in_process`

pub mod error;
pub mod executor;
pub mod in_process;
pub mod scheduler;
pub mod trace_collector;
pub mod validator;

#[cfg(test)]
pub mod test_helpers;

#[cfg(target_arch = "wasm32")]
pub mod client;
#[cfg(target_arch = "wasm32")]
pub mod worker;

pub use error::ShadowVmError;
pub use executor::{ShadowExecutor, ShadowScript};
pub use in_process::InProcessShadowVm;
pub use scheduler::SamplingScheduler;
pub use trace_collector::TraceCollector;
pub use validator::ShadowValidator;

#[cfg(target_arch = "wasm32")]
pub use client::ShadowVmClient;
