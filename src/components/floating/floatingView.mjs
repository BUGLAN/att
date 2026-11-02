// 极简悬浮窗的纯展示模型（不依赖 Vue，供 tests/test_floating_view.mjs 锁定）。
//
// 后端推来的 floating:state（见 src-tauri/src/floating.rs 的 from_recording_state）
// → 一枚印章 + 可选的文字 + 何时收起。印章与托盘图标同源：
//   錄 待命 · 聽 录音中 · 轉 识别 / 优化中 · 成 已输入 · 敗 失败
// 一次会话里胶囊只变两次大小：没有文字时只有印章（识别 / 优化中带处理点），
// 出现第一个字后展开到固定宽度并保持（见 FloatingWindow.vue），始终单行、超出裁掉开头；
// 松开后进入「轉」沿用识别稿撑住宽度（保持正文色，处理中由印章与处理点表达），直到结束都不换：
// 优化稿不回填悬浮窗（它停留很短，一闪而过反而突兀），优化结果直接打进输入框；
// 流式录音中，末尾还没被服务端确认的那截（正在说的这句）带 pending，界面显示成淡白色；
// 结果态（成 / 敗）只剩印章，停留片刻后隐藏；失败原因放在 title 里悬停可见。

export const SEALS = Object.freeze({
  idle: '錄',
  listening: '聽',
  converting: '轉',
  success: '成',
  failure: '敗'
})

export const SUCCESS_HOLD_MS = 1400
// 失败原因直接写在胶囊里，多留一会儿读（悬停时还会暂停收起）
export const FAILURE_HOLD_MS = 3200

/**
 * 声纹门限：后端 recording:level = RMS × 6（夹到 0..1），0.19 ≈ -30 dBFS。
 * 比切段用的静音阈值（默认 -40 dBFS）高一截：环境底噪常在 -35 ~ -40 dBFS，不该让声纹一直跳。
 */
export const WAVE_SHOW_LEVEL = 0.19
/** 音量落回门限下后，声纹再跳这么久才收成静止的点（字与字之间的停顿不闪） */
export const WAVE_HOLD_MS = 700

/**
 * 声纹开关（带滞回）：音量过门限立即「有声」，落回门限下持续 WAVE_HOLD_MS 才回到「安静」。
 * @param {{ loud: boolean, lastLoudAt: number }} prev
 * @param {number} level 0..1
 * @param {number} now 毫秒时间戳
 */
export function nextVoiceState(prev = { loud: false, lastLoudAt: -Infinity }, level = 0, now = 0) {
  if (level >= WAVE_SHOW_LEVEL) {
    return { loud: true, lastLoudAt: now }
  }
  return { loud: prev.loud && now - prev.lastLoudAt < WAVE_HOLD_MS, lastLoudAt: prev.lastLoudAt }
}

/** 胶囊单行只显示得下末尾一截：保留这些字足够（空白折叠），更早的由界面从左侧裁掉。 */
export const TAIL_CHARS = 80

export function tailText(text, max = TAIL_CHARS) {
  const chars = Array.from(String(text || '').replace(/\s+/g, ' ').trim())
  return (chars.length > max ? chars.slice(-max) : chars).join('')
}

/**
 * 与 tailText 同样截取末尾，同时算出其中还没确认的那截。
 * @param {string} text 整场文本
 * @param {number} pendingChars text 末尾未确认的字数（后端按 Unicode 标量计，与 Array.from 一致）
 * @returns {{ text: string, pending: string }} pending 恒为 text 的后缀
 */
export function tailWithPending(text, pendingChars = 0, max = TAIL_CHARS) {
  const raw = Array.from(String(text || ''))
  const cut = raw.length - Math.min(raw.length, Math.max(0, Math.floor(Number(pendingChars) || 0)))
  const confirmed = raw.slice(0, cut).join('')
  const full = tailText(text, max)
  // 未确认部分按同样规则折叠空白，再截成 full 的后缀（不会比截取后的全文更长）
  const pending = Array.from(tailText(raw.slice(cut).join(''), max))
  const fullChars = Array.from(full)
  const pendingLength = confirmed.trim() ? Math.min(pending.length, fullChars.length) : fullChars.length
  return { text: full, pending: fullChars.slice(fullChars.length - pendingLength).join('') }
}

