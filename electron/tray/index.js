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
    // 创建一个简单的 22x22 像素图标
    const size = 22
    const canvas = Buffer.alloc(size * size * 4)

    // 根据类型设置颜色
    const colors = {
      idle: [128, 128, 128, 255],      // 灰色
      recording: [248, 113, 113, 255], // 红色
      transcribing: [201, 162, 39, 255], // 金色
      disconnected: [113, 113, 122, 128] // 半透明灰色
    }

    const color = colors[iconType] || colors.idle

    // 填充像素
    for (let i = 0; i < size * size; i++) {
      const idx = i * 4
      canvas[idx] = color[0]     // R
      canvas[idx + 1] = color[1] // G
      canvas[idx + 2] = color[2] // B
      canvas[idx + 3] = color[3] // A
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

    const contextMenu = Menu.buildFromTemplate([
      {
        label: '显示窗口',
        click: () => {
          if (this.mainWindow) {
            this.mainWindow.show()
            this.mainWindow.focus()
          }
        }
      },
      { type: 'separator' },
      {
        label: '最近记录',
        enabled: records.length > 0,
        submenu: this.buildRecordsMenu(records)
      },
      { type: 'separator' },
      {
        label: '开始录音',
        accelerator: process.platform === 'darwin' ? 'Alt+Cmd' : 'Alt+Ctrl',
        click: () => {
          this.mainWindow?.webContents.send('hotkey:start')
          this.setIcon('recording')
        }
      },
      { type: 'separator' },
      {
        label: '设置',
        click: () => {
          if (this.mainWindow) {
            this.mainWindow.show()
            this.mainWindow.webContents.send('navigate:settings')
          }
        }
      },
      { type: 'separator' },
      {
        label: '退出',
        click: () => {
          this.emit('quit')
        }
      }
    ])

    this.tray.setContextMenu(contextMenu)
  }

  /**
   * 构建最近记录菜单
   */
  buildRecordsMenu(records) {
    if (records.length === 0) {
      return Menu.buildFromTemplate([
        { label: '暂无记录', enabled: false }
      ])
    }

    // 只显示最近 5 条记录
    const recentRecords = records.slice(0, 5)

    return Menu.buildFromTemplate(recentRecords.map(record => ({
      label: this.truncateText(record.raw_text || record.optimized_text || '空记录', 30),
      click: () => {
        // 复制文本并粘贴
        const text = record.optimized_text || record.raw_text
        if (text) {
          this.emit('copy-and-paste', text)
        }
      }
    })))
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
