// 在线识别引擎（豆包 / ChatGPT 听写）卡片状态的纯逻辑（不依赖 Vue，供 tests/test_settings_online_engine.mjs 锁定）。
//
// 「填了 Token」不等于「能用」：保存后后端会做一次真实握手（见 att_core::stt::verify），
// 卡片据此区分「已配置 / 验证中 / 可用 / 验证失败 / 未能验证」。

export const ONLINE_PROVIDERS = Object.freeze(['doubao_asr', 'chatgpt_asr'])

export function isOnlineProvider(provider) {
  return ONLINE_PROVIDERS.includes(provider)
}

/**
 * 卡片上的状态点。
 *
 * @param {object} params
 * @param {boolean} params.keySet 是否已保存凭据
 * @param {{ status: 'checking'|'ok'|'invalid'|'unknown', message?: string } | null} [params.check]
 *   该引擎当前凭据的验证结论；没验过为 null
 * @returns {{ chip: string, tone: 'ok'|'warn'|'err'|'accent'|'neutral', loading: boolean }}
 */
export function onlineEngineChip({ keySet, check = null }) {
  if (!keySet) {
    return { chip: '在线 · 未配置', tone: 'warn', loading: false }
  }
  switch (check?.status) {
    case 'checking':
      return { chip: '在线 · 验证中', tone: 'accent', loading: true }
    case 'ok':
      return { chip: '在线 · 可用', tone: 'ok', loading: false }
    case 'invalid':
      return { chip: '在线 · 验证失败', tone: 'err', loading: false }
    case 'unknown':
      return { chip: '在线 · 未能验证', tone: 'warn', loading: false }
    default:
      return { chip: '在线 · 已配置', tone: 'ok', loading: false }
  }
}

/**
 * 接入配置区里的提示条：只在验证失败 / 没验成时出现。
 * 返回 { tone: 'err'|'warn', text } 或 null。
 */
export function credentialNote(check) {
  if (check?.status === 'invalid') {
    return { tone: 'err', text: check.message || '凭据验证失败，请检查后重新填写。' }
  }
  if (check?.status === 'unknown') {
    const reason = check.message ? `${check.message}。` : ''
    return { tone: 'warn', text: `${reason}暂时没能验证，录音时仍会照常尝试；再点一下上方的引擎卡片可重新验证。` }
  }
  return null
}

/**
 * 保存设置后是否需要重新验证当前在线引擎：换了引擎、填了新 Token，或改了 Resource ID / 接入点。
 *
 * @param {object} payload 本次提交的设置载荷（明文 Token 只在用户输入时出现）
 * @param {object} previous 上次已保存的服务端设置
 */
export function needsCredentialCheck(payload, previous = {}) {
  const provider = payload?.asr_provider
  if (!isOnlineProvider(provider)) {
    return false
  }
  if (provider !== previous?.asr_provider) {
    return true
  }
  if (provider === 'doubao_asr') {
    return Boolean(payload.doubao_asr_api_key) ||
      payload.doubao_asr_resource_id !== previous?.doubao_asr_resource_id ||
      payload.doubao_asr_endpoint !== previous?.doubao_asr_endpoint
  }
  return Boolean(payload.chatgpt_asr_api_key) || payload.chatgpt_asr_endpoint !== previous?.chatgpt_asr_endpoint
}
