//! Anti-cheat crate — 伺服器端反作弊偵測與 session 管理。

pub mod rules;
pub use rules::{CheatAction, CheatDetector, SessionId};
