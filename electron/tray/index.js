/**
 * 托盘管理器
 * 管理系统托盘图标、右键菜单
 */

const { Tray, Menu, nativeImage, app } = require('electron')
const path = require('path')
const EventEmitter = require('events')

class TrayManager extends EventEmitter {
  constructor(mainWindow, apiProxy) {
    super()
    this.mainWindow = mainWindow
    this.apiProxy = apiProxy
    this.tray = null
    this.currentIcon = 'idle'
    this.records = []
  }

  /**
   * 获取图标路径
   */
  getIconPath(iconType) {
    const iconName = {
      idle: 'idle.png',
      recording: 'recording.png',
      transcribing: 'transcribing.png',
      disconnected: 'disconnected.png'
    }[iconType] || 'idle.png'

    const isDev = process.env.NODE_ENV === 'development'
    const basePath = isDev
      ? path.join(__dirname, 'icons')
      : path.join(process.resourcesPath, 'icons')

    return path.join(basePath, iconName)
  }

  /**
   * 创建托盘图标
   */
  createTrayIcon(iconType) {
    const iconPath = this.getIconPath(iconType)

    // 如果图标文件不存在，创建默认图标
    if (!require('fs').existsSync(iconPath)) {
      return this.createDefaultIcon(iconType)
    }

    return nativeImage.createFromPath(iconPath)
  }

  /**
   * 创建默认图标（如果没有图标文件）
   */
  createDefaultIcon(iconType) {
    const size = 22
    const canvas = Buffer.alloc(size * size * 4)

    const colors = {
      idle: [201, 162, 39, 255],
      recording: [230, 126, 34, 255],
      transcribing: [201, 162, 39, 255],
      disconnected: [113, 113, 122, 180]
    }

    const color = colors[iconType] || colors.idle
    const center = (size - 1) / 2
    const radius = 8

    for (let y = 0; y < size; y++) {
      for (let x = 0; x < size; x++) {
        const idx = (y * size + x) * 4
        const distance = Math.hypot(x - center, y - center)

        if (distance > radius + 0.75) {
          continue
        }

        const alphaScale = distance > radius
          ? Math.max(0, 1 - (distance - radius) / 0.75)
          : 1

        canvas[idx] = color[0]
        canvas[idx + 1] = color[1]
        canvas[idx + 2] = color[2]
        canvas[idx + 3] = Math.round(color[3] * alphaScale)
      }
    }

    return nativeImage.createFromBuffer(canvas, { width: size, height: size })
  }

  /**
   * 创建托盘
   */
  create() {
    const icon = this.createTrayIcon('idle')
    this.tray = new Tray(icon)
    this.tray.setToolTip('Audio to Text')

    this.updateMenu()
    this.setupEvents()
  }

  /**
   * 设置托盘事件
   */
  setupEvents() {
    // 点击托盘图标显示窗口
    this.tray.on('click', () => {
      if (this.mainWindow) {
        if (this.mainWindow.isMinimized()) {
          this.mainWindow.restore()
        }
        this.mainWindow.show()
        this.mainWindow.focus()
      }
    })

    // 双击托盘图标
    this.tray.on('double-click', () => {
      if (this.mainWindow) {
        this.mainWindow.show()
        this.mainWindow.focus()
      }
    })
  }

  /**
   * 更新右键菜单
   */
  updateMenu(records = []) {
    this.records = records
    const menuItems = []

    // 历史记录直接显示在主菜单（时间倒序）
    const recentRecords = records.slice(0, 10)
    if (recentRecords.length > 0) {
      recentRecords.forEach((record) => {
        menuItems.push({
          label: this.formatRecordLabel(record),
          click: () => {
            const text = record.optimized_text || record.raw_text
            if (text) {
              this.emit('copy-and-paste', text)
            }
          }
        })
      })
      menuItems.push({ type: 'separator' })
    }

    // 设置
    menuItems.push({
      label: '设置',
      click: () => {
        if (this.mainWindow) {
          this.mainWindow.show()
          this.mainWindow.webContents.send('navigate:settings')
        }
      }
    })

    menuItems.push({ type: 'separator' })

    // 退出
    menuItems.push({
      label: '退出',
      click: () => {
        this.emit('quit')
      }
    })

    this.tray.setContextMenu(Menu.buildFromTemplate(menuItems))
  }

  /**
   * 格式化记录标签
   */
  formatRecordLabel(record) {
    const text = record.optimized_text || record.raw_text || '空记录'
    const time = this.formatTime(record.created_at)
    return `${time} - ${this.truncateText(text, 40)}`
  }

  /**
   * 格式化时间
   */
  formatTime(timestamp) {
    if (!timestamp) return ''

    const date = new Date(timestamp)
    const now = new Date()
    const isToday = date.toDateString() === now.toDateString()

    if (isToday) {
      return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
    }

    return date.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' })
  }

  /**
   * 截断文本
   */
  truncateText(text, maxLength) {
    if (!text) return ''
    if (text.length <= maxLength) return text
    return text.substring(0, maxLength) + '...'
  }

  /**
   * 设置托盘图标
   */
  setIcon(iconType) {
    if (!this.tray) return

    this.currentIcon = iconType
    const icon = this.createTrayIcon(iconType)
    this.tray.setImage(icon)

    // 更新提示文字
    const tooltips = {
      idle: 'Audio to Text - 就绪',
      recording: 'Audio to Text - 录音中...',
      transcribing: 'Audio to Text - 转写中...',
      disconnected: 'Audio to Text - 未连接'
    }

    this.tray.setToolTip(tooltips[iconType] || tooltips.idle)
  }

  /**
   * 销毁托盘
   */
  destroy() {
    if (this.tray) {
      this.tray.destroy()
      this.tray = null
    }
  }
}

module.exports = { TrayManager }
