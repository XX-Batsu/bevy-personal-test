//! Server 端權威模擬引擎（每 session 一個獨立 instance）。
//! Native only——不需 WASM 相容。

pub mod authoritative;

pub use authoritative::AuthoritativeSimulation;
