<template>
  <section class="result-card">
    <header class="result-header">
      <div>
        <span class="result-kicker">{{ result.action_type || 'translate' }}</span>
        <h3 class="result-title">{{ titleText }}</h3>
      </div>
      <span v-if="result.saved_to_wordbook" class="result-saved">已加入生词本</span>
    </header>

    <div class="result-body">
      <p v-if="result.translation" class="result-translation">{{ result.translation }}</p>
      <p v-if="result.answer" class="result-answer">{{ result.answer }}</p>

      <ul v-if="result.explanations?.length" class="result-list">
        <li v-for="(explanation, index) in result.explanations" :key="`${result.action_type}-${index}`">
          {{ explanation }}
        </li>
      </ul>

      <div v-if="showExamples && result.examples?.length" class="result-examples">
        <strong>例句</strong>
        <p v-for="(example, index) in result.examples" :key="`${result.action_type}-example-${index}`">
          {{ example }}
        </p>
      </div>
    </div>
  </section>
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  result: {
    type: Object,
    required: true
  },
  showExamples: {
    type: Boolean,
    default: true
  }
})

const titleText = computed(() => props.result.source_text || 'AI 动作结果')
</script>

<style scoped>
/* 原为浅色卡片（rgba(255,250,242,.96) + 棕色系文字）被塞进深色浮层，
   是亮色系残留；现改为深色语义，颜色全部取自 tokens.css。 */
.result-card {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  padding: var(--s-4);
  border-radius: var(--r-lg);
  background: var(--bg-card);
  border: 1px solid var(--line);
  color: var(--text-1);
  -webkit-app-region: no-drag;
}

.result-header {
  display: flex;
  justify-content: space-between;
  gap: var(--s-3);
  align-items: start;
}

.result-kicker {
  display: inline-flex;
  padding: 4px 10px;
  border-radius: var(--r-full);
  background: var(--accent-dim);
  color: var(--accent-hi);
  font-size: var(--fs-cap);
  text-transform: uppercase;
}

.result-title {
  margin-top: 8px;
  font-size: var(--fs-lg);
  line-height: 1.3;
}

.result-saved {
  color: var(--text-3);
  font-size: var(--fs-sm);
}

.result-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding-right: 4px;
  scrollbar-width: thin;
  scrollbar-color: var(--scroll-thumb) transparent;
}

.result-translation,
.result-answer {
  margin-top: 14px;
  line-height: 1.7;
  color: var(--text-1);
}

.result-list {
  margin-top: 14px;
  padding-left: 18px;
  line-height: 1.7;
  color: var(--text-2);
}

.result-examples {
  margin-top: var(--s-3);
  padding: var(--s-3);
  border-radius: var(--r-md);
  background: var(--line-soft);
}

.result-examples strong {
  display: block;
  margin-bottom: 8px;
}

.result-examples p + p {
  margin-top: 6px;
}

.result-body::-webkit-scrollbar {
  width: var(--scroll-size-thin);
}

.result-body::-webkit-scrollbar-track {
  background: transparent;
}

.result-body::-webkit-scrollbar-thumb {
  border-radius: var(--r-full);
  background: var(--scroll-thumb);
}

.result-body::-webkit-scrollbar-thumb:hover {
  background: var(--scroll-thumb-hover);
}
</style>
