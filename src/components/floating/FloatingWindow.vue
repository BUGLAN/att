<template>
  <div class="floating-shell">
    <!--
      极简胶囊：一次会话里只变两次大小——
      起步只有一枚印章（与托盘同源）；出现第一个字后一口气展开到固定宽度并保持，
      之后字数增减、识别回改、聽→轉 都只换里面的内容；结束时收回成 成 / 敗 一枚印章。
      换印章时底色渐变、旧字先缩小淡出，新字稍后像盖章一样落下（两字不同时叠着），印章本身轻按一下；
      文字、声纹 / 处理点、✕ 在原地淡出的同时胶囊就开始收拢（收起走缓入缓出，印章平稳回到中间），不会一下子跳变。
      优化稿不回填：「轉」期间一直是识别稿（正文色，不变淡），优化结果直接打进输入框。
      敗：原因直接写在印章旁（出过字的会话保持原宽度，没出过字的按原因长短展开），悬停时暂停收起，点一下唤起主窗口。
      流式录音中，服务端还没确认的那截（正在说的这句）用淡白色，确认后变回正常文字色。
      始终单行，超出宽度后裁掉开头（左侧渐隐），最新的字留在右边。
      出字：到字先进缓冲，按稳定节拍逐字放出、每个字墨晕洇开；超宽后整行平滑左移（见 floatingView.mjs 的 paceStep）。
      胶囊本身不参与排版：内层 .content 排好，胶囊量出它的大小后用过渡追上去。
    -->
    <section
      :key="sessionKey"
      class="capsule"
      :class="{ sized, settling: !view.active, leaving: leaving || view.seal === 'idle', clickable: Boolean(reason) }"
      :data-seal="view.seal"
      :style="capsuleStyle"
      :title="view.detail || undefined"
      role="status"
      aria-live="polite"
      @mouseenter="hovered = true"
      @mouseleave="hovered = false"
      @click="onCapsuleClick"
    >
      <div :ref="observeContent" class="content" :class="{ expanded: expanded || (wide && reason) }">
        <span ref="sealEl" class="seal" aria-hidden="true">
          <Transition name="glyph">
            <span :key="view.glyph" class="seal-glyph">
              <span class="seal-ink" :style="{ transform: `translateY(${glyphNudges[view.glyph] || 0}px)` }">{{ view.glyph }}</span>
            </span>
          </Transition>
        </span>
        <span class="sr-only">{{ SEAL_LABELS[view.seal] }}{{ view.detail ? `：${view.detail}` : '' }}</span>

        <!--
          离场的元素一律先钉在原位、脱离排版（detachOnLeave），在原地淡出：
          内容当场排成新的尺寸，胶囊立刻开始收拢，淡出与收拢同时进行，不会淡完再空等一拍才收；
          新旧元素（识别稿→失败原因、声纹→处理点）也就在同一个位置交叠，宽度只变一次。
        -->
        <Transition name="line" @before-leave="detachOnLeave">
          <span v-if="expanded" key="line" ref="lineEl" class="line" :class="{ clipped }"><span ref="trackEl" class="track" :style="{ transform: `translateX(${-shift}px)` }"><span v-for="(c, index) in shownChars" :key="c.id" class="ch" :class="{ pending: index >= pendingFrom }">{{ c.ch }}</span></span></span>
          <span v-else-if="reason" key="reason" class="line reason">{{ reason }}</span>
        </Transition>

        <Transition name="slot" @before-leave="detachOnLeave">
          <span v-if="showWave" key="wave" class="slot wave" :class="{ quiet: !voice.loud }" aria-hidden="true">
            <span v-for="(height, index) in waveHeights" :key="index" class="wave-bar" :style="{ height: `${height}px` }"></span>
          </span>
          <span v-else-if="view.busy" key="busy" class="slot busy" aria-hidden="true">
            <span></span><span></span><span></span>
          </span>
        </Transition>

        <Transition name="slot" @before-leave="detachOnLeave">
          <button v-if="showCancel" type="button" class="cancel" aria-label="取消" title="取消" @click="cancel">
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
          </button>
        </Transition>
      </div>
    </section>
  </div>
</template>

<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { deriveFloatingView, glideToward, nextVoiceState, paceStep, reconcileLine, sealGlyphNudge, startsNewSession } from './floatingView.mjs'

