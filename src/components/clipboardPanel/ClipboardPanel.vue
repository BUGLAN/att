<template>
  <!--
    剪贴板悬浮框（原型 B「横卷」）：从屏幕底边升起的一条卡片带，内容看得最全（图片直接是缩略图）。
    记录分段加载：每次唤起取第一段，翻到快到头时取下一段；搜索与筛选同样交给后端分段取。
    不显示系统滚动条：两侧渐隐暗示还有内容（滚到头那一侧不再渐隐），头部「当前 / 总数」给出位置，滚轮竖滚转横滚。
    搜索框常驻焦点；←→ 选卡片、↵ 按设置粘贴或复制、⌘↵ 换另一种、⌘1–9 直接选第几张、Tab 切筛选、Esc 先清搜索再收起。
    拖上边缘调高度，拖左右边缘 / 上方两角调宽度（水平居中，左右对称伸缩）；按住头部空白处上下拖动改位置，双击回到底部；都在松手后记住。
  -->
  <section class="panel" :class="{ resizing, moving }" role="dialog" aria-label="剪贴板" @keydown="handleKeydown">
    <span
      v-for="handle in RESIZE_HANDLES"
      :key="handle.name"
      class="panel-resize"
      :class="`panel-resize-${handle.name}`"
      aria-hidden="true"
      @pointerdown="startResize($event, handle)"
    ></span>
    <!-- 头部三栏：左 印章 + 筛选，中 搜索（视线落在屏幕中线上），右 按键提示 -->
    <header
      class="panel-head"
      title="按住空白处上下拖动，双击回到底部"
      @pointerdown="startMove"
      @dblclick="resetPosition"
    >
      <div class="panel-head-side">
        <span class="ui-seal panel-seal" aria-hidden="true"><span class="ui-seal-ink">{{ NAV_GLYPHS.clipboard }}</span></span>
        <nav class="ui-tabs panel-tabs" aria-label="筛选">
          <button
            v-for="f in PANEL_FILTERS"
            :key="f.value"
            type="button"
            tabindex="-1"
            :class="{ 'is-on': filter === f.value }"
            @click="filter = f.value"
          >{{ f.label }}</button>
        </nav>
      </div>
      <label class="ui-search panel-search">
        <span class="ui-search-glyph">{{ G.search }}</span>
        <input
          ref="searchInput"
          v-model="query"
          placeholder="搜索"
          spellcheck="false"
          autocomplete="off"
          aria-label="搜索剪贴板"
        >
      </label>
      <span v-if="errorText" class="ui-meta panel-hint panel-error"><span class="ui-dot err"></span>{{ errorText }}</span>
      <span v-else class="ui-meta panel-hint">
        <span v-if="total" class="panel-count ui-mono"><b>{{ index + 1 }}</b> / {{ total }}</span>
        <kbd class="ui-kbd">←→</kbd>选择
        <kbd class="ui-kbd">↵</kbd>{{ commitActionLabel(primaryAction) }}
        <kbd class="ui-kbd">{{ modKey }}↵</kbd>{{ commitActionLabel(alternateAction) }}
        <kbd class="ui-kbd">esc</kbd>收起
      </span>
    </header>

    <div
      ref="stripEl"
      class="panel-strip"
      :class="{ 'at-start': edges.start, 'at-end': edges.end }"
      @scroll="handleScroll"
      @wheel="handleWheel"
    >
      <button
        v-for="(item, i) in items"
        :key="item.id"
        class="panel-card"
        :class="{ 'is-on': i === index }"
        type="button"
        tabindex="-1"
        :data-index="i"
        :title="getClipboardDisplayTitle(item)"
        @mouseenter="index = i"
        @click="commit(item, false)"
      >
        <span class="panel-card-head">
          <span class="panel-card-glyph">{{ clipTypeGlyph(item.item_type) }}</span>
          <span>{{ getClipboardTypeLabel(item.item_type) }}</span>
          <span v-if="item.is_pinned" class="panel-card-mark" title="已置顶">{{ G.pin }}</span>
          <span v-if="item.is_favorite" class="panel-card-mark" title="已收藏">{{ G.favorite }}</span>
          <kbd v-if="i < 9" class="ui-kbd panel-card-num">{{ modKey }}{{ i + 1 }}</kbd>
        </span>

        <span v-if="isClipboardImageItem(item)" class="panel-card-image">
          <img v-if="previews[item.id]" :src="previews[item.id]" :alt="getClipboardDisplayTitle(item)">
          <span v-else class="ui-meta">{{ G.file }}</span>
        </span>
        <span v-else-if="item.item_type === 'link'" class="panel-card-body panel-card-link">
          <strong>{{ splitLink(item.content).host }}</strong>
          <span class="ui-mono">{{ splitLink(item.content).rest }}</span>
        </span>
        <span v-else-if="item.item_type === 'file'" class="panel-card-body panel-card-file">
          <strong>{{ panelFileName(item, getClipboardDisplayTitle(item)) }}</strong>
          <span class="ui-mono">{{ item.content }}</span>
        </span>
        <span v-else class="panel-card-body" :class="{ 'ui-mono panel-card-code': item.item_type === 'code' }">{{ item.content }}</span>

        <span class="panel-card-foot ui-meta">{{ detailText(item) }}</span>
      </button>

      <div v-if="items.length && items.length < total" class="panel-card panel-card-more" aria-hidden="true">
        <span class="ui-meta"><span class="ui-dot blue pulse"></span>{{ loading ? '加载中…' : `还有 ${total - items.length} 条` }}</span>
      </div>

      <div v-if="!items.length" class="ui-empty panel-empty">
        <span class="ui-glyph-tile ui-glyph-tile--lg">{{ NAV_GLYPHS.clipboard }}</span>
        <span>{{ emptyText }}</span>
      </div>
    </div>
  </section>
