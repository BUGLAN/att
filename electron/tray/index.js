/**
 * 托盘管理器
 * 管理系统托盘图标、右键菜单
 */

const { Tray, Menu, nativeImage, screen } = require('electron')
const fs = require('fs')
const path = require('path')
const EventEmitter = require('events')

/**
 * 托盘状态 → 图标文件名。state 字符串由渲染层经 IPC 传入
 * （见 src/composables/useRecorder.js 的 tray.setIcon 调用），必须与文件一一对应。
 */
const TRAY_ICON_STATES = ['idle', 'recording', 'transcribing', 'disconnected']

/** Windows 托盘逻辑尺寸（DIP）。各 DPI 档位的像素尺寸都是它的整数倍。 */
const TRAY_BASE_SIZE = 16

/**
 * Windows 托盘各 DPI 档位：[像素尺寸, 文件名后缀]，按尺寸升序。
 * 与 scripts/generate_app_icon.py 的 WIN_TRAY_SIZES 必须一一对应。
 */
const WIN_TRAY_LADDER = [
  [16, ''],
  [20, '@1.25x'],
  [24, '@1.5x'],
  [28, '@1.75x'],
  [32, '@2x'],
  [40, '@2.5x'],
  [48, '@3x']
]

/**
 * 多倍图目录：Windows 托盘以 16px 为基准，macOS/Linux 菜单栏以 22px 为基准。
 */
function trayIconPlatformDir() {
  return process.platform === 'win32' ? 'win' : 'macos'
}

/** 把状态字符串收敛到已知状态，未知一律按 idle。 */
function normalizeTrayIconState(iconType) {
  return TRAY_ICON_STATES.includes(iconType) ? iconType : 'idle'
}

/** 像素尺寸 → 文件名后缀。 */
function trayIconSuffixForSize(size) {
  const entry = WIN_TRAY_LADDER.find(([px]) => px === size)
  return entry ? entry[1] : ''
}

/**
 * 当前主显示器的缩放系数；读不到（如单测环境）返回 null。
 *
 * 托盘挂在主屏任务栏上，所以取主屏的 scaleFactor 就够。
 */
function currentDisplayScaleFactor() {
  try {
    const factor = screen?.getPrimaryDisplay?.()?.scaleFactor
    return Number.isFinite(factor) && factor > 0 ? factor : null
  } catch (error) {
    return null
  }
}

/**
 * 按缩放系数挑一个"够用"的图标像素尺寸：不小于需要的物理像素的最小档位。
 *
 * 为什么不是直接给 16px 让系统自己缩放：Windows 托盘图标是按物理像素画的，
 * 200% 缩放时槽位是 32px，给 16px 位图就会被放大成糊块（用用户截图与各档位
 * 做 MSE 比对确认过：截图最接近"16px 双线性放大到 32px"）。反过来，给足像素
 * 让系统去缩小，边缘和笔画都保得住。
 *
 * 读不到缩放系数时给最大档：宁可让系统缩小，也不要放大。
 */
function pickWindowsTraySize(scaleFactor) {
  const largest = WIN_TRAY_LADDER[WIN_TRAY_LADDER.length - 1][0]

  if (!Number.isFinite(scaleFactor) || scaleFactor <= 0) {
    return largest
  }

  const needed = Math.max(TRAY_BASE_SIZE, Math.round(TRAY_BASE_SIZE * scaleFactor))
  const match = WIN_TRAY_LADDER.find(([px]) => px >= needed)
  return match ? match[0] : largest
}

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
   *
   * 直接读 __dirname 下的资源：开发态和打包态（app.asar 内）Electron 都能读，
   * 不要再走 process.resourcesPath —— electron-builder 从未把图标解包到那里，
   * 那条路径在打包版里必然落空。
   *
   * Windows 走 DPI 分档（见 pickWindowsTraySize）；macOS 由 createFromPath
   * 自己处理同目录的 @2x 约定。
   */
  getIconPath(iconType, scaleFactor = currentDisplayScaleFactor()) {
    const state = normalizeTrayIconState(iconType)
    const dir = path.join(__dirname, 'icons', trayIconPlatformDir())

    if (process.platform !== 'win32') {
      return path.join(dir, `${state}.png`)
    }

    const suffix = trayIconSuffixForSize(pickWindowsTraySize(scaleFactor))
    return path.join(dir, `${state}${suffix}.png`)
  }

  /**
   * 创建托盘图标
   */
  createTrayIcon(iconType) {
    const iconPath = this.getIconPath(iconType)

    if (fs.existsSync(iconPath)) {
      return nativeImage.createFromPath(iconPath)
    }

    // 这里刻意不再手绘兜底图标：历史实现用裸 RGBA 缓冲喂 createFromBuffer，
    // 被 Electron 当成 BGRA 解析后 R/B 互换，画出来是个青蓝色小球。
    // 缺图就退回应用图标并留下日志，方便一眼看出是资产没生成。
    console.error(`[Tray] 缺少托盘图标：${iconPath}，已回退应用图标`)
    return nativeImage.createFromPath(path.join(__dirname, '..', 'icons', 'icon.png'))
  }

  /**
   * 创建托盘
   */
  create() {
    const icon = this.createTrayIcon('idle')
    this.tray = new Tray(icon)
    this.tray.setToolTip('Audio to Text')

    // 显示器缩放变化（改 DPI、换屏）后按新档位重新取图，
    // 否则图标会停在旧档位上，又被系统放大成糊图
    this.handleDisplayMetricsChanged = () => this.setIcon(this.currentIcon)
    try {
      screen?.on?.('display-metrics-changed', this.handleDisplayMetricsChanged)
    } catch (error) {
      // 单测 / 无 GUI 环境拿不到 screen，忽略即可
    }

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
    if (this.handleDisplayMetricsChanged) {
      try {
        screen?.removeListener?.('display-metrics-changed', this.handleDisplayMetricsChanged)
      } catch (error) {
        // 同上：无 GUI 环境忽略
      }
      this.handleDisplayMetricsChanged = null
    }

    if (this.tray) {
      this.tray.destroy()
      this.tray = null
    }
  }
}

module.exports = {
  TrayManager,
  // 纯函数导出给单测用（不依赖 Electron 运行时）
  TRAY_ICON_STATES,
  WIN_TRAY_LADDER,
  normalizeTrayIconState,
  pickWindowsTraySize
}
