<template>
  <!-- Toast 队列：支持 tone(success|error|info)，同一时刻最多堆叠 3 条 -->
  <Teleport to="body">
    <div class="ui-toast-stack" aria-live="polite">
      <TransitionGroup name="ui-toast">
        <div
          v-for="toast in toasts"
          :key="toast.id"
          class="ui-toast"
          :class="`ui-toast--${toast.tone}`"
          role="status"
        >
          <span class="ui-toast__dot" aria-hidden="true"></span>
          <span class="ui-toast__text">{{ toast.message }}</span>
        </div>
      </TransitionGroup>
    </div>
  </Teleport>
</template>

<script setup>
import { computed } from 'vue'
import { useAppStore } from '@/stores/app'

const appStore = useAppStore()

const toasts = computed(() => appStore.toasts)
</script>

<style scoped>
.ui-toast-stack {
  position: fixed;
  left: 50%;
  bottom: var(--s-6);
  transform: translateX(-50%);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--s-2);
  z-index: var(--z-toast);
  pointer-events: none;
}

.ui-toast {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
  max-width: min(560px, 90vw);
  padding: var(--s-2) var(--s-4);
  border: 1px solid var(--line-strong);
  border-radius: var(--r-md);
  background: var(--bg-raise);
  color: var(--text-1);
  font-size: var(--fs-sm);
  box-shadow: var(--e-2);
}

.ui-toast__dot {
  width: 8px;
  height: 8px;
  flex-shrink: 0;
  border-radius: var(--r-xs);
  background: var(--info);
}

.ui-toast__text {
  min-width: 0;
  word-break: break-word;
}

.ui-toast--success {
  border-color: var(--ok-line-soft);
}

.ui-toast--success .ui-toast__dot {
  background: var(--ok);
}

.ui-toast--error {
  border-color: var(--err-line-soft);
}

.ui-toast--error .ui-toast__dot {
  background: var(--err);
}

.ui-toast--info .ui-toast__dot {
  background: var(--info);
}

.ui-toast-enter-active,
.ui-toast-leave-active {
  transition: opacity var(--t-base) var(--ease), transform var(--t-base) var(--ease);
}

.ui-toast-enter-from,
.ui-toast-leave-to {
  opacity: 0;
  transform: translateY(12px);
}

.ui-toast-move {
  transition: transform var(--t-base) var(--ease);
}
</style>
