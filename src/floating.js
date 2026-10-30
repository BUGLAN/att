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
    // 胶囊上的 ✕：录音中取消录音，识别 / 优化中取消处理（已输入的字保留）
    cancel: () => import('@tauri-apps/api/core').then(({ invoke }) => invoke('recording_cancel')),
    // 录音音量 0..1（录音 Actor 约 25Hz 广播），驱动声纹
    onLevel: async (callback) => {
      const { listen } = await import('@tauri-apps/api/event')
      unlisteners.push(await listen('recording:level', (event) => callback(Number(event.payload?.level) || 0)))
    },
    onStateChange: async (callback) => {
      // 先订阅再补拉，且补拉结果只在尚未收到推送时生效：
      // 否则补拉的旧快照晚于新推送返回时，会把悬浮窗覆盖回过期状态
      let receivedPush = false
      const [{ listen }, { invoke }] = await Promise.all([
        import('@tauri-apps/api/event'),
        import('@tauri-apps/api/core')
      ])
      unlisteners.push(await listen('floating:state', (event) => {
        receivedPush = true
        callback(event.payload)
      }))
      try {
        const snapshot = await invoke('get_floating_state')
        if (!receivedPush) {
          callback(snapshot)
        }
      } catch (error) {
        console.warn('[Floating] Failed to read floating state:', error)
      }
    },
    removeAllListeners: () => {
      while (unlisteners.length) unlisteners.pop()()
    }
  }
}

createApp(FloatingWindow).mount('#app')