</template>

<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { api } from '@/api/client'
import { useElectron } from '@/composables/useElectron'
import { ACTION_GLYPHS as G, NAV_GLYPHS, clipTypeGlyph } from '@/components/common/glyphs.mjs'
import {
  getClipboardDisplayTitle,
  getClipboardTypeLabel,
  isClipboardImageItem
} from '@/utils/clipboardPresentation'
import {
  PANEL_FILTERS,
  appendPage,
  buildPanelQuery,
  commitActionLabel,
  cycleFilter,
  moveIndex,
  normalizePage,
  panelFileName,
  panelKeyIntent,
  relativeTime,
  resolveCommitAction,
  shouldLoadMore,
  splitLink
} from './clipboardPanelModel.mjs'

const { clipboard, clipboardPanel, onClipboardChanged, platform } = useElectron()
const modKey = platform === 'darwin' ? '⌘' : 'Ctrl+'

// 已加载的段（按后端顺序）与筛选后的总条数
const items = ref([])
const total = ref(0)
const query = ref('')
const filter = ref('all')
const index = ref(0)
const loading = ref(false)
const errorText = ref('')
const context = ref({ enter_action: 'paste', hotkey_display: '' })
const previews = reactive({})
const searchInput = ref(null)
const stripEl = ref(null)
let committing = false

// 拖动手柄：top 为 true 时改高度；side 为 -1 / 1 时改宽度（左右对称，所以位移按两倍算）
const RESIZE_HANDLES = [
  { name: 'top', top: true, side: 0 },
  { name: 'left', top: false, side: -1 },
  { name: 'right', top: false, side: 1 },
  { name: 'top-left', top: true, side: -1 },
  { name: 'top-right', top: true, side: 1 }
]
/** 窗口四周的透明阴影边（与 clipboard-panel.css 的 #app padding、clipboard_panel.rs 的 SHADOW_EDGE 一致） */
const SHADOW_EDGE = 16
const resizing = ref(false)
const moving = ref(false)
// 两侧渐隐：滚到头的那一侧不渐隐
const edges = reactive({ start: true, end: true })
// 每次重新查询加一：晚到的旧响应（上一次的搜索词 / 筛选）直接丢弃
let requestSeq = 0
let queryTimer = 0

const current = computed(() => items.value[index.value] || null)
const primaryAction = computed(() => resolveCommitAction(context.value.enter_action))
const alternateAction = computed(() => resolveCommitAction(context.value.enter_action, true))
const emptyText = computed(() => {
  if (loading.value) return '正在读取剪贴板…'
  if (query.value || filter.value !== 'all') return '没有匹配的条目'
  return '还没有剪贴板记录'
})

function detailText(item) {
  const metadata = item.metadata || {}
  const size = isClipboardImageItem(item)
    ? (metadata.width && metadata.height ? `${metadata.width} × ${metadata.height}` : '')
    : (item.content_size ? `${item.content_size} 字符` : '')
  return [size, relativeTime(item.updated_at || item.created_at)].filter(Boolean).join(' · ')
}

