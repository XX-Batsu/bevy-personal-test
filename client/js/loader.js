// client/js/loader.js — 入口：載入並協調其他 JS 模組

import { start } from './bootstrap.js';

/**
 * 顯示錯誤畫面：隱藏 spinner/text/canvas，顯示 error-text。
 * @param {string} message
 */
function showError(message) {
  const ids = {
    'error-text': 'block',
    'loading-text': 'none',
    'game-canvas': 'none',
  };
  for (const [id, display] of Object.entries(ids)) {
    const el = document.getElementById(id);
    if (el) {
      el.style.display = display;
      if (id === 'error-text') el.textContent = message;
    }
  }
  const spinner = document.querySelector('.spinner');
  if (spinner) spinner.style.display = 'none';
}

// 掛載至 window.__game 供 WASM js_show_error() callback 呼叫
window.__game = window.__game || {};
window.__game.js_show_error = showError;

// 啟動應用，失敗時顯示錯誤畫面
start().catch((err) => showError(`無法啟動遊戲：${err.message || err}`));
