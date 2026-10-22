import { computed, ref } from 'vue'
import { defineStore } from 'pinia'

// 当前正在流式优化的记录。应用同一时刻只优化一条记录，因此用单槽状态即可。
export const useOptimizationStreamStore = defineStore('optimizationStream', () => {
  const recordId = ref('')
  const text = ref('')
  const phase = ref('idle') // idle | waiting | streaming | done | error | cancelled
  const error = ref('')
  const source = ref('auto') // auto | manual
  const startedAt = ref(0)
  const firstTokenAt = ref(0)

  const isActive = computed(() => phase.value === 'waiting' || phase.value === 'streaming')
  const isWaitingForFirstToken = computed(() => phase.value === 'waiting')
  const hasText = computed(() => Boolean(text.value))

  function begin(id, options = {}) {
    recordId.value = id || ''
    text.value = ''
    phase.value = 'waiting'
    error.value = ''
    source.value = options.source || 'auto'
    startedAt.value = Date.now()
    firstTokenAt.value = 0
  }

  function append(chunk) {
    if (!chunk) {
      return
    }

    if (phase.value === 'waiting' || phase.value === 'streaming') {
      if (!firstTokenAt.value) {
        firstTokenAt.value = Date.now()
      }
      text.value += chunk
      phase.value = 'streaming'
    }
  }

  function finish(finalText) {
    if (typeof finalText === 'string') {
      text.value = finalText
    }
    phase.value = 'done'
    error.value = ''
  }

  function fail(message, options = {}) {
    phase.value = 'error'
    error.value = message || '优化失败'
    if (options.keepText === false) {
      text.value = ''
    }
  }

  function cancel() {
    phase.value = 'cancelled'
  }

  function reset() {
    recordId.value = ''
    text.value = ''
    phase.value = 'idle'
    error.value = ''
    source.value = 'auto'
    startedAt.value = 0
    firstTokenAt.value = 0
  }

  return {
    recordId,
    text,
    phase,
    error,
    source,
    startedAt,
    firstTokenAt,
    isActive,
    isWaitingForFirstToken,
    hasText,
    begin,
    append,
    finish,
    fail,
    cancel,
    reset
  }
})