const SEAL_LABELS = { idle: '待命', listening: '录音中', converting: '处理中', success: '已完成', failure: '失败' }
const WAVE_BARS = 7
/** 安静时声纹收成的小点高度（px） */
const WAVE_REST = 3
/** 与 floating.css 的 .capsule.leaving 过渡时长一致：淡出完再真正隐藏窗口 */
const LEAVE_MS = 200
/** 结果态悬停时暂停收起（读失败原因），移开后再停留这么久 */
const HOVER_REHIDE_MS = 1200
/** 胶囊贴底的边距，与 floating.css .floating-shell 的 padding-bottom 一致 */
const SHELL_PADDING_BOTTOM = 12
/** 量到尺寸前胶囊的默认边长，与 floating.css .capsule 的 width / height 一致 */
const CAPSULE_MIN = 54

const state = ref({ phase: 'idle' })
const levels = ref(Array(WAVE_BARS).fill(0))
const size = ref({ width: 0, height: 0 })
const sized = ref(false)
const sessionKey = ref(0)
const leaving = ref(false)
const hovered = ref(false)
const lineEl = ref(null)
const trackEl = ref(null)
/** 已上屏的字：识别文本的前缀，由节拍器逐字补齐（id 让新字才播墨晕，旧字不重播） */
const shownChars = ref([])
/** 整行左移量（px），平滑追赶「文字宽 − 可视宽」 */
const shift = ref(0)
let charSeq = 0
let paceBudget = 0
let rafId = 0
let lastFrameAt = 0
const sealEl = ref(null)
/** 各印章字的竖直校正（px），按当前系统实际用到的字体量出来，见 sealGlyphNudge；新旧字交叠时各用各的 */
const glyphNudges = ref({})
const nudgeCache = new Map()
let measureCtx = null
/** 本次会话是否已展开（出现过文字）；展开后保持到会话结束，不随字数伸缩 */
const wide = ref(false)
/** 当前是否有人在说话（音量过门限，带滞回，见 nextVoiceState） */
const voice = ref(nextVoiceState())
/** 本次会话说过话：声纹位从此保留到录音结束，安静时收成静止的点，胶囊不随说停伸缩 */
const voiced = ref(false)
/** 本次会话最近一次显示的文字（含未确认的草稿）：进入「轉」、优化增量还没出字时沿用它撑住胶囊 */
const lastText = ref('')
let hideTimer = null
let leaveTimer = null
let observer = null

const view = computed(() => deriveFloatingView(state.value, { previousText: lastText.value }))
const expanded = computed(() => wide.value && view.value.active)
// 没展开时 ✕ 收起来保持最小，悬停才露出；展开后常驻
// pending 恒为 text 的后缀（见 tailWithPending）：下标从这里起的字是还没确认的淡白草稿
const pendingFrom = computed(() => Array.from(view.value.text).length - Array.from(view.value.pending).length)
/** 展开时才有要上屏的字；收起后清空，下次展开从头写 */
const targetChars = computed(() => (expanded.value ? Array.from(view.value.text) : []))
// 开头被裁掉时左侧渐隐，提示前面还有内容
const clipped = computed(() => shift.value > 0.5)
// 失败原因直接显示在印章旁（完整内容仍在 title 里）
const reason = computed(() => (view.value.seal === 'failure' ? view.value.detail : ''))
const showCancel = computed(() => view.value.cancellable && (expanded.value || hovered.value))
// 声纹：录音中音量第一次过门限时出现（不必等出字）
const showWave = computed(() => voiced.value && view.value.seal === 'listening')
// 音量 0..1 偏小，开方放大低音量段的起伏；安静时收成一排小点
const waveHeights = computed(() => levels.value.map((level) => (
  voice.value.loud ? Math.round(4 + Math.sqrt(Math.min(1, level)) * 18) : WAVE_REST
)))
// 胶囊在窗口里的区域（贴底居中）；淡出 / 待命时为 null，整窗点击穿透。按目标尺寸算，不等宽高过渡走完
const hitRect = computed(() => {
  if (leaving.value || view.value.seal === 'idle') {
    return null
  }
  const width = size.value.width || CAPSULE_MIN
  const height = size.value.height || CAPSULE_MIN
  return [(window.innerWidth - width) / 2, window.innerHeight - SHELL_PADDING_BOTTOM - height, width, height]
})
const capsuleStyle = computed(() => (
  size.value.width ? { width: `${size.value.width}px`, height: `${size.value.height}px` } : null
))

function observeContent(el) {
  observer?.disconnect()
  if (!el) {
    return
  }
  observer = new ResizeObserver(() => {
    // 胶囊有 1px 描边（border-box），比内容各大 2px；尺寸没变就不换对象，免得下游（命中区域上报）空跑
    const next = { width: el.offsetWidth + 2, height: el.offsetHeight + 2 }
    if (next.width !== size.value.width || next.height !== size.value.height) {
      size.value = next
    }
    if (!sized.value) {
      // 首帧直接落到初始尺寸，下一帧起才开过渡，避免从 0 长出来
      requestAnimationFrame(() => { sized.value = true })
    }
  })
  observer.observe(el)
}

