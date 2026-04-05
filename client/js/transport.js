// client/js/transport.js — WebSocket 通訊層（純傳輸，無加密邏輯）

let ws = null;
let messageCallback = null;
let pendingQueue = []; // WebSocket 未 OPEN 時暫存的待送訊息

function getWsUrl() {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${protocol}//${window.location.host}/ws`;
}

/**
 * 建立 WebSocket 連線。
 * URL 從 window.location 推導：ws(s)://<host>/ws
 * @returns {Promise<WebSocket>} resolve 於 onopen
 */
export function connect() {
  return new Promise((resolve, reject) => {
    ws = new WebSocket(getWsUrl());
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => {
      // flush pending queue（解決 sendBinary 競速問題）
      for (const data of pendingQueue) ws.send(data);
      pendingQueue = [];
      resolve(ws);
    };
    ws.onmessage = (event) => {
      if (messageCallback && event.data instanceof ArrayBuffer) {
        messageCallback(new Uint8Array(event.data));
      }
    };
    ws.onerror = (err) => reject(err);
    ws.onclose = () => console.log('[transport] WebSocket 已關閉');
  });
}

/**
 * 傳送 binary 資料（ArrayBuffer）至 server。
 * 若 WebSocket 尚未 OPEN，訊息暫存於內部佇列。
 * @param {ArrayBuffer} data
 */
export function sendBinary(data) {
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(data);
  } else {
    pendingQueue.push(data);
  }
}

/**
 * 設定接收 server 訊息的回呼。
 * @param {function(ArrayBuffer): void} callback
 */
export function onMessage(callback) {
  messageCallback = callback;
}