async function fetchPage(offset, seq) {
  loading.value = true
  try {
    const page = normalizePage(await api.getClipboardItems(buildPanelQuery({ filter: filter.value, query: query.value, offset })))
    if (seq !== requestSeq) return null
    errorText.value = ''
    return page
  } catch (error) {
    if (seq === requestSeq) {
      console.error('[ClipboardPanel] 读取剪贴板失败:', error)
      errorText.value = '读取剪贴板失败'
    }
    return null
  } finally {
    if (seq === requestSeq) loading.value = false
  }
}

/** 重新查询（唤起 / 搜索 / 切筛选）：只取第一段，回到开头 */
async function loadFirstPage() {
  const seq = ++requestSeq
  const page = await fetchPage(0, seq)
  if (!page) return
  items.value = page.items
  total.value = page.total
  index.value = 0
  await nextTick()
  if (stripEl.value) stripEl.value.scrollLeft = 0
  updateEdges()
  maybeLoadMore()
}

/** 取下一段，接在已加载的后面 */
async function loadMore() {
  const seq = requestSeq
  const page = await fetchPage(items.value.length, seq)
  if (!page) return
  items.value = appendPage(items.value, page.items)
  total.value = page.total
  await nextTick()
  updateEdges()
}

function maybeLoadMore() {
  const el = stripEl.value
  const need = shouldLoadMore({
    loaded: items.value.length,
    total: total.value,
    loading: loading.value,
    index: index.value,
    scrollLeft: el?.scrollLeft || 0,
    clientWidth: el?.clientWidth || 0,
    scrollWidth: el?.scrollWidth || 0
  })
  if (need) void loadMore()
}

function updateEdges() {
  const el = stripEl.value
  if (!el) return
  edges.start = el.scrollLeft <= 1
  edges.end = el.scrollLeft + el.clientWidth >= el.scrollWidth - 1
}

function handleScroll() {
  updateEdges()
  maybeLoadMore()
}

// 系统滚动条藏起来后，多数人仍会下意识上下滚：竖向滚轮转成横向
function handleWheel(event) {
  if (Math.abs(event.deltaY) > Math.abs(event.deltaX)) {
    event.preventDefault()
    stripEl.value.scrollLeft += event.deltaY
  }
}

// 只滚卡片带本身：scrollIntoView 会连外层容器一起滚，把整条面板顶出原位。
// 直接定位、不做平滑滚动：连按 ←→ 时平滑动画会排队滞后，换了搜索结果后还会接着把卡片带滚走
function revealCard(i) {
  const el = stripEl.value
  const card = el?.querySelector(`[data-index="${i}"]`)
  if (!card) return
  // 留出渐隐的宽度，选中的卡片不落在半透明区里
  const margin = 56
  const left = card.offsetLeft - margin
  const right = card.offsetLeft + card.offsetWidth + margin - el.clientWidth
  if (el.scrollLeft > left) el.scrollLeft = Math.max(0, left)
  else if (el.scrollLeft < right) el.scrollLeft = right
}

async function loadContext() {
  try {
    context.value = { ...context.value, ...(await clipboardPanel.getContext()) }
  } catch (error) {
    console.warn('[ClipboardPanel] 读取设置失败:', error)
  }
}

/** 每次唤起：清空搜索与筛选、回到开头、重新取第一段与设置 */
async function resetForShow() {
  // 搜索词 / 筛选有变化时由下面的 watch 负责重新查询，这里只在没变时自己查，免得一次唤起查两遍
  const unchanged = !query.value && filter.value === 'all'
  query.value = ''
  filter.value = 'all'
  index.value = 0
  errorText.value = ''
  committing = false
  await nextTick()
  searchInput.value?.focus()
  await Promise.all([unchanged ? loadFirstPage() : null, loadContext()])
}

// 图片卡片直接显示缩略图：按条目缓存成 blob 地址，只取前面一屏多的，往后翻到再取
async function ensurePreview(item) {
  if (!item || !isClipboardImageItem(item) || previews[item.id]) return
  try {
    const url = await api.getClipboardItemImageUrl(item.id, item.updated_at || item.created_at || '')
    const response = await fetch(url)
    if (!response.ok) throw new Error(`HTTP ${response.status}`)
    previews[item.id] = URL.createObjectURL(await response.blob())
  } catch (error) {
    console.warn('[ClipboardPanel] 缩略图加载失败:', error)
  }
}