function result(seal, hideAfterMs, detail = '') {
  return { seal, glyph: SEALS[seal], active: false, text: '', pending: '', busy: false, cancellable: false, hideAfterMs, detail }
}

function working(seal, { text = '', pending = '', busy = false }) {
  return { seal, glyph: SEALS[seal], active: true, text, pending, busy, cancellable: true, hideAfterMs: null, detail: '' }
}

/**
 * @param {{ phase?: string, mode?: string, text?: string, pending?: number, draft?: string, ok?: boolean }} state
 *   pending：录音中 text 末尾未确认的字数（只有流式有，多段的分段结果都是定稿）
 *   draft：进入优化时的转写原文，照常显示；优化阶段的 text 一律不显示
 * @param {{ previousText?: string }} session 本次会话上一次显示的文字：
 *   松开后进入「轉」时后端不再带文字，沿用它撑住胶囊直到结束。
 */
export function deriveFloatingView({ phase, mode, text, pending, draft, ok } = {}, { previousText = '' } = {}) {
  switch (phase) {
    case 'recording':
      // 单段模式录完才识别：录音中只有印章
      return mode === 'streaming' || mode === 'multi'
        ? working('listening', tailWithPending(text, pending))
        : working('listening', {})
    case 'transcribing':
    case 'optimizing': {
      const pending = tailText(draft) || previousText
      return working('converting', { text: pending, busy: true })
    }
    case 'complete':
      // 旧载荷没有 ok 字段：按成功处理
      return ok === false ? result('failure', FAILURE_HOLD_MS, text || '') : result('success', SUCCESS_HOLD_MS)
    case 'error':
      return result('failure', FAILURE_HOLD_MS, text || '')
    default:
      return result('idle', null)
  }
}

/**
 * 这条状态是不是新一次会话的开头（页面据此重建胶囊、重放入场动效、清掉上一段的宽度与文字）。
 * - 胶囊此前不在显示（待命 / 结果态 / 正在淡出）时进入工作态；
 * - 或在「轉」时直接回到「聽」：流式模式允许上一段优化转入后台、立即开始下一段，
 *   而同一次会话里「聽」只会在开头，不会跟在「轉」后面。
 * @param {{ wasActive: boolean, prevSeal: string, next: { active: boolean, seal: string } }} transition
 */
export function startsNewSession({ wasActive, prevSeal, next }) {
  if (!next.active) {
    return false
  }
  return !wasActive || (next.seal === 'listening' && prevSeal === 'converting')
}

/**
 * 印章字的竖直校正量（px，负数为上移），让字的墨迹中心落在印章受光面的中心。
 *
 * 宋体等衬线字体把字画在 em 框偏下的位置（Songti TC 22px 约低 1.2–1.6px），且各平台回退字体不同，
 * 所以不写死偏移：由调用方用 canvas 按实际字体量出字形度量，这里按 CSS 行盒规则换算。
 * - `line-height: 1` 的行盒高 = 字号，基线在「半行距 + 字体上伸」处；
 * - 印章底部有 2px 暗边（内阴影），受光面是上面那 34px，以它的中心为目标。
 *
 * @param {{ fontAscent: number, fontDescent: number, inkAscent: number, inkDescent: number }} metrics
 *   canvas measureText 的 fontBoundingBoxAscent / fontBoundingBoxDescent / actualBoundingBoxAscent / actualBoundingBoxDescent
 */
export function sealGlyphNudge(metrics, { fontSize = 22, tile = 36, bevel = 2 } = {}) {
  const { fontAscent, fontDescent, inkAscent, inkDescent } = metrics || {}
  const target = (tile - bevel) / 2
  if (![fontAscent, fontDescent, inkAscent, inkDescent].every(Number.isFinite)) {
    // 量不到（极老内核）：只扣掉暗边，按字体本身居中处理
    return target - tile / 2
  }
  const baseline = (fontSize - (fontAscent + fontDescent)) / 2 + fontAscent
  const inkCenter = (tile - fontSize) / 2 + baseline + (inkDescent - inkAscent) / 2
  return Math.round((target - inkCenter) * 4) / 4
}

// ---------- 出字动效：匀速吐字 + 墨晕（设计稿 docs/prototype/floating-text-motion.html 方案 A1） ----------
// 流式识别是「几个字一批」到的，直接上屏会一顿一顿。到字先进缓冲，再按稳定节拍一个个放出，
// 每个字自己短促地洇开（floating.css .ch）；整行超宽后的左移也改成平滑追赶，不再一格一格跳。

