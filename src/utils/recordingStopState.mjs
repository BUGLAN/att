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

/**
 * 判断「启动进行中收到的停止请求」是否应当排队，等启动完成后再消费。
 *
 * 关键点一：`prepareRecordingEnvironment` 在打开麦克风**之前**就把 `isRecording` 置为 true，
 * 因此启动窗口内 `isRecording` 不能作为"录音是否已经开始"的判据。历史缺陷正出在这里：
 * 判据写成 `startPending && !appStore.isRecording` 时，整个启动窗口内恒为假，松手请求落进
 * 「放弃 / 收尾」分支；而 `startStreamingRecording` 开头又会复位放弃标志，最终开出一段
 * 无人停止的幽灵录音（托盘显示录音中、悬浮窗停在"正在完成流式转写"）。
 *
 * 关键点二：不要用 `currentSessionId` 判断"会话是否已建立"——意外断开等路径不会清空它，
 * 残留 id 会让短按松手被误判成「会话已建立」而绕开排队。`startPending` 本身就够了：
 * 所有 start 的 finally 都会清它，且建连有 8s settle 兜底，排队窗口有界。
 *
 * @param {object} params
 * @param {boolean} params.startPending 启动是否仍在进行（prepareRecordingEnvironment 置位，各 start 的 finally 清除）
 * @param {boolean} [params.fromStart] 当前调用是否已是「启动完成后消费排队停止」，该路径不能再入队（否则死循环）
 */
export function shouldQueueStopDuringStart({ startPending, fromStart = false }) {
  return Boolean(startPending) && !fromStart
}
