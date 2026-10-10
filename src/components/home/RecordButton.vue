<template>
  <!-- 录音按钮（旧版圆形按钮，当前未被引用；已统一到原语，尺寸通过 CSS 变量注入） -->
  <BaseButton
    variant="ghost"
    class="record-btn"
    :class="{ recording: isRecording }"
    :disabled="disabled"
    style="--ui-btn-radius: var(--r-full)"
    @click="$emit('click')"
  >
    <template #icon>
      <span class="btn-icon">{{ icon }}</span>
    </template>
    <span class="btn-text">{{ text }}</span>
  </BaseButton>
</template>

<script setup>
import { computed } from 'vue'
import BaseButton from '@/components/common/BaseButton.vue'

const props = defineProps({
  isRecording: {
    type: Boolean,
    default: false
  },
  disabled: {
    type: Boolean,
    default: false
  }
})

defineEmits(['click'])

const icon = computed(() => {
  if (props.isRecording) return '⏹'
  return '🎙'
})

const text = computed(() => {
  if (props.isRecording) return '停止'
  return '开始'
})
</script>

<style scoped>
/* 140px 圆形录音按钮：在幽灵按钮基线上保留原有外观 */
.record-btn {
  flex-direction: column;
  width: 140px;
  height: 140px;
  gap: var(--s-2);
  border: 2px solid var(--line-strong);
  background: radial-gradient(circle at 30% 30%, var(--gray-700), var(--gray-850));
  color: var(--gray-300);
}

.record-btn:hover:not(:disabled) {
  border-color: var(--gray-500);
  color: var(--gray-100);
}

.record-btn.recording {
  border-color: var(--err);
  background: radial-gradient(circle at 30% 30%, var(--gray-700), var(--gray-900));
  color: var(--err);
}

.btn-icon {
  font-size: var(--fs-2xl);
  line-height: 1;
}

.btn-text {
  font-size: var(--fs-sm);
}
</style>