/** 平时的吐字速度（字/秒） */
export const PACE_CPS = 16
/** 最多落后识别结果这么久（秒）：积压越多放得越快 */
export const PACE_MAX_LAG_S = 0.5
/** 整行左移的平滑时间常数（ms）：指数逼近，不回弹 */
export const GLIDE_TAU_MS = 110

/** 末尾重合段至少这么长才算数：单个字（的、了）巧合对上太常见，不能据此认定回改的边界 */
export const MIN_TAIL_MATCH = 2

/**
 * 在 target[keep, …) 里找与已上屏末尾重合最长的位置（中段回改后，回改点之后的字往往原样还在）。
 * @returns {{ tail: number, end: number }} 已上屏的末 tail 个字对上 target[end - tail, end)；
 *   对不上时 tail = 0，end 取「原位替换」的长度
 */
function matchTail(shown, drop, keep, target) {
  const rest = shown.length - drop
  let best = { tail: 0, end: Math.min(rest, target.length) }
  for (let end = target.length; end > keep; end--) {
    let tail = 0
    while (tail < rest - keep && tail < end - keep && shown[shown.length - 1 - tail] === target[end - 1 - tail]) {
      tail++
    }
    // 同样长时取离原长度最近的位置：回改多半不改变字数
    if (tail >= MIN_TAIL_MATCH && (tail > best.tail || (tail === best.tail && Math.abs(end - rest) < Math.abs(best.end - rest)))) {
      best = { tail, end }
    }
  }
  return best
}

/**
 * 把已上屏的字对齐到新的识别文本。
 * 新文本可能是：末尾追加、回改（流式识别会改正在说的这句、定稿时补标点 / 数字改写，回改点可能在中间）、
 * 或因 tailText 只留末尾而从开头裁掉了几个字。
 * 在已上屏的字里找一个开头裁掉量 drop，使剩下的字与新文本开头、末尾两段重合的字数最多
 * （同样多时取开头重合更长的，再取最小 drop）。只有中间对不上的那截算回改，前后两段原样保留，
 * 回改点离末尾再远，屏上看得见的字也不会整行重打。
 * @param {string[]} shown 已上屏的字（按 Array.from 切分）
 * @param {string[]} target 新的识别文本（按 Array.from 切分）
 * @returns {{ drop: number, keep: number, tail: number, end: number }}
 *   开头去掉 drop 个；剩下的前 keep 个、末 tail 个原样保留，中间换成 target[keep, end - tail)；
 *   对齐后上屏 end 个字（= target 的前 end 个），其后的交给节拍器
 */
export function reconcileLine(shown, target) {
  let best = { drop: 0, keep: 0, tail: 0, end: Math.min(shown.length, target.length) }
  let bestScore = 0
  for (let drop = 0; drop < shown.length; drop++) {
    const rest = shown.length - drop
    let keep = 0
    while (keep < rest && keep < target.length && shown[drop + keep] === target[keep]) {
      keep++
    }
    const { tail, end } = keep < rest ? matchTail(shown, drop, keep, target) : { tail: 0, end: rest }
    const score = keep + tail
    if (score > bestScore || (score === bestScore && score > 0 && keep > best.keep)) {
      best = { drop, keep, tail, end }
      bestScore = score
    }
  }
  return best
}

/**
 * 吐字节拍器：按经过的时间累计可放出的字数。
 * @param {number} budget 上一帧剩下的零头
 * @param {number} dtMs 距上一帧的毫秒数
 * @param {number} backlog 还没上屏的字数
 * @returns {{ emit: number, budget: number }}
 */
export function paceStep(budget, dtMs, backlog) {
  if (backlog <= 0) {
    return { emit: 0, budget: 0 }
  }
  const cps = Math.max(PACE_CPS, backlog / PACE_MAX_LAG_S)
  const next = budget + (Math.max(0, dtMs) / 1000) * cps
  const emit = Math.min(backlog, Math.floor(next))
  return { emit, budget: next - emit }
}

/** 整行左移量向目标指数逼近；足够近时直接落定，好让动画循环停下来 */
export function glideToward(current, target, dtMs, tau = GLIDE_TAU_MS) {
  const next = target + (current - target) * Math.exp(-Math.max(0, dtMs) / tau)
  return Math.abs(next - target) < 0.25 ? target : next
}
