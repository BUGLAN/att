// 识别引擎页的纯逻辑：录音模式与引擎的兼容关系、本地引擎加载状态的展示模型
// （不依赖 Vue，供 tests/test_settings_asr_engine.mjs 锁定）。
//
// 录音模式和引擎放在同一页选：流式只有豆包支持，选别的引擎时流式卡片直接置灰并说明原因，
// 而不是让用户去另一页才发现「当前引擎不支持流式」。

export const LOCAL_PROVIDERS = Object.freeze(['whisper', 'qwen_asr'])

/** 支持流式（边说边出字）的引擎：目前只有豆包 SAUC。 */
export const STREAMING_PROVIDERS = Object.freeze(['doubao_asr'])

export const RECORDING_MODES = Object.freeze([
  { value: 'single', label: '单段', desc: '录完整段后一次识别' },
  { value: 'multi', label: '多段', desc: '按静音自动切段，边录边出字' },
  { value: 'streaming', label: '流式', desc: '边说边出字，延迟最低（仅豆包语音）' }
])

const PROVIDER_LABELS = Object.freeze({
  doubao_asr: '豆包语音',
  chatgpt_asr: 'ChatGPT 听写',
  whisper: 'faster-whisper',
  qwen_asr: 'Qwen3-ASR'
})

export function providerLabel(provider) {
  return PROVIDER_LABELS[provider] || provider || '当前引擎'
}

export function isLocalProvider(provider) {
  return LOCAL_PROVIDERS.includes(provider)
}

export function supportsStreaming(provider) {
  return STREAMING_PROVIDERS.includes(provider)
}

/**
 * 录音模式卡片：按当前引擎标出不可用的模式。
 * 返回 [{ value, label, desc, disabled, hint }]；hint 为不可用原因（可用时为空串）。
 */
export function recordingModeOptions(provider) {
  return RECORDING_MODES.map((mode) => {
    const disabled = mode.value === 'streaming' && !supportsStreaming(provider)
    return {
      ...mode,
      disabled,
      hint: disabled ? `${providerLabel(provider)}不支持流式` : ''
    }
  })
}

/** 当前引擎与录音模式冲突时给出的提示（无冲突返回空串）。 */
export function recordingModeConflict(provider, recordingMode) {
  if (recordingMode === 'streaming' && !supportsStreaming(provider)) {
    return `${providerLabel(provider)}不支持流式录音，录音时会直接报错；请改用单段或多段。`
  }
  return ''
}

/**
 * 本地引擎加载状态 → 展示模型。
 *
 * `localEngine` 是 /api/health 的 `local_engine` 字段（只描述当前设置里选中的本地引擎）。
 * 返回 { state, tone, chip, title, message, tip }（tip：可选的下一步提示，界面不放按钮，加载/重试靠点卡片）：
 * - none：不是本地引擎，不展示；
 * - loading：后端还没切到这个引擎（保存防抖中），或正在加载/预热；
 * - missing：模型或推理库文件不在，message 是后端给的可照做提示（含下载命令，见 {@link splitHint}）；
 * - failed：文件在但加载失败；
 * - ready：已加载常驻，message 为「设备 · 模型目录」；
 * - idle：文件就位、尚未加载（首次录音时会自动加载）。
 */
export function localEngineView(provider, localEngine) {
  if (!isLocalProvider(provider)) {
    return { state: 'none', tone: 'neutral', chip: '', title: '', message: '', tip: '' }
  }
  if (!localEngine || localEngine.provider !== provider || localEngine.loading) {
    return {
      state: 'loading',
      tone: 'info',
      chip: '加载中',
      title: '正在加载模型',
      message: '首次加载需要十几秒到半分钟，期间可以照常录音，识别会在加载完成后进行。',
      tip: ''
    }
  }
  if (!localEngine.available) {
    return {
      state: 'missing',
      tone: 'warn',
      chip: '缺少模型',
      title: '缺少模型文件',
      message: localEngine.error || `未找到 ${providerLabel(provider)} 的模型文件`,
      tip: ''
    }
  }
  if (localEngine.load_error) {
    return {
      state: 'failed',
      tone: 'err',
      chip: '加载失败',
      title: '模型加载失败',
      message: localEngine.load_error,
      tip: '再点一下上方的引擎卡片即可重试。'
    }
  }
  if (localEngine.loaded) {
    return {
      state: 'ready',
      tone: 'ok',
      chip: '已就绪',
      title: '模型已就绪',
      message: [localEngine.device, localEngine.model_dir].filter(Boolean).join(' · '),
      tip: ''
    }
  }
  return {
    state: 'idle',
    tone: 'neutral',
    chip: '未加载',
    title: '模型未加载',
    message: '模型文件已就位，首次录音时自动加载。',
    tip: '想省掉第一次录音的等待，再点一下上方的引擎卡片即可提前加载。'
  }
}

/**
 * 把后端的可照做提示拆成三段，便于把命令单独排成代码块：
 * 首行是结论（summary），以 Tab 缩进的行是命令（commands），其余行是补充说明（notes）。
 */
export function splitHint(message) {
  const lines = String(message || '').split('\n')
  const summary = (lines.shift() || '').trim()
  const commands = []
  const notes = []
  for (const line of lines) {
    if (!line.trim()) {
      continue
    }
    if (/^\s/.test(line)) {
      commands.push(line.trim())
    } else {
      notes.push(line.trim())
    }
  }
  return { summary, commands, notes }
}
