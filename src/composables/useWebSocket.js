import { onMounted, onUnmounted, ref } from 'vue'
import { api } from '@/api/client'
import { useAppStore } from '@/stores/app'
import { createSingletonAsyncInvoker } from '@/utils/asyncSingleton.mjs'

const globalWsState = {
  isConnected: ref(false),
  lastMessage: ref(null),
  error: ref(null)
}
const messageHandlers = new Set()

let globalWs = null
let globalReconnectTimer = null
let globalHeartbeatTimer = null
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
  if (globalWs?.readyState === WebSocket.OPEN || globalWs?.readyState === WebSocket.CONNECTING) {
    return globalWs
  }

  return connectGlobalOnce()
}

function disconnectGlobal() {
  shouldReconnect = false
  stopGlobalHeartbeat()
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

export function useWebSocket() {
  const ws = useGlobalWebSocket()

  onMounted(() => {
    ws.connect()
  })

  onUnmounted(() => {
    if (messageHandlers.size === 0) {
      ws.disconnect()
    }
  })

  return ws
}