function measureGlyphNudge(glyph) {
  const el = sealEl.value
  if (!el || !glyph) {
    return
  }
  const style = getComputedStyle(el)
  const font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`
  const key = `${font}|${glyph}`
  if (!nudgeCache.has(key)) {
    measureCtx = measureCtx || document.createElement('canvas').getContext('2d')
    measureCtx.font = font
    const m = measureCtx.measureText(glyph)
    nudgeCache.set(key, sealGlyphNudge({
      fontAscent: m.fontBoundingBoxAscent,
      fontDescent: m.fontBoundingBoxDescent,
      inkAscent: m.actualBoundingBoxAscent,
      inkDescent: m.actualBoundingBoxDescent
    }, { fontSize: parseFloat(style.fontSize), tile: el.offsetHeight || 36 }))
  }
  glyphNudges.value = { ...glyphNudges.value, [glyph]: nudgeCache.get(key) }
}

function prefersReducedMotion() {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
}

/** 换印章时印章轻按一下；失败再左右轻摇，提示出了问题 */
function pressSeal(seal) {
  const el = sealEl.value
  if (!el?.animate || prefersReducedMotion()) {
    return
  }
  const press = [
    { transform: 'scale(1)' },
    { transform: 'scale(0.86)', offset: 0.35 },
    { transform: 'scale(1)' }
  ]
  const shake = [
    { transform: 'translateX(0)' },
    { transform: 'translateX(-3px)', offset: 0.2 },
    { transform: 'translateX(3px)', offset: 0.45 },
    { transform: 'translateX(-2px)', offset: 0.7 },
    { transform: 'translateX(0)' }
  ]
  el.animate(seal === 'failure' ? shake : press, {
    duration: seal === 'failure' ? 380 : 320,
    easing: 'cubic-bezier(0.2, 0.9, 0.25, 1)'
  })
}

/** 这次按键没生效（收尾 / 优化中又按了录音键）：印章摇一下，与失败同一个动作 */
function nudgeSeal() {
  pressSeal('failure')
}

function clearTimers() {
  clearTimeout(hideTimer)
  clearTimeout(leaveTimer)
  hideTimer = null
  leaveTimer = null
}

function applyState(payload = {}) {
  const wasActive = view.value.active && !leaving.value
  const prevSeal = view.value.seal
  state.value = payload || {}
  const next = view.value
  clearTimers()

  if (startsNewSession({ wasActive, prevSeal, next })) {
    // 新一次会话：重建胶囊重放入场动效，并从最小尺寸重新生长
    sessionKey.value += 1
    sized.value = false
    size.value = { width: 0, height: 0 }
    levels.value = Array(WAVE_BARS).fill(0)
    voice.value = nextVoiceState()
    voiced.value = false
    hovered.value = false
    wide.value = false
    lastText.value = ''
  }
  leaving.value = false

  if (next.active && next.text) {
    wide.value = true
    // 未确认的草稿也记下：否则下一个空增量会退回更早的识别中间结果，文字跳一下
    lastText.value = next.text
  }

  if (next.hideAfterMs && !hovered.value) {
    scheduleHide(next.hideAfterMs)
  }
}

function scheduleHide(delay) {
  clearTimers()
  hideTimer = setTimeout(() => {
    leaving.value = true
    leaveTimer = setTimeout(() => window.floatingAPI?.close(), LEAVE_MS)
  }, delay)
}

function onCapsuleClick() {
  if (reason.value) {
    window.floatingAPI?.openMain?.()
  }
}

function pushLevel(level) {
  if (view.value.seal !== 'listening') {
    return
  }
  voice.value = nextVoiceState(voice.value, level, performance.now())
  if (voice.value.loud) {
    voiced.value = true
  }
  levels.value = [...levels.value.slice(1), level]
}

function cancel() {
  window.floatingAPI?.cancel?.()
}

/** 离场元素钉在当前位置（相对 .content）并脱离排版：内容立即按剩下的元素排版，它在原地淡出 */
function detachOnLeave(el) {
  const { offsetLeft, offsetTop, offsetWidth, offsetHeight } = el
  Object.assign(el.style, {
    position: 'absolute',
    left: `${offsetLeft}px`,
    top: `${offsetTop}px`,
    width: `${offsetWidth}px`,
    height: `${offsetHeight}px`,
    margin: '0',
    pointerEvents: 'none'
  })
}

function emitChars(count, target) {
  const next = shownChars.value.slice()
  for (let i = 0; i < count && next.length < target.length; i++) {
    next.push({ id: ++charSeq, ch: target[next.length] })
  }
  shownChars.value = next
}

/** 字在整行里的横向位置（offsetLeft 不受墨晕动画的 transform 影响） */
function charLeft(index) {
  return trackEl.value?.children[index]?.offsetLeft
}

/**
 * 识别文本变了：对齐已上屏的字。裁掉的开头直接去掉，前后两段对得上的字原样保留，
 * 只把中间被回改的那截当场换掉（新字照样墨晕），还没上屏的新字交给节拍器。
 */
function syncLine(target) {
  if (!target.length) {
    shownChars.value = []
    shift.value = 0
    paceBudget = 0
    return
  }
  const shown = shownChars.value
  const { drop, keep, tail, end } = reconcileLine(shown.map((c) => c.ch), target)
  // 锚点：保留下来的最靠右那段的第一个字。回改前后让它停在屏上原位——
  // 开头裁掉、中间换字的宽度差都记到左移量上，最新的字留在原处，画面不跳
  const anchor = tail ? shown.length - tail : keep ? drop : -1
  const anchorId = anchor >= 0 ? shown[anchor].id : null
  const anchorLeft = anchor >= 0 ? charLeft(anchor) : undefined
  shownChars.value = [
    ...shown.slice(drop, drop + keep),
    ...target.slice(keep, end - tail).map((ch) => ({ id: ++charSeq, ch })),
    ...shown.slice(shown.length - tail)
  ]
  if (anchorLeft !== undefined) {
    nextTick(() => {
      const left = charLeft(shownChars.value.findIndex((c) => c.id === anchorId))
      if (left !== undefined) {
        shift.value = Math.max(0, shift.value + left - anchorLeft)
      }
    })
  }
  if (prefersReducedMotion()) {
    emitChars(target.length, target)
  }
  startLineLoop()
}

function startLineLoop() {
  if (!rafId) {
    lastFrameAt = performance.now()
    rafId = requestAnimationFrame(lineFrame)
  }
}

function lineFrame(now) {
  const dt = Math.min(50, now - lastFrameAt)
  lastFrameAt = now
  const target = targetChars.value
  const step = paceStep(paceBudget, dt, target.length - shownChars.value.length)
  paceBudget = step.budget
  if (step.emit) {
    emitChars(step.emit, target)
  }
  const line = lineEl.value
  const track = trackEl.value
  const want = line && track ? Math.max(0, track.offsetWidth - line.clientWidth) : 0
  shift.value = prefersReducedMotion() ? want : glideToward(shift.value, want, dt)
  // 还有字没放完、或左移没追上（含刚放出的字下一帧才排好版）就继续
  rafId = target.length > shownChars.value.length || shift.value !== want || step.emit
    ? requestAnimationFrame(lineFrame)
    : 0
}

watch(targetChars, syncLine)

// 每次上报都是一次 IPC：数值没变不报
let reportedHit = ''
watch(hitRect, (rect) => {
  const key = JSON.stringify(rect)
  if (key !== reportedHit) {
    reportedHit = key
    window.floatingAPI?.setHitRect?.(rect)
  }
}, { immediate: true })

// 结果态悬停：暂停收起；移开后再停留一会儿（已开始淡出的不拦）
watch(hovered, (now) => {
  if (!view.value.hideAfterMs || leaving.value) {
    return
  }
  if (now) {
    clearTimers()
  } else {
    scheduleHide(HOVER_REHIDE_MS)
  }
})

watch(() => view.value.glyph, async (glyph) => {
  await nextTick()
  measureGlyphNudge(glyph)
}, { immediate: true })

// 同一次会话里换印章（聽→轉→成 / 敗）才按；新会话由胶囊入场动效负责
watch(() => [sessionKey.value, view.value.seal], ([key, seal], previous) => {
  if (previous && previous[0] === key && previous[1] !== seal && seal !== 'idle') {
    pressSeal(seal)
  }
})

onMounted(() => {
  window.floatingAPI?.onStateChange(applyState)
  window.floatingAPI?.onLevel?.(pushLevel)
  window.floatingAPI?.onNudge?.(nudgeSeal)
  window.floatingAPI?.onPointerOut?.(() => { hovered.value = false })
})

onUnmounted(() => {
  clearTimers()
  cancelAnimationFrame(rafId)
  observer?.disconnect()
  window.floatingAPI?.removeAllListeners('floating:state')
})
</script>
