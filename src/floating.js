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
    // 结果态停留到点后收起：Rust 侧只收已结束的会话，晚到的请求不会把新一次录音藏掉
    close: () => import('@tauri-apps/api/core').then(({ invoke }) => invoke('hide_floating_window')),
    openSettings: () => import('@tauri-apps/api/core').then(({ invoke }) => invoke('open_main_settings')),
    // 失败态点胶囊：唤起主窗口看完整原因与记录
    openMain: () => import('@tauri-apps/api/core').then(({ invoke }) => invoke('open_main_window')),
    // 胶囊上的 ✕：录音中取消录音，识别 / 优化中取消处理（已输入的字保留）
    cancel: () => import('@tauri-apps/api/core').then(({ invoke }) => invoke('recording_cancel')),
    // 录音音量 0..1（录音 Actor 约 25Hz 广播），驱动声纹
    onLevel: async (callback) => {
      const { listen } = await import('@tauri-apps/api/event')
      unlisteners.push(await listen('recording:level', (event) => callback(Number(event.payload?.level) || 0)))
    },
    // 胶囊区域 [x, y, 宽, 高]（CSS px）；null = 不可见。Rust 据此让透明部分点击穿透
    setHitRect: (rect) => import('@tauri-apps/api/core').then(({ invoke }) => invoke('floating_hit_rect', { rect })),
    // 光标移出胶囊、窗口切到点击穿透：页面收不到 mouseleave，由 Rust 补发
    onPointerOut: async (callback) => {
      const { listen } = await import('@tauri-apps/api/event')
      unlisteners.push(await listen('floating:pointer-out', () => callback()))
    },
    // 这次按键没生效（收尾 / 优化中）：印章摇一下
    onNudge: async (callback) => {
      const { listen } = await import('@tauri-apps/api/event')
      unlisteners.push(await listen('floating:nudge', () => callback()))
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
