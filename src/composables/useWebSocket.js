import { ref } from 'vue'
import { api } from '@/api/client'
import { useAppStore } from '@/stores/app'
import { createSingletonAsyncInvoker } from '@/utils/asyncSingleton.mjs'

// Tauri 模式下全局 WS 端点已按架构决策移除（D4）：
// 「连接状态」改为健康检查轮询，业务事件由 Tauri events 下发（3b 起接入）。
const isTauri = typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__

const globalWsState = {
  isConnected: ref(false),
  lastMessage: ref(null),
  error: ref(null)
}
const messageHandlers = new Set()

let globalWs = null
let globalReconnectTimer = null
let globalHeartbeatTimer = null
let healthPollTimer = null
let shouldReconnect = true
const connectGlobalOnce = createSingletonAsyncInvoker(async () => {
  try {
    shouldReconnect = true
    const url = await api.getWebSocketUrl()

    if (globalWs?.readyState === WebSocket.OPEN || globalWs?.readyState === WebSocket.CONNECTING) {
      return globalWs
    }

    globalWs = new WebSocket(url)

    globalWs.onopen = () => {
      const appStore = useAppStore()
      globalWsState.isConnected.value = true
      globalWsState.error.value = null
      appStore.setConnected(true)
      startGlobalHeartbeat()
    }

    globalWs.onclose = () => {
      const appStore = useAppStore()
      globalWsState.isConnected.value = false
      stopGlobalHeartbeat()
      appStore.setConnected(false)
      appStore.setConnectedClients(0)

      if (shouldReconnect) {
        scheduleGlobalReconnect()
      }
    }

    globalWs.onerror = (event) => {
      console.error('[GlobalWebSocket] Error:', event)
      globalWsState.error.value = 'WebSocket connection error'
    }

    globalWs.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data)
        globalWsState.lastMessage.value = data
        applyConnectionState(data)
        messageHandlers.forEach((handler) => handler(data))
      } catch (e) {
        console.error('[GlobalWebSocket] Failed to parse message:', e)
      }
    }

    return globalWs
  } catch (e) {
    console.error('[GlobalWebSocket] Failed to connect:', e)
    globalWsState.error.value = e.message
    scheduleGlobalReconnect()
    throw e
  }
})

function applyConnectionState(data) {
  const appStore = useAppStore()

  if (data.type === 'connected' && data.status) {
    appStore.setConnected(true)
    appStore.setConnectedClients(data.status.connected_clients || 1)
    return
  }

  if (data.type === 'status') {
    appStore.setConnected(true)
    appStore.setConnectedClients(data.connected_clients || 1)
  }
}

function stopGlobalHeartbeat() {
  if (globalHeartbeatTimer) {
    clearInterval(globalHeartbeatTimer)
    globalHeartbeatTimer = null
  }
}

// ---------------- Tauri 模式：健康检查轮询替代 WS ----------------

async function probeBackendHealth() {
  const appStore = useAppStore()
  try {
    await api.getHealth(false)
    globalWsState.isConnected.value = true
    globalWsState.error.value = null
    appStore.setConnected(true)
    appStore.setConnectedClients(1)
  } catch (error) {
    globalWsState.isConnected.value = false
    globalWsState.error.value = error?.message || 'backend unreachable'
    appStore.setConnected(false)
    appStore.setConnectedClients(0)
  }
}

function startTauriHealthLoop() {
  if (healthPollTimer) {
    return
  }
  void probeBackendHealth()
  healthPollTimer = setInterval(() => {
    void probeBackendHealth()
  }, 5000)
}

function stopTauriHealthLoop() {
  if (healthPollTimer) {
    clearInterval(healthPollTimer)
    healthPollTimer = null
  }
}

function sendGlobal(data) {
  if (globalWs?.readyState === WebSocket.OPEN) {
    globalWs.send(JSON.stringify(data))
    return true
  }

  return false
}

function startGlobalHeartbeat() {
  stopGlobalHeartbeat()
  globalHeartbeatTimer = setInterval(() => {
    sendGlobal({ type: 'ping' })
  }, 30000)
}

function scheduleGlobalReconnect() {
  clearTimeout(globalReconnectTimer)
  globalReconnectTimer = setTimeout(() => {
    connectGlobal()
  }, 3000)
}

async function connectGlobal() {
  // Tauri：无 WS 端点，改为健康检查驱动连接状态
  if (isTauri) {
    shouldReconnect = true
    startTauriHealthLoop()
    return null
  }

  if (globalWs?.readyState === WebSocket.OPEN || globalWs?.readyState === WebSocket.CONNECTING) {
    return globalWs
  }

  return connectGlobalOnce()
}

function disconnectGlobal() {
  shouldReconnect = false
  stopGlobalHeartbeat()
  stopTauriHealthLoop()
  clearTimeout(globalReconnectTimer)

  if (globalWs) {
    globalWs.close()
    globalWs = null
  }

  globalWsState.isConnected.value = false
}

function addGlobalMessageHandler(handler) {
  messageHandlers.add(handler)
}

function removeGlobalMessageHandler(handler) {
  messageHandlers.delete(handler)
}

export function useGlobalWebSocket() {
  return {
    ...globalWsState,
    connect: connectGlobal,
    disconnect: disconnectGlobal,
    send: sendGlobal,
    addMessageHandler: addGlobalMessageHandler,
    removeMessageHandler: removeGlobalMessageHandler,
    sendRecordingStatus: (status) => sendGlobal({ type: 'status_update', ...status })
  }
}