async function commit(item, alternate) {
  if (!item || committing) return
  committing = true
  const action = resolveCommitAction(context.value.enter_action, alternate)
  try {
    if (isClipboardImageItem(item) && item.metadata?.image_path) {
      await clipboard.writeImage({ imagePath: item.metadata.image_path, imageToken: item.content || '' })
    } else {
      await clipboard.writeText(item.content || '')
    }
  } catch (error) {
    console.error('[ClipboardPanel] 写入剪贴板失败:', error)
    errorText.value = '写入剪贴板失败'
    committing = false
    return
  }
  try {
    await clipboardPanel.finish(action === 'paste')
  } catch (error) {
    // 面板已收起：粘贴失败（多为缺「辅助功能」权限）时内容仍在剪贴板里，用户可以手动粘贴
    console.error('[ClipboardPanel] 自动粘贴失败:', error)
  } finally {
    committing = false
  }
}

function startResize(event, handle) {
  if (event.button !== 0) return
  event.preventDefault()
  const target = event.currentTarget
  target.setPointerCapture(event.pointerId)
  resizing.value = true
  const start = {
    x: event.screenX,
    y: event.screenY,
    width: window.innerWidth - 2 * SHADOW_EDGE,
    height: window.innerHeight - 2 * SHADOW_EDGE
  }
  let pending = null
  let frame = 0
  const sizeAt = (e) => ({
    width: start.width + 2 * handle.side * (e.screenX - start.x),
    height: handle.top ? start.height - (e.screenY - start.y) : start.height
  })
  // 每帧最多改一次窗口尺寸：pointermove 比刷新率密，每次都改会卡
  const flush = () => {
    frame = 0
    if (pending) void clipboardPanel.resize(pending.width, pending.height, false)
  }
  const onMove = (e) => {
    pending = sizeAt(e)
    if (!frame) frame = requestAnimationFrame(flush)
  }
  const onUp = (e) => {
    cancelAnimationFrame(frame)
    target.removeEventListener('pointermove', onMove)
    target.removeEventListener('pointerup', onUp)
    target.removeEventListener('pointercancel', onUp)
    resizing.value = false
    const size = sizeAt(e)
    void clipboardPanel.resize(size.width, size.height, true)
    searchInput.value?.focus()
  }
  target.addEventListener('pointermove', onMove)
  target.addEventListener('pointerup', onUp)
  target.addEventListener('pointercancel', onUp)
}

// 头部里这些是可操作的控件，不当作拖动把手
function isControl(target) {
  return Boolean(target.closest('input, button, .ui-search'))
}

// 按住头部空白处上下拖动：只改离屏幕底边的距离，水平始终居中
function startMove(event) {
  if (event.button !== 0 || isControl(event.target)) return
  event.preventDefault()
  const target = event.currentTarget
  target.setPointerCapture(event.pointerId)
  moving.value = true
  const startY = event.screenY
  const startOffset = Number(context.value.bottom_offset) || 0
  let pending = null
  let frame = 0
  const offsetAt = (e) => Math.max(0, startOffset - (e.screenY - startY))
  const flush = () => {
    frame = 0
    if (pending != null) void clipboardPanel.move(pending, false)
  }
  const onMove = (e) => {
    pending = offsetAt(e)
    if (!frame) frame = requestAnimationFrame(flush)
  }
  const onUp = async (e) => {
    cancelAnimationFrame(frame)
    target.removeEventListener('pointermove', onMove)
    target.removeEventListener('pointerup', onUp)
    target.removeEventListener('pointercancel', onUp)
    moving.value = false
    const offset = offsetAt(e)
    searchInput.value?.focus()
    if (offset === startOffset) return
    const result = await clipboardPanel.move(offset, true)
    context.value = { ...context.value, bottom_offset: result?.bottom_offset ?? offset }
  }
  target.addEventListener('pointermove', onMove)
  target.addEventListener('pointerup', onUp)
  target.addEventListener('pointercancel', onUp)
}

async function resetPosition(event) {
  if (isControl(event.target)) return
  await clipboardPanel.resetPosition()
  context.value = { ...context.value, bottom_offset: 0 }
  searchInput.value?.focus()
}

