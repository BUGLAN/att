// 真流式录音的 provider 展示名（纯查表，无副作用，可被 Node 直接单测）。

// provider 在录音界面上的展示名（文案不再写死 Qwen）
const STREAMING_PROVIDER_LABELS = Object.freeze({
  qwen_asr: 'Qwen 真流式',
  doubao_asr: '豆包在线流式'
})

/**
 * 真流式 provider 的展示名；未知 provider 回落到通用文案。
 */
export function deriveStreamingProviderLabel(provider) {
  return STREAMING_PROVIDER_LABELS[provider] || '真流式'
}
