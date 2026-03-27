function clamp(value, min, max) {
  return Math.min(Math.max(value, min), max)
}

function rmsToDb(rms) {
  if (!rms || Number.isNaN(rms)) {
    return -100
  }

  return 20 * Math.log10(rms)
}

export function createSilenceDetector(options) {
  const {
    stream,
    silenceLevel = -40,
    silenceThreshold = 1500,
    minSpeechActivateMs = 120,
    sampleIntervalMs = 40,
    onSpeechStart,
    onSpeechEnd,
    onLevelChange
  } = options

  const AudioContextClass = globalThis.window?.AudioContext || globalThis.window?.webkitAudioContext
  if (!AudioContextClass) {
    throw new Error('AudioContext unavailable')
  }

  const audioContext = new AudioContextClass()
  const source = audioContext.createMediaStreamSource(stream)
  const analyser = audioContext.createAnalyser()
  analyser.fftSize = 2048
  analyser.smoothingTimeConstant = 0.75
  source.connect(analyser)

  const samples = new Float32Array(analyser.fftSize)
  let speaking = false
  let speechCandidateAt = 0
  let silenceStartedAt = 0
  let timer = null

  function sample() {
    analyser.getFloatTimeDomainData(samples)

    let sumSquares = 0
    for (let index = 0; index < samples.length; index += 1) {
      const value = samples[index]
      sumSquares += value * value
    }

    const rms = Math.sqrt(sumSquares / samples.length)
    const db = rmsToDb(rms)
    const normalizedLevel = clamp((db - silenceLevel) / 32, 0, 1)

    onLevelChange?.({
      db,
      normalizedLevel,
      speaking
    })

    const now = Date.now()

    if (db > silenceLevel) {
      silenceStartedAt = 0

      if (!speaking) {
        if (!speechCandidateAt) {
          speechCandidateAt = now
          return
        }

        if (now - speechCandidateAt >= minSpeechActivateMs) {
          speaking = true
          speechCandidateAt = 0
          onSpeechStart?.()
        }
      }

      return
    }

    speechCandidateAt = 0

    if (!speaking) {
      return
    }

    if (!silenceStartedAt) {
      silenceStartedAt = now
      return
    }

    if (now - silenceStartedAt >= silenceThreshold) {
      speaking = false
      silenceStartedAt = 0
      onSpeechEnd?.()
    }
  }

  if (audioContext.state === 'suspended') {
    audioContext.resume().catch(() => {})
  }

  timer = globalThis.window?.setInterval(sample, sampleIntervalMs)

  return {
    stop() {
      if (timer) {
        globalThis.window?.clearInterval(timer)
        timer = null
      }

      source.disconnect()
      analyser.disconnect()
      audioContext.close().catch(() => {})
    }
  }
}
