// 录音状态 / 完成态文案映射（纯查表，无副作用）。
//
// 背景：原 useRecorder.js 里同一套"完成提示"三态文案重复了 6 遍（WS 通知、轮询、多段收尾、
// 真流式完成），多段录音阶段文案又是一条 if-else 链。这里统一收敛成纯函数，
// 文案字符串逐字保持原样（注意真流式那句是"复制或粘贴失败"，与其他阶段的"复制并粘贴失败"不同）。
//
// 本模块不依赖 Vue / 浏览器全局对象，可被 Node 直接单测。

// 多段录音阶段（livePhase）-> 会话状态文案
const REALTIME_PHASE_TEXT = Object.freeze({
  listening: '静音切段已启用，停顿后会自动出字',
  speaking: '正在监听语音片段...',
  'segment-processing': '上一段正在转写，继续说话即可',
  finalizing: '正在合并整场录音结果',
  completed: '多段录音处理完成',
  error: '本次录音未形成有效结果'
})

// 空闲态兜底文案（原 setRealtimePhase 的 else 分支）
export const IDLE_SESSION_STATUS_TEXT = '按住快捷键开始录音'

// 完成态三态文案：copied = 已复制并粘贴；failed = 尝试过但失败；plain = 未开启自动复制
const COMPLETION_TOAST_COPY = Object.freeze({
  transcription: Object.freeze({
    copied: '转写完成，已复制并粘贴',
    failed: '转写完成，复制并粘贴失败',
    plain: '转写完成'
  }),
  optimization: Object.freeze({
    copied: '优化完成，已复制并粘贴',
    failed: '优化完成，复制并粘贴失败',
    plain: '优化完成'
  }),
  streaming: Object.freeze({
    copied: '真流式转写完成，已复制并粘贴',
    failed: '真流式转写完成，复制或粘贴失败',
    plain: '真流式转写完成'
  }),
  multi: Object.freeze({
    copied: '多段录音完成，已复制并粘贴',
    failed: '多段录音完成，复制并粘贴失败',
    plain: '多段录音完成'
  })
})

/**
 * 多段录音阶段对应的会话状态文案；未知阶段回退空闲文案。
 * 对应原 setRealtimePhase 中 phase -> sessionStatusText 的 if-else 链。
 */
export function deriveRealtimePhaseText(phase) {
  return REALTIME_PHASE_TEXT[phase] || IDLE_SESSION_STATUS_TEXT
}

/**
 * 完成态提示文案。stage 取 transcription / optimization / streaming / multi。
 * copyResult 为 maybeCopyText 的返回值（{ attempted, copied, pasted }）。
 */
export function deriveCompletionToast(stage, copyResult) {
  const copy = COMPLETION_TOAST_COPY[stage]
  if (!copy) {
    throw new Error(`未知的完成态阶段：${stage}`)
  }

  if (copyResult.pasted) {
    return copy.copied
  }

  if (copyResult.attempted) {
    return copy.failed
  }

  return copy.plain
}

/**
 * 优化降级提示文案：用户在设置里配置的 LLM 调用失败后，回退到 .env 内置 key
 * 完成了优化。优化结果本身可用，但必须让用户知道「配置没生效」，
 * 否则设置页里配错的 key 会被静默掩盖成「优化成功」。
 */
export function deriveOptimizationDegradedToast(reason) {
  const detail = String(reason || '').trim() || '配置的 LLM 调用失败'
  return `优化已降级：${detail}（本次已用内置 key 完成）`
}

/**
 * 当前支持的完成态阶段列表（便于测试与调用方排查打错字的情况）。
 */
export function listCompletionStages() {
  return Object.keys(COMPLETION_TOAST_COPY)
}
