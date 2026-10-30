<template>
  <div class="floating-shell">
    <!--
      极简胶囊：一次会话里只变两次大小——
      起步只有一枚印章（与托盘同源）；出现第一个字后一口气展开到固定宽度并保持，
      之后字数增减、识别回改、聽→轉 都只换里面的内容；结束时收回成 成 / 敗 一枚印章。
      换印章时底色渐变、新旧字交叠（旧字缩小淡出，新字像盖章一样落下），印章本身轻按一下；
      文字与声纹 / 处理点先淡出，胶囊再收拢，不会一下子跳变。
      始终单行，超出宽度后裁掉开头（左侧渐隐），最新的字留在右边。
      胶囊本身不参与排版：内层 .content 排好，胶囊量出它的大小后用过渡追上去。
    -->
    <section
      :key="sessionKey"
      class="capsule"
      :class="{ sized, leaving: leaving || view.seal === 'idle' }"
      :data-seal="view.seal"
      :style="capsuleStyle"
      :title="view.detail || undefined"
      role="status"
      aria-live="polite"
      @mouseenter="hovered = true"
      @mouseleave="hovered = false"
    >
      <div :ref="observeContent" class="content" :class="{ expanded }">
        <span ref="sealEl" class="seal" aria-hidden="true">
          <Transition name="glyph">
            <span :key="view.glyph" class="seal-glyph">
              <span class="seal-ink" :style="{ transform: `translateY(${glyphNudges[view.glyph] || 0}px)` }">{{ view.glyph }}</span>
            </span>
          </Transition>
        </span>
        <span class="sr-only">{{ SEAL_LABELS[view.seal] }}{{ view.detail ? `：${view.detail}` : '' }}</span>

        <Transition name="line">
          <span v-if="expanded" ref="lineEl" class="line" :class="{ clipped, dim: view.dim }"><span>{{ view.text }}</span></span>
        </Transition>

        <!-- 声纹与处理点占同一个位置：先淡出旧的再淡入新的，宽度不会两个叠在一起 -->
        <Transition name="slot" mode="out-in">
          <span v-if="showWave" key="wave" class="slot wave" :class="{ quiet: !voice.loud }" aria-hidden="true">
            <span v-for="(height, index) in waveHeights" :key="index" class="wave-bar" :style="{ height: `${height}px` }"></span>
          </span>
          <span v-else-if="view.busy" key="busy" class="slot busy" aria-hidden="true">
            <span></span><span></span><span></span>
          </span>
        </Transition>

        <button v-if="showCancel" type="button" class="cancel" aria-label="取消" title="取消" @click="cancel">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
        </button>
      </div>
    </section>
  </div>
</template>

<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { deriveFloatingView, nextVoiceState, sealGlyphNudge } from './floatingView.mjs'

const SEAL_LABELS = { idle: '待命', listening: '录音中', converting: '处理中', success: '已完成', failure: '失败' }
const WAVE_BARS = 7
/** 安静时声纹收成的小点高度（px） */
const WAVE_REST = 3
/** 与 floating.css 的 .capsule.leaving 过渡时长一致：淡出完再真正隐藏窗口 */
const LEAVE_MS = 200

const state = ref({ phase: 'idle' })
const levels = ref(Array(WAVE_BARS).fill(0))
const size = ref({ width: 0, height: 0 })
const sized = ref(false)
const sessionKey = ref(0)
const leaving = ref(false)
const hovered = ref(false)
const clipped = ref(false)
const lineEl = ref(null)
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
/** 本次会话最近一次识别 / 优化出的文字：进入「轉」时沿用它撑住胶囊 */
const lastText = ref('')
let hideTimer = null
let leaveTimer = null
let observer = null

const view = computed(() => deriveFloatingView(state.value, { previousText: lastText.value }))
const expanded = computed(() => wide.value && view.value.active)
// 没展开时 ✕ 收起来保持最小，悬停才露出；展开后常驻
const showCancel = computed(() => view.value.cancellable && (expanded.value || hovered.value))
// 声纹：录音中音量第一次过门限时出现（不必等出字）
const showWave = computed(() => voiced.value && view.value.seal === 'listening')
// 音量 0..1 偏小，开方放大低音量段的起伏；安静时收成一排小点
const waveHeights = computed(() => levels.value.map((level) => (
  voice.value.loud ? Math.round(4 + Math.sqrt(Math.min(1, level)) * 18) : WAVE_REST
)))
const capsuleStyle = computed(() => (
  size.value.width ? { width: `${size.value.width}px`, height: `${size.value.height}px` } : null
))

function observeContent(el) {
  observer?.disconnect()
  if (!el) {
    return
  }
  observer = new ResizeObserver(() => {
    // 胶囊有 1px 描边（border-box），比内容各大 2px
    size.value = { width: el.offsetWidth + 2, height: el.offsetHeight + 2 }
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

function clearTimers() {
  clearTimeout(hideTimer)
  clearTimeout(leaveTimer)
  hideTimer = null
  leaveTimer = null
}

function applyState(payload = {}) {
  const wasActive = view.value.active && !leaving.value
  state.value = payload || {}
  const next = view.value
  clearTimers()

  if (next.active && !wasActive) {
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
    if (!next.dim) {
      lastText.value = next.text
    }
  }

  if (next.hideAfterMs) {
    hideTimer = setTimeout(() => {
      leaving.value = true
      leaveTimer = setTimeout(() => window.floatingAPI?.close(), LEAVE_MS)
    }, next.hideAfterMs)
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

// 开头被裁掉时左侧渐隐，提示前面还有内容（文字比可视宽度长即为被裁）
watch(() => view.value.text, async () => {
  await nextTick()
  const el = lineEl.value
  clipped.value = Boolean(el?.firstElementChild && el.firstElementChild.offsetWidth > el.clientWidth + 1)
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
})

onUnmounted(() => {
  clearTimers()
  observer?.disconnect()
  window.floatingAPI?.removeAllListeners('floating:state')
})
</script>
