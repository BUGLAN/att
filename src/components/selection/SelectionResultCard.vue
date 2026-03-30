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
.result-card {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  padding: 16px;
  border-radius: 18px;
  background: rgba(255, 250, 242, 0.96);
  border: 1px solid rgba(128, 90, 40, 0.12);
  color: #1f1711;
  -webkit-app-region: no-drag;
}

.result-header {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  align-items: start;
}

.result-kicker {
  display: inline-flex;
  padding: 4px 10px;
  border-radius: 999px;
  background: rgba(205, 145, 72, 0.14);
  color: #805b28;
  font-size: 0.72rem;
  text-transform: uppercase;
}

.result-title {
  margin-top: 8px;
  font-size: 1.15rem;
  line-height: 1.3;
}

.result-saved {
  color: #73593b;
  font-size: 0.8rem;
}

.result-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding-right: 4px;
  scrollbar-width: thin;
  scrollbar-color: rgba(128, 90, 40, 0.28) transparent;
}

.result-translation,
.result-answer {
  margin-top: 14px;
  line-height: 1.7;
  color: #2d231b;
}

.result-list {
  margin-top: 14px;
  padding-left: 18px;
  line-height: 1.7;
  color: #5f4e3e;
}

.result-examples {
  margin-top: 14px;
  padding: 12px 14px;
  border-radius: 14px;
  background: rgba(128, 90, 40, 0.06);
}

.result-examples strong {
  display: block;
  margin-bottom: 8px;
}

.result-examples p + p {
  margin-top: 6px;
}

.result-body::-webkit-scrollbar {
  width: 6px;
}

.result-body::-webkit-scrollbar-track {
  background: transparent;
}

.result-body::-webkit-scrollbar-thumb {
  border-radius: 999px;
  background: rgba(128, 90, 40, 0.24);
}

.result-body::-webkit-scrollbar-thumb:hover {
  background: rgba(128, 90, 40, 0.34);
}
</style>
