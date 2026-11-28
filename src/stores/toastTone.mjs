export const TOAST_TONES = ['success', 'error', 'warn', 'info']

/** 未知色调继续回退到 info，保持旧调用方行为。 */
export function normalizeToastTone(tone) {
  return TOAST_TONES.includes(tone) ? tone : 'info'
}
