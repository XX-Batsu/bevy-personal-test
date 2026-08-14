// client/js/config.js — 執行期組態

/**
 * 離線 demo 模式。
 *
 * true 時 bootstrap 跳過 WebSocket 連線與 ECDH 握手，直接啟動 Bevy App，
 * 讓 client 能在沒有 gateway server 的靜態託管環境（GitHub Pages）執行。
 * 僅供展示用途——正式建置一律維持 false，由 .github/workflows/pages.yml
 * 在 demo 部署時覆寫此檔。
 *
 * 亦可在網址加上 `?offline` 於本機臨時啟用。
 */
export const OFFLINE_DEMO = false;
