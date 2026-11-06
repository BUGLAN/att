import { ref } from 'vue'
import { api } from '@/api/client'
import { useAppStore } from '@/stores/app'

// 后端（内嵌数据面）连接状态：每 5 秒探一次 /api/health。
// 业务事件走 Tauri events（见 useElectron），这里只回答「后端还在不在」，
// 结果写进 appStore.isConnected，供侧边栏与托盘图标（useTrayIcon）展示断连。

const HEALTH_POLL_INTERVAL_MS = 5000

const isConnected = ref(false)
const error = ref(null)
let healthPollTimer = null

async function probeBackendHealth() {
  const appStore = useAppStore()
  try {
    await api.getHealth(false)
    isConnected.value = true
    error.value = null
    appStore.setConnected(true)
    appStore.setConnectedClients(1)
  } catch (caught) {
    isConnected.value = false
    error.value = caught?.message || 'backend unreachable'
    appStore.setConnected(false)
    appStore.setConnectedClients(0)
  }
}

function connect() {
  if (healthPollTimer) {
    return
  }
  void probeBackendHealth()
  healthPollTimer = setInterval(() => {
    void probeBackendHealth()
  }, HEALTH_POLL_INTERVAL_MS)
}

function disconnect() {
  if (healthPollTimer) {
    clearInterval(healthPollTimer)
    healthPollTimer = null
  }
  isConnected.value = false
}

export function useBackendConnection() {
  return { isConnected, error, connect, disconnect }
}
