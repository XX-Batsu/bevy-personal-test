/* tslint:disable */
/* eslint-disable */

/**
 * 接收主線程發送的 ShadowInit（bincode），初始化 ShadowValidator。
 * 對應 worker.rs §handle_init 完整實作。
 */
export function handle_init(data: Uint8Array): void;

/**
 * 接收主線程發送的 ShadowRequest（bincode），執行 hash 驗證並回傳 ShadowResponse。
 * 對應 worker.rs §handle_message 完整實作。
 */
export function handle_message(data: Uint8Array): void;

/**
 * Shadow Worker 啟動初始化，由 shadow_worker.js 在 WASM init() 後呼叫。
 * debug-mode 下設定 console_error_panic_hook，確保 Rust panic 可在瀏覽器 console 顯示。
 */
export function shadow_worker_setup(): void;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly handle_init: (a: number, b: number) => void;
    readonly handle_message: (a: number, b: number) => void;
    readonly shadow_worker_setup: () => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
