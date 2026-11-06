// 录音状态 / 完成态文案映射（纯查表，无副作用）。
//
// 完成提示按阶段给三态文案（注意真流式那句是"复制或粘贴失败"，与其他阶段的"复制并粘贴失败"不同）。
// 本模块不依赖 Vue / 浏览器全局对象，可被 Node 直接单测。

// 空闲态兜底文案
export const IDLE_SESSION_STATUS_TEXT = '按快捷键开始录音'

// 完成态三态文案：copied = 已复制并粘贴；failed = 尝试过但失败；plain = 未开启自动复制
const COMPLETION_TOAST_COPY = Object.freeze({
  transcription: Object.freeze({
    copied: '转写完成，已复制并粘贴',
    failed: '转写完成，复制并粘贴失败',
    plain: '转写完成'
  }),
  optimization: Object.freeze({
    copied: '优化完成，已输入并复制',
    failed: '优化完成，自动输入失败',
    plain: '优化完成'
  }),
  streaming: Object.freeze({
    copied: '真流式转写完成，已复制并粘贴',
    failed: '真流式转写完成，复制或粘贴失败',
    plain: '真流式转写完成'
  })
})

/**
 * 完成态提示文案。stage 取 transcription / optimization / streaming。
 * copyResult 为录音 Actor 回报的复制结果（{ attempted, copied, pasted }）。
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
 * 当前支持的完成态阶段列表（便于测试与调用方排查打错字的情况）。
 */
export function listCompletionStages() {
  return Object.keys(COMPLETION_TOAST_COPY)
}
