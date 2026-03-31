const { clipboard } = require('electron')
const crypto = require('crypto')
const fs = require('fs')
const path = require('path')
const { logElectronOverlay } = require('../logger')

const POLL_INTERVAL_MS = 800
const INTERNAL_WRITE_TTL_MS = 5000

class ClipboardMonitor {
  constructor({ getBackendPort, onCaptured } = {}) {
    this.getBackendPort = getBackendPort
    this.onCaptured = onCaptured
    this.timer = null
    this.lastObservedText = ''
    this.internalWrites = []
    this.requestInFlight = false
    this.imageOutputDir = path.join(__dirname, '..', '..', 'data', 'clipboard', 'images')
  }

  normalizeText(text) {
    return String(text || '').replace(/\r\n/g, '\n').trim()
  }

  pruneInternalWrites() {
    const now = Date.now()
    this.internalWrites = this.internalWrites.filter((entry) => entry.expiresAt > now)
  }

  markInternalWrite(text, reason = 'unknown') {
    const normalized = this.normalizeText(text)
    if (!normalized) {
      return
    }

    this.pruneInternalWrites()
    this.internalWrites.push({
      text: normalized,
      reason,
      expiresAt: Date.now() + INTERNAL_WRITE_TTL_MS
    })
    logElectronOverlay('clipboard-monitor', 'Marked internal clipboard write', {
      reason,
      textLength: normalized.length
    })
  }

  consumeInternalWrite(normalizedText) {
    this.pruneInternalWrites()

    const index = this.internalWrites.findIndex((entry) => entry.text === normalizedText)
    if (index === -1) {
      return null
    }

    const [entry] = this.internalWrites.splice(index, 1)
    return entry
  }

  start() {
    if (this.timer) {
      return
    }

    this.lastObservedText = this.normalizeText(clipboard.readText())
    fs.mkdirSync(this.imageOutputDir, { recursive: true })
    this.timer = setInterval(() => {
      this.checkClipboard().catch((error) => {
        logElectronOverlay('clipboard-monitor', 'Clipboard poll failed', {
          error: error.message
        })
      })
    }, POLL_INTERVAL_MS)

    logElectronOverlay('clipboard-monitor', 'Clipboard monitor started', {
      pollIntervalMs: POLL_INTERVAL_MS,
      baselineLength: this.lastObservedText.length
    })
  }

  stop() {
    if (!this.timer) {
      return
    }

    clearInterval(this.timer)
    this.timer = null
    this.internalWrites = []
    logElectronOverlay('clipboard-monitor', 'Clipboard monitor stopped')
  }

  async checkClipboard() {
    if (clipboard.availableFormats().includes('image/png')) {
      const image = clipboard.readImage()
      if (!image.isEmpty()) {
        const buffer = image.toPNG()
        const imageHash = crypto.createHash('sha256').update(buffer).digest('hex')
        const imageContent = `image:${imageHash}`

        if (imageContent !== this.lastObservedText) {
          this.lastObservedText = imageContent

          if (!this.requestInFlight) {
            await this.captureImage({
              buffer,
              imageHash,
              size: image.getSize()
            })
          }
        }
        return
      }
    }

    const normalized = this.normalizeText(clipboard.readText())
    if (!normalized || normalized === this.lastObservedText) {
      return
    }

    this.lastObservedText = normalized

    const internalWrite = this.consumeInternalWrite(normalized)
    if (internalWrite) {
      logElectronOverlay('clipboard-monitor', 'Skipped internal clipboard update', {
        reason: internalWrite.reason,
        textLength: normalized.length
      })
      return
    }

    if (this.requestInFlight) {
      logElectronOverlay('clipboard-monitor', 'Skipped clipboard capture while request in flight', {
        textLength: normalized.length
      })
      return
    }

    await this.captureText(normalized)
  }

  async captureText(content) {
    const port = this.getBackendPort?.()
    if (!port) {
      logElectronOverlay('clipboard-monitor', 'Skipped clipboard capture because backend port is unavailable')
      return
    }

    this.requestInFlight = true

    try {
      const response = await fetch(`http://127.0.0.1:${port}/api/clipboard/items`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({
          content,
          source: 'system_clipboard_monitor'
        })
      })

      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`)
      }

      const payload = await response.json()
      logElectronOverlay('clipboard-monitor', 'Captured clipboard text', {
        id: payload.id,
        itemType: payload.item_type,
        contentLength: content.length
      })

      this.onCaptured?.(payload)
    } catch (error) {
      logElectronOverlay('clipboard-monitor', 'Failed to capture clipboard text', {
        error: error.message,
        contentLength: content.length
      })
    } finally {
      this.requestInFlight = false
    }
  }

  async captureImage({ buffer, imageHash, size }) {
    const port = this.getBackendPort?.()
    if (!port) {
      logElectronOverlay('clipboard-monitor', 'Skipped clipboard image capture because backend port is unavailable')
      return
    }

    this.requestInFlight = true

    try {
      const filename = `${imageHash}.png`
      const imagePath = path.join(this.imageOutputDir, filename)
      const relativeImagePath = path.posix.join('images', filename)
      const displayName = `剪切图片-${new Date().toLocaleString('zh-CN').replace(/[/: ]/g, '-')}.png`
      if (!fs.existsSync(imagePath)) {
        fs.writeFileSync(imagePath, buffer)
      }

      const response = await fetch(`http://127.0.0.1:${port}/api/clipboard/items`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({
          content: `image:${imageHash}`,
          source: 'system_clipboard_monitor',
          item_type: 'image',
          title: displayName,
          summary: `${size.width} x ${size.height}`,
          metadata: {
            image_path: relativeImagePath,
            file_name: displayName,
            stored_file_name: filename,
            image_hash: imageHash,
            width: size.width,
            height: size.height,
            file_size: buffer.length,
            mime_type: 'image/png'
          }
        })
      })

      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`)
      }

      const payload = await response.json()
      logElectronOverlay('clipboard-monitor', 'Captured clipboard image', {
        id: payload.id,
        imageHash,
        width: size.width,
        height: size.height
      })

      this.onCaptured?.(payload)
    } catch (error) {
      logElectronOverlay('clipboard-monitor', 'Failed to capture clipboard image', {
        error: error.message,
        imageHash
      })
    } finally {
      this.requestInFlight = false
    }
  }
}

module.exports = { ClipboardMonitor }
