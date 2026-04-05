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
 * 建立並啟動 Bevy 歡迎畫面 App。
 * 只能呼叫一次（由 js_start_game_loop() callback 觸發）；重複呼叫靜默忽略。
 * WASM 環境：WinitPlugin 接管 rAF，此函式非阻塞返回。
 */
export function wasm_start_game(): void;

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
    readonly wasm_start_game: () => void;
    readonly wasm_tick: (a: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h982eed14cf430e42: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h43d8cbcd7620786a: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hbf11878112de8321: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h08d848fcabd95ebf: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__ha3b2e3e6975b995a: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h083ae3e5125011c8: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h65b1dcdf538474dd: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h4005bc30c83f3d30: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h40c69f3b057a1916: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h5e786c18403bebe7: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hb9a54380b6de4600: (a: number, b: number) => number;
    readonly wasm_bindgen__convert__closures_____invoke__h05feebdc2d11e6bf: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h5c982496db70eb3e: (a: number, b: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_destroy_closure: (a: number, b: number) => void;
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
