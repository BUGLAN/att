export function derivePostStopState({ mode, durationSeconds }) {
  const normalizedDuration = Number.isFinite(durationSeconds) ? durationSeconds : 0

  if (mode === 'single') {
    if (normalizedDuration >= 1) {
      return {
        recording: false,
        transcribing: true,
        trayIcon: 'transcribing'
      }
    }

    return {
      recording: false,
      transcribing: false,
      trayIcon: 'idle'
    }
  }

  if (mode === 'multi' || mode === 'streaming') {
    return {
      recording: false,
      transcribing: true,
      trayIcon: 'transcribing'
    }
  }

  return {
    recording: false,
    transcribing: false,
    trayIcon: 'idle'
  }
}

export function deriveStartAbortState() {
  return {
    recording: false,
    transcribing: false,
    trayIcon: 'idle'
  }
}
