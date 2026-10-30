// 设计令牌必须先于任何组件/窗口样式引入
import './styles/tokens.css'
import { createApp } from 'vue'
import FloatingWindow from './components/floating/FloatingWindow.vue'
import './styles/floating.css'

// Tauri 下垫片实现旧 Electron preload 暴露的 window.floatingAPI（组件零改动）：
// 状态由 Rust 录音 Actor 推送（floating:state），挂载时补拉一次最近状态
if (window.__TAURI_INTERNALS__ && !window.floatingAPI) {
  const unlisteners = []
  window.floatingAPI = {
    close: () => import('@tauri-apps/api/core').then(({ invoke }) => invoke('hide_floating_window')),
    openSettings: () => import('@tauri-apps/api/core').then(({ invoke }) => invoke('open_main_settings')),
    onStateChange: (callback) => {
      import('@tauri-apps/api/event').then(async ({ listen }) => {
        unlisteners.push(await listen('floating:state', (event) => callback(event.payload)))
      })
      import('@tauri-apps/api/core').then(async ({ invoke }) => callback(await invoke('get_floating_state')))
    },
    removeAllListeners: () => {
      while (unlisteners.length) unlisteners.pop()()
    }
  }
}

createApp(FloatingWindow).mount('#app')
