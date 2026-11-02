<template>
  <!--
    看图器：全窗口遮罩 + 可缩放 / 拖动的大图。
    打开时按「适应窗口」显示；滚轮以光标为中心缩放，双击在「适应 / 原始尺寸」间切换，
    放大后可拖动；Esc、关闭按钮或点击图片外的空白处关闭。
  -->
  <Teleport to="body">
    <Transition name="image-viewer">
      <div
        v-if="src"
        ref="rootRef"
        class="image-viewer"
        role="dialog"
        aria-modal="true"
        :aria-label="title || '查看图片'"
      >
        <header class="image-viewer-bar">
          <div class="image-viewer-info">
            <span class="image-viewer-title">{{ title }}</span>
            <span v-if="meta" class="ui-meta ui-mono">{{ meta }}</span>
          </div>
          <div class="image-viewer-tools">
            <button class="ui-gbtn" type="button" title="缩小（-）" aria-label="缩小" :disabled="scale <= MIN_SCALE" @click="zoomBy(1 / ZOOM_STEP)">{{ G.zoomOut }}</button>
            <span class="image-viewer-zoom ui-mono" aria-live="polite">{{ Math.round(scale * 100) }}%</span>
            <button class="ui-gbtn" type="button" title="放大（+）" aria-label="放大" :disabled="scale >= MAX_SCALE" @click="zoomBy(ZOOM_STEP)">{{ G.zoomIn }}</button>
            <button class="ui-gbtn" type="button" title="适应窗口（0）" aria-label="适应窗口" :class="{ 'is-on': isFit }" @click="fitToStage">{{ G.fit }}</button>
            <button class="ui-gbtn" type="button" title="原始尺寸（1）" aria-label="原始尺寸" :class="{ 'is-on': scale === 1 && !isFit }" @click="actualSize">{{ G.actualSize }}</button>
            <span class="image-viewer-sep" aria-hidden="true"></span>
            <button v-if="copyable" class="ui-tbtn" type="button" @click="emit('copy')">
              <span class="ui-tbtn-glyph">{{ G.copy }}</span>复制
            </button>
            <button ref="closeRef" class="ui-gbtn" type="button" title="关闭（Esc）" aria-label="关闭" @click="close">{{ G.close }}</button>
          </div>
        </header>

        <div
          ref="stageRef"
          class="image-viewer-stage"
          :class="{ 'is-dragging': dragging, 'is-pannable': canPan }"
          @wheel.prevent="handleWheel"
          @click.self="close"
        >
          <img
            :src="src"
            :alt="title"
            class="image-viewer-img"
            :style="imageStyle"
            draggable="false"
            @load="handleLoad"
            @dblclick="toggleZoom"
            @pointerdown="startDrag"
            @pointermove="moveDrag"
            @pointerup="endDrag"
            @pointercancel="endDrag"
          >
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup>
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { ACTION_GLYPHS as G } from '@/components/common/glyphs.mjs'

const props = defineProps({
  // 有值即打开；置空即关闭
  src: { type: String, default: '' },
  title: { type: String, default: '' },
  meta: { type: String, default: '' },
  copyable: { type: Boolean, default: false }
})

const emit = defineEmits(['close', 'copy'])

const MIN_SCALE = 0.05
const MAX_SCALE = 8
const ZOOM_STEP = 1.25
// 适应窗口时四周留白，图片不贴边
const STAGE_PADDING = 32

const rootRef = ref(null)
const stageRef = ref(null)
const closeRef = ref(null)
const natural = ref({ width: 0, height: 0 })
const scale = ref(1)
const offset = ref({ x: 0, y: 0 })
const isFit = ref(true)
const dragging = ref(false)
let dragStart = null
let lastActiveElement = null

// 图片以舞台中心为原点，offset 是相对中心的平移
const imageStyle = computed(() => ({
  width: natural.value.width ? `${natural.value.width}px` : undefined,
  transform: `translate(calc(-50% + ${offset.value.x}px), calc(-50% + ${offset.value.y}px)) scale(${scale.value})`
}))

const canPan = computed(() => {
  const stage = stageRef.value
  if (!stage || !natural.value.width) {
    return false
  }
  return natural.value.width * scale.value > stage.clientWidth || natural.value.height * scale.value > stage.clientHeight
})

watch(() => props.src, async (src) => {
  if (src) {
    lastActiveElement = document.activeElement
    natural.value = { width: 0, height: 0 }
    scale.value = 1
    offset.value = { x: 0, y: 0 }
    isFit.value = true
    window.addEventListener('resize', handleResize)
    // 键盘监听挂在 window 上：拖动 / 点击图片后焦点会落到 body，挂在弹层上会收不到 Esc
    window.addEventListener('keydown', handleKeydown, true)
    await nextTick()
    closeRef.value?.focus()
  } else {
    window.removeEventListener('resize', handleResize)
    window.removeEventListener('keydown', handleKeydown, true)
    lastActiveElement?.focus?.()
    lastActiveElement = null
  }
}, { immediate: true })

onBeforeUnmount(() => {
  window.removeEventListener('resize', handleResize)
  window.removeEventListener('keydown', handleKeydown, true)
})

function fitScale() {
  const stage = stageRef.value
  const { width, height } = natural.value
  if (!stage || !width || !height) {
    return 1
  }
  const availableWidth = Math.max(stage.clientWidth - STAGE_PADDING * 2, 1)
  const availableHeight = Math.max(stage.clientHeight - STAGE_PADDING * 2, 1)
  // 小图不放大：适应窗口最多按原始尺寸显示
  return Math.min(availableWidth / width, availableHeight / height, 1)
}

