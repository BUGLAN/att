<template>
  <!-- 本地引擎的模型状态行（极简：只有状态点、标题与一行说明，不放按钮；加载/重试靠点上方的引擎卡片） -->
  <div class="model-status" :class="`is-${view.state}`" role="status" aria-live="polite">
    <div class="model-status-head">
      <span class="model-status-dot" aria-hidden="true"></span>
      <div class="model-status-body">
        <span class="model-status-title">{{ view.title }}</span>
        <span class="model-status-desc" :class="{ 'is-mono': view.state === 'ready' }">{{ hint.summary }}</span>
      </div>
    </div>

    <!-- 加载中：底边一道扫过的金色光带（与选中卡片同款） -->
    <div v-if="view.state === 'loading'" class="model-status-track" aria-hidden="true">
      <span class="model-status-bar"></span>
    </div>

    <div v-if="hint.commands.length || hint.notes.length || view.tip" class="model-status-extra">
      <pre v-if="hint.commands.length" class="model-status-code"><code>{{ hint.commands.join('\n') }}</code></pre>
      <p v-for="note in hint.notes" :key="note" class="model-status-note">{{ note }}</p>
      <p v-if="view.tip" class="model-status-note">{{ view.tip }}</p>
    </div>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import { splitHint } from './asrEngineStatus.mjs'

defineOptions({ name: 'LocalModelStatus' })

const props = defineProps({
  // asrEngineStatus.localEngineView() 的返回值
  view: {
    type: Object,
    required: true
  }
})

const hint = computed(() => splitHint(props.view.message))
</script>

<style scoped>
.model-status {
  --status-color: var(--text-3);
  position: relative;
  margin-bottom: var(--s-3);
  padding: var(--s-3) var(--s-4);
  background: var(--bg-card);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  overflow: hidden;
}

.model-status.is-loading { --status-color: var(--accent); }
.model-status.is-ready { --status-color: var(--ok); }
.model-status.is-missing { --status-color: var(--warn); }
.model-status.is-failed { --status-color: var(--err); }

.model-status-head {
  display: flex;
  align-items: flex-start;
  gap: var(--s-3);
}

/* 状态点与标题首行对齐 */
.model-status-dot {
  flex: none;
  width: 6px;
  height: 6px;
  margin-top: 8px;
  border-radius: 50%;
  background: var(--status-color);
}

.model-status.is-loading .model-status-dot {
  animation: model-status-pulse 1.4s var(--ease) infinite;
}

.model-status-body {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.model-status-title {
  font-size: var(--fs-base);
  font-weight: 500;
  color: var(--text-1);
}

.model-status-desc {
  font-size: var(--fs-cap);
  line-height: 1.55;
  color: var(--text-3);
  overflow-wrap: anywhere;
}

.model-status-desc.is-mono,
.model-status-code {
  font-family: var(--font-mono, ui-monospace, SFMono-Regular, Menlo, monospace);
}

.model-status-track {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 2px;
  background: var(--accent-faint);
  overflow: hidden;
}

.model-status-bar {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 35%;
  background: linear-gradient(90deg, transparent, var(--accent), transparent);
  animation: model-status-sweep 1.4s var(--ease) infinite;
}

/* 说明与命令和标题左对齐（让出状态点 6px + 间距） */
.model-status-extra {
  margin-left: calc(6px + var(--s-3));
}

.model-status-code {
  margin: var(--s-2) 0 0;
  padding: var(--s-2) var(--s-3);
  overflow-x: auto;
  font-size: var(--fs-cap);
  line-height: 1.7;
  color: var(--text-2);
  white-space: pre;
  background: var(--bg-base);
  border-radius: var(--r-sm);
  user-select: text;
}

.model-status-note {
  margin: var(--s-1) 0 0;
  font-size: var(--fs-cap);
  color: var(--text-3);
}

@keyframes model-status-sweep {
  from { left: -35%; }
  to { left: 100%; }
}

@keyframes model-status-pulse {
  50% { opacity: 0.35; }
}

@media (prefers-reduced-motion: reduce) {
  .model-status.is-loading .model-status-dot { animation: none; }
  .model-status-bar { animation: none; left: 0; width: 100%; opacity: 0.5; }
}
</style>
