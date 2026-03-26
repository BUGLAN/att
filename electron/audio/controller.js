const { execFile } = require('child_process')
const path = require('path')

class DesktopAudioController {
  constructor(getPythonPath) {
    this.getPythonPath = getPythonPath
    this.enabled = true
    this.recordingActive = false
    this.originalMuted = null
    this.mutedByController = false
    this.helperPath = path.join(__dirname, '..', 'python', 'audio_helper.py')
  }

  updateSettings(settings = {}) {
    if (typeof settings.pause_audio_on_recording === 'boolean') {
      this.enabled = settings.pause_audio_on_recording
    }

    if (!this.enabled && this.recordingActive) {
      return this.resumeAfterRecording()
    }

    return Promise.resolve({
      success: true,
      enabled: this.enabled
    })
  }

  async pauseForRecording() {
    if (!this.enabled) {
      return { success: true, skipped: true, reason: 'disabled' }
    }

    if (this.recordingActive) {
      return { success: true, skipped: true, reason: 'already-recording' }
    }

    const status = await this.runHelper(['status'])
    if (!status.success) {
      return status
    }

    this.recordingActive = true
    this.originalMuted = !!status.muted

    if (this.originalMuted) {
      this.mutedByController = false
      return { success: true, skipped: true, muted: true }
    }

    const result = await this.runHelper(['set-mute', 'true'])
    this.mutedByController = !!result.success
    return result
  }

  async resumeAfterRecording() {
    if (!this.recordingActive) {
      return { success: true, skipped: true, reason: 'not-recording' }
    }

    const restoreMuted = !!this.originalMuted
    this.recordingActive = false

    try {
      if (!this.enabled && !this.mutedByController && !restoreMuted) {
        return { success: true, skipped: true, reason: 'disabled' }
      }

      if (this.mutedByController || restoreMuted) {
        return await this.runHelper(['set-mute', restoreMuted ? 'true' : 'false'])
      }

      return { success: true, skipped: true, reason: 'unchanged' }
    } finally {
      this.originalMuted = null
      this.mutedByController = false
    }
  }

  runHelper(args) {
    return new Promise((resolve) => {
      const pythonPath = this.getPythonPath()
      execFile(pythonPath, [this.helperPath, ...args], { cwd: path.join(__dirname, '..', '..') }, (error, stdout, stderr) => {
        const output = (stdout || '').trim()
        if (output) {
          try {
            const parsed = JSON.parse(output)
            if (error && parsed.success !== false) {
              resolve({
                ...parsed,
                success: false,
                error: stderr?.trim() || error.message
              })
              return
            }
            resolve(parsed)
            return
          } catch (parseError) {
            // Fall through to generic error handling.
          }
        }

        resolve({
          success: false,
          error: (stderr || '').trim() || error?.message || 'Audio helper returned no output'
        })
      })
    })
  }
}

module.exports = { DesktopAudioController }