function handleKeydown(event) {
  const intent = panelKeyIntent(event, { hasQuery: Boolean(query.value) })
  if (!intent) return
  event.preventDefault()
  switch (intent.type) {
    case 'move':
      index.value = moveIndex(index.value, intent.delta, items.value.length)
      break
    case 'filter':
      filter.value = cycleFilter(filter.value, intent.delta)
      break
    case 'commit':
      void commit(current.value, intent.alternate)
      break
    case 'quick':
      void commit(items.value[intent.index], false)
      break
    case 'clear-query':
      query.value = ''
      break
    case 'close':
      void clipboardPanel.hide()
      break
  }
}

// 切筛选立即查；打字停 150ms 再查，不每个字符打一次接口
watch([query, filter], ([, nextFilter], [, previousFilter]) => {
  clearTimeout(queryTimer)
  if (nextFilter !== previousFilter) {
    void loadFirstPage()
  } else {
    queryTimer = setTimeout(() => void loadFirstPage(), 150)
  }
})

const PREVIEW_AHEAD = 12

watch([items, index], ([list, selected]) => {
  for (const item of list.slice(0, selected + PREVIEW_AHEAD)) {
    void ensurePreview(item)
  }
})

watch(index, () => {
  void nextTick(() => {
    revealCard(index.value)
    maybeLoadMore()
  })
})

let resizeObserver = null

onMounted(() => {
  clipboardPanel.onShow(() => void resetForShow())
  // 新复制进来：还停在开头时刷新第一段；已经往后翻了就不打断，下次唤起再刷新
  onClipboardChanged(() => {
    if (index.value === 0 && edges.start && !query.value) void loadFirstPage()
  })
  resizeObserver = new ResizeObserver(updateEdges)
  if (stripEl.value) resizeObserver.observe(stripEl.value)
  void resetForShow()
})

onBeforeUnmount(() => {
  clearTimeout(queryTimer)
  resizeObserver?.disconnect()
  for (const url of Object.values(previews)) URL.revokeObjectURL(url)
})
</script>

<style scoped>
.panel {
  position: relative;
  width: 100%;
  height: 100%;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--line-soft-strong);
  border-radius: var(--r-lg);
  background: var(--surface-float);
  backdrop-filter: blur(24px) saturate(1.2);
  -webkit-backdrop-filter: blur(24px) saturate(1.2);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45), var(--edge-hi);
  overflow: hidden;
  animation: panel-rise 220ms cubic-bezier(0.2, 0.9, 0.25, 1);
}

@keyframes panel-rise { from { opacity: 0; transform: translateY(20px); } }

/* 拖动手柄：贴着面板边缘的透明细条，悬停时露出一条金色细线提示可拖 */
.panel-resize { position: absolute; z-index: 2; }
.panel-resize-top { top: 0; left: 14px; right: 14px; height: 6px; cursor: ns-resize; }
.panel-resize-left { top: 14px; bottom: 14px; left: 0; width: 6px; cursor: ew-resize; }
.panel-resize-right { top: 14px; bottom: 14px; right: 0; width: 6px; cursor: ew-resize; }
.panel-resize-top-left { top: 0; left: 0; width: 14px; height: 14px; cursor: nwse-resize; }
.panel-resize-top-right { top: 0; right: 0; width: 14px; height: 14px; cursor: nesw-resize; }

.panel-resize-top::after {
  content: '';
  position: absolute;
  top: 2px;
  left: 50%;
  width: 36px;
  height: 3px;
  border-radius: 2px;
  background: var(--line-soft-strong);
  transform: translateX(-50%);
  transition: background-color var(--t-fast) var(--ease), width var(--t-fast) var(--ease);
}

.panel-resize-top:hover::after,
.panel.resizing .panel-resize-top::after { width: 56px; background: var(--accent-line-strong); }

/* 拖动中关掉卡片的过渡与悬停响应，免得跟着抖 */
.panel.resizing .panel-card { transition: none; pointer-events: none; }

.panel-head {
  cursor: grab;
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(220px, 360px) minmax(0, 1fr);
  align-items: center;
  gap: 16px;
  padding: 10px 14px 0;
}

