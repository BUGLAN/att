// 首页引擎标签的展示逻辑（不依赖 Vue，供 tests/test_recorder_engine_status.mjs 锁定）。
//
// 模型加载 / 引擎不可用仍然优先；两者都不成立时，在线凭据正在验证不能提前显示「已就绪」。

/**
 * @param {object} params
 * @param {string} params.engineText 引擎与录音模式，例如「豆包语音 · 单段」
 * @param {boolean} params.serviceLoading 本地模型是否正在加载
 * @param {boolean} params.buttonDisabled 录音按钮是否不可用
 * @param {string} params.disabledTitle 不可用时的标题
 * @param {{ status: 'checking'|'ok'|'invalid'|'unknown' } | null} [params.credentialCheck]
 * @returns {{ text: string, dot: string, title: string }}
 */
export function deriveRecorderEngineStatus({
  engineText,
  serviceLoading,
  buttonDisabled,
  disabledTitle,
  credentialCheck = null
}) {
  if (serviceLoading) {
    return { text: `${engineText} · 模型加载中`, dot: 'gold pulse', title: '模型加载中' }
  }
  if (buttonDisabled) {
    return { text: engineText, dot: 'warn', title: disabledTitle }
  }
  if (credentialCheck?.status === 'checking') {
    return { text: `${engineText} · 验证中`, dot: 'gold pulse', title: '正在验证凭据' }
  }
  return { text: engineText, dot: 'ok', title: '识别引擎已就绪' }
}
