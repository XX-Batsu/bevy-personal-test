/* tslint:disable */
/* eslint-disable */

/**
 * 初始化 WASM 模組：panic hook → tracing → ECDH keygen。
 * 回傳 client X25519 公鑰（32 bytes）。
 */
export function wasm_init(): Uint8Array;

/**
 * 握手 timeout callback（由 JS setTimeout 5s 後呼叫）。
 */
export function wasm_on_handshake_timeout(): void;

/**
 * 轉發 Shadow VM 結果至 WASM。
 */
export function wasm_on_shadow_result(data: Uint8Array): void;

/**
 * WebSocket 訊息接收。
 * 握手中→解析 EcdhServerResponse；遊戲中→netcode 佇列。
 */
export function wasm_on_websocket_message(data: Uint8Array): void;

/**
 * 推進一個 game frame（由 JS requestAnimationFrame 呼叫）。
 */
export function wasm_tick(timestamp: number): void;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly wasm_init: () => [number, number, number, number];
    readonly wasm_on_handshake_timeout: () => void;
    readonly wasm_on_shadow_result: (a: number, b: number) => void;
    readonly wasm_on_websocket_message: (a: number, b: number) => void;
    readonly wasm_tick: (a: number) => void;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_dealloc: (a: number) => void;
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
