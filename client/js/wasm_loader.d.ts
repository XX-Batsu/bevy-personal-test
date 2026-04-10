/* tslint:disable */
/* eslint-disable */

/**
 * 將 client X25519 公鑰封裝為標準 codec wire frame（供 bootstrap.js 呼叫）。
 *
 * `public_key` 必須恰好 32 bytes，否則回傳 Err(JsValue)。
 * 輸出格式：`[4B LE len] | [bincode(NetMessage::ClientHello { public_key })]`
 */
export function wasm_encode_client_hello(public_key: Uint8Array): Uint8Array;

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
 * 加密並送出一則遊戲訊息（整合測試輔助用途）。
 *
 * `msg_bytes` = `bincode::serialize(&NetMessage)` 的結果（已序列化 bytes）。
 * 內部：deserialize → encrypt_outgoing → js_send_websocket。
 *
 * 注意：Bevy system 送訊息應直接呼叫 `handshake::encrypt_outgoing()` 再
 * `js_send_websocket()`，避免跨 WASM 邊界的額外序列化開銷。
 * 此 export 主要供整合測試使用；Phase 15 補齊完整 Bevy 分派整合。
 */
export function wasm_send_message(msg_bytes: Uint8Array): void;

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
    readonly wasm_encode_client_hello: (a: number, b: number) => [number, number, number, number];
    readonly wasm_init: () => [number, number, number, number];
    readonly wasm_on_handshake_timeout: () => void;
    readonly wasm_on_shadow_result: (a: number, b: number) => void;
    readonly wasm_on_websocket_message: (a: number, b: number) => void;
    readonly wasm_send_message: (a: number, b: number) => [number, number];
    readonly wasm_start_game: () => void;
    readonly wasm_tick: (a: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h982eed14cf430e42: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h45409842cfc42ca0: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h14fb53fd931fe28c: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h0303316257e55131: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__ha1ca2ebc91a73f21: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h283b026338f9b503: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h33f004939760aea9: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h93c781489a5e80a5: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h2afbaf96985aab62: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__ha29bc205405ba151: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hb9a54380b6de4600: (a: number, b: number) => number;
    readonly wasm_bindgen__convert__closures_____invoke__hc3996ebc4deeaaa0: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h6999338c3bcd4b83: (a: number, b: number) => void;
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