.panel-head-side { display: flex; align-items: center; gap: 12px; min-width: 0; overflow: hidden; }
.panel-seal { width: 26px; height: 26px; font-size: 15px; }
.panel.moving .panel-head { cursor: grabbing; }
.panel-tabs > button { cursor: pointer; }
.panel-search { cursor: text; height: 32px; background: var(--white-veil); border-color: var(--line-soft); }
.panel-search input { user-select: text; }
.panel-tabs { flex: none; border-bottom: 0; gap: 4px 14px; flex-wrap: nowrap; }
.panel-tabs > button { padding: 6px 0; font-size: var(--fs-cap); white-space: nowrap; }
.panel-tabs > button.is-on::after { bottom: 0; }
.panel-hint { justify-self: end; gap: 4px; overflow: hidden; }
.panel-hint .ui-kbd { margin-left: 6px; }
.panel-error { color: var(--err); }

/* 卡片高度跟着面板高度走（拖动可调），宽度按 6:5 等比 */
.panel-strip {
  --fade-l: 48px;
  --fade-r: 48px;
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  align-items: stretch;
  gap: 10px;
  padding: 10px 14px 12px;
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
  /* 两侧渐隐：卡片在边缘「卷进去」，暗示那边还有；滚到头那一侧不渐隐 */
  -webkit-mask-image: linear-gradient(90deg, transparent 0, #000 var(--fade-l), #000 calc(100% - var(--fade-r)), transparent 100%);
  mask-image: linear-gradient(90deg, transparent 0, #000 var(--fade-l), #000 calc(100% - var(--fade-r)), transparent 100%);
}

.panel-strip::-webkit-scrollbar { display: none; }
.panel-strip.at-start { --fade-l: 0px; }
.panel-strip.at-end { --fade-r: 0px; }

.panel-count { margin-right: 8px; }
.panel-count b { font-weight: 600; color: var(--accent); }

.panel-card-more { place-items: center; justify-content: center; align-items: center; border-style: dashed; cursor: default; }

.panel-card {
  flex: none;
  height: 100%;
  aspect-ratio: 6 / 5;
  display: flex;
  flex-direction: column;
  padding: 0;
  overflow: hidden;
  border: 1px solid var(--line-soft-strong);
  border-radius: var(--r-md);
  background: var(--grad-card);
  color: var(--text-2);
  font: inherit;
  text-align: left;
  cursor: pointer;
  transition: transform var(--t-fast) var(--ease), border-color var(--t-fast) var(--ease), box-shadow var(--t-fast) var(--ease);
}

.panel-card.is-on {
  border-color: var(--accent-line-strong);
  color: var(--text-1);
  transform: translateY(-3px);
  box-shadow: 0 0 0 3px var(--accent-halo), var(--e-2);
}

.panel-card-head {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: none;
  height: 30px;
  padding: 0 10px;
  border-bottom: 1px solid var(--line-soft);
  background: var(--scrim-veil);
  font-size: var(--fs-cap);
  color: var(--text-3);
}

.panel-card-glyph,
.panel-card-mark {
  font-family: var(--font-seal);
  font-weight: 900;
  font-size: 13px;
  line-height: 1;
}

.panel-card-glyph { color: var(--text-2); }
.panel-card.is-on .panel-card-glyph,
.panel-card-mark { color: var(--accent); }
.panel-card-num { margin-left: auto; }

/* 卡片高度可调，正文不按行数截断：放得下多少显示多少，底部渐隐 */
.panel-card-body {
  display: block;
  flex: 1;
  min-height: 0;
  padding: 10px;
  overflow: hidden;
  font-size: var(--fs-sm);
  line-height: 1.55;
  word-break: break-all;
  white-space: pre-wrap;
  -webkit-mask-image: linear-gradient(180deg, #000 72%, transparent);
  mask-image: linear-gradient(180deg, #000 72%, transparent);
}

.panel-card-code { font-size: 11.5px; }

.panel-card-link,
.panel-card-file {
  display: grid;
  align-content: start;
  gap: 4px;
}

.panel-card-link strong,
.panel-card-file strong {
  font-weight: 600;
  color: var(--text-1);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.panel-card-link .ui-mono,
.panel-card-file .ui-mono {
  font-size: 11.5px;
  color: var(--text-3);
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.panel-card-image {
  flex: 1;
  min-height: 0;
  display: grid;
  place-items: center;
  margin: 8px;
  overflow: hidden;
  border: 1px solid var(--line-soft);
  border-radius: var(--r-sm);
  background: var(--scrim-veil);
}

.panel-card-image img { width: 100%; height: 100%; object-fit: cover; }
.panel-card-foot { flex: none; padding: 0 10px 8px; }
.panel-empty { width: 100%; padding: 0; }
</style>
