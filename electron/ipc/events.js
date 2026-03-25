/**
 * IPC 事件发射器
 * 处理状态轮询和事件通知
 */

const { ipcMain } = require('electron')

class EventEmitter {
  constructor(pythonManager, mainWindow) {
    this.pythonManager = pythonManager
    this.mainWindow = mainWindow
    this.pollingIntervals = new Map()
  }

  get baseUrl() {
    return `http://127.0.0.1:${this.pythonManager.port}/api`
  }

  /**
   * 设置 IPC 处理器
   */
  setupIPC() {
    // 开始轮询
    ipcMain.on('polling:start', (event, recordId) => {
      this.startPolling(recordId)
    })

    // 停止轮询
    ipcMain.on('polling:stop', () => {
      this.stopAllPolling()
    })
  }

  /**
   * 开始轮询记录状态
   */
  startPolling(recordId) {
    console.log('[EventEmitter] Starting polling for:', recordId)
    console.log('[EventEmitter] mainWindow exists:', !!this.mainWindow)

    // 如果已在轮询，先停止
    if (this.pollingIntervals.has(recordId)) {
      clearInterval(this.pollingIntervals.get(recordId))
    }

    const interval = setInterval(async () => {
      try {
        const response = await fetch(`${this.baseUrl}/records/${recordId}/status`)
        const status = await response.json()

        console.log('[EventEmitter] Polled status:', status.status, 'record_id:', status.record_id)

        // 发送状态到渲染进程
        if (this.mainWindow) {
          this.mainWindow.webContents.send('record:status', status)
          console.log('[EventEmitter] Sent record:status event')
        } else {
          console.error('[EventEmitter] mainWindow is null, cannot send event')
        }

        // 检查是否完成
        if (status.status === 'transcribed' || status.status === 'optimized') {
          this.stopPolling(recordId)

          // 发送完成事件
          if (this.mainWindow) {
            this.mainWindow.webContents.send('record:completed', status)
            console.log('[EventEmitter] Sent record:completed event')
          }
        }

        // 检查是否失败
        if (status.status === 'failed') {
          this.stopPolling(recordId)
        }
      } catch (error) {
        console.error('[EventEmitter] Polling error:', error)
      }
    }, 1000)

    this.pollingIntervals.set(recordId, interval)
  }

  /**
   * 停止轮询特定记录
   */
  stopPolling(recordId) {
    if (this.pollingIntervals.has(recordId)) {
      clearInterval(this.pollingIntervals.get(recordId))
      this.pollingIntervals.delete(recordId)
    }
  }

  /**
   * 停止所有轮询
   */
  stopAllPolling() {
    for (const interval of this.pollingIntervals.values()) {
      clearInterval(interval)
    }
    this.pollingIntervals.clear()
  }
}

module.exports = { EventEmitter }