function handleLoad(event) {
  natural.value = { width: event.target.naturalWidth, height: event.target.naturalHeight }
  fitToStage()
}

function handleResize() {
  if (isFit.value) {
    fitToStage()
  }
}

function fitToStage() {
  scale.value = fitScale()
  offset.value = { x: 0, y: 0 }
  isFit.value = true
}

function actualSize() {
  scale.value = 1
  offset.value = { x: 0, y: 0 }
  isFit.value = false
}

function toggleZoom(event) {
  if (isFit.value && fitScale() < 1) {
    // 放到原始尺寸，并让双击处保持在光标下
    zoomTo(1, event)
  } else {
    fitToStage()
  }
}

/** 以舞台内某点（默认中心）为锚缩放：锚点下的图像内容缩放前后不移动。 */
function zoomTo(nextScale, event) {
  const clamped = Math.min(Math.max(nextScale, MIN_SCALE), MAX_SCALE)
  const stage = stageRef.value
  if (!stage || clamped === scale.value) {
    return
  }
  let anchorX = 0
  let anchorY = 0
  if (event) {
    const rect = stage.getBoundingClientRect()
    anchorX = event.clientX - rect.left - rect.width / 2
    anchorY = event.clientY - rect.top - rect.height / 2
  }
  const ratio = clamped / scale.value
  offset.value = {
    x: anchorX - (anchorX - offset.value.x) * ratio,
    y: anchorY - (anchorY - offset.value.y) * ratio
  }
  scale.value = clamped
  isFit.value = false
}

function zoomBy(factor, event) {
  zoomTo(scale.value * factor, event)
}

function handleWheel(event) {
  // 触控板的连续小步长与鼠标滚轮的大步长统一成按幅度缩放
  const factor = Math.exp(-event.deltaY * 0.0015)
  zoomBy(factor, event)
}

function startDrag(event) {
  if (event.button !== 0 || !canPan.value) {
    return
  }
  dragging.value = true
  dragStart = { x: event.clientX, y: event.clientY, offsetX: offset.value.x, offsetY: offset.value.y }
  event.currentTarget.setPointerCapture(event.pointerId)
}

function moveDrag(event) {
  if (!dragging.value || !dragStart) {
    return
  }
  offset.value = {
    x: dragStart.offsetX + event.clientX - dragStart.x,
    y: dragStart.offsetY + event.clientY - dragStart.y
  }
}

function endDrag() {
  dragging.value = false
  dragStart = null
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    // 捕获阶段拦下，避免同时触发页面或其他弹窗的 Esc
    event.stopPropagation()
    event.preventDefault()
    close()
  } else if (event.key === '+' || event.key === '=') {
    zoomBy(ZOOM_STEP)
  } else if (event.key === '-') {
    zoomBy(1 / ZOOM_STEP)
  } else if (event.key === '0') {
    fitToStage()
  } else if (event.key === '1') {
    actualSize()
  } else if (event.key === 'Tab') {
    // 简单焦点陷阱：焦点只在工具栏按钮间循环
    const buttons = [...(rootRef.value?.querySelectorAll('button:not(:disabled)') || [])]
    if (!buttons.length) {
      return
    }
    const index = buttons.indexOf(document.activeElement)
    const next = event.shiftKey ? (index <= 0 ? buttons.length - 1 : index - 1) : (index + 1) % buttons.length
    event.preventDefault()
    buttons[next].focus()
  }
}

function close() {
  emit('close')
}
</script>

<style scoped>
.image-viewer {
  position: fixed;
  inset: 0;
  z-index: var(--z-popup);
  display: flex;
  flex-direction: column;
  background: var(--scrim);
  backdrop-filter: blur(8px);
}

.image-viewer-bar {
  flex: none;
  display: flex;
  align-items: center;
  gap: var(--s-4);
  height: 52px;
  padding: 0 var(--s-3) 0 var(--s-6);
  background: var(--surface-float);
  border-bottom: 1px solid var(--line-soft);
}

.image-viewer-info {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  min-width: 0;
}

.image-viewer-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--fs-sm);
  font-weight: 600;
  color: var(--text-1);
}

.image-viewer-tools {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 2px;
  flex: none;
}

.image-viewer-zoom {
  min-width: 48px;
  text-align: center;
  font-size: var(--fs-cap);
  color: var(--text-2);
  font-variant-numeric: tabular-nums;
}

.image-viewer-sep {
  width: 1px;
  height: 16px;
  margin: 0 var(--s-2);
  background: var(--line-soft-strong);
}

.image-viewer-stage {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  cursor: zoom-out;
}

.image-viewer-img {
  position: absolute;
  left: 50%;
  top: 50%;
  max-width: none;
  transform-origin: center;
  border-radius: var(--r-sm);
  box-shadow: var(--e-4);
  /* 透明 PNG 下垫一层底色，避免和遮罩糊在一起 */
  background: var(--bg-base);
  user-select: none;
  cursor: zoom-in;
  will-change: transform;
}

.image-viewer-stage.is-pannable .image-viewer-img { cursor: grab; }
.image-viewer-stage.is-dragging .image-viewer-img { cursor: grabbing; }

.image-viewer-enter-active,
.image-viewer-leave-active {
  transition: opacity var(--t-fast) var(--ease);
}

.image-viewer-enter-from,
.image-viewer-leave-to {
  opacity: 0;
}
</style>
