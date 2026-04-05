//! build_utils — Build 工具鏈共用函式
//!
//! 提供 build-time 環境變數讀取、debug key 取得等功能。
//! 此 crate 僅供 build 工具鏈使用（bytecode_compiler、asset_encryptor 等），
//! **不應**在 game logic 或 runtime crate 中依賴。

pub mod env;
