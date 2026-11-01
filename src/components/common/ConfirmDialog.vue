<template>
  <!--
    二次确认对话框：危险操作统一走这里（红色实心主按钮）。
    自带 role="dialog" / aria-modal / Esc 关闭 / 打开时聚焦取消按钮 / 简单焦点陷阱。
  -->
  <Teleport to="body">
    <div
      v-if="modelValue"
      class="ui-confirm"
      @click.self="handleCancel"
      @keydown="handleKeydown"
    >
      <div
        ref="dialogRef"
        class="ui-confirm__panel"
        role="dialog"
        aria-modal="true"
        :aria-label="title"
      >
        <h3 class="ui-confirm__title">{{ title }}</h3>
        <p v-if="description || $slots.default" class="ui-confirm__desc">
          <slot>{{ description }}</slot>
        </p>
        <div class="ui-confirm__actions">
          <BaseButton variant="outline" @click="handleCancel">
            {{ cancelText }}
          </BaseButton>
          <BaseButton
            :variant="tone === 'danger' ? 'danger' : 'solid'"
            :loading="loading"
            @click="handleConfirm"
          >
            {{ confirmText }}
          </BaseButton>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<script setup>
import { nextTick, onBeforeUnmount, ref, watch } from 'vue'
import BaseButton from './BaseButton.vue'

defineOptions({ name: 'ConfirmDialog' })

const props = defineProps({
  modelValue: {
    type: Boolean,
    default: false
  },
  title: {
    type: String,
    default: '确认操作'
  },
  description: {
    type: String,
    default: ''
  },
  confirmText: {
    type: String,
    default: '确认'
  },
  cancelText: {
    type: String,
    default: '取消'
  },
  loading: {
    type: Boolean,
    default: false
  },
  tone: {
    type: String,
    default: 'danger',
    validator: (value) => ['danger', 'default'].includes(value)
  }
})

const emit = defineEmits(['update:modelValue', 'confirm', 'cancel'])

const dialogRef = ref(null)
let lastActiveElement = null

function close() {
  emit('update:modelValue', false)
}

function handleCancel() {
  emit('cancel')
  close()
}

function handleConfirm() {
  emit('confirm')
  close()
}

// 焦点陷阱：Tab / Shift+Tab 只在对话框内循环
function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault()
    handleCancel()
    return
  }

  if (event.key !== 'Tab' || !dialogRef.value) {
    return
  }

  const focusables = Array.from(
    dialogRef.value.querySelectorAll(
      'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])'
    )
  )

  if (!focusables.length) {
    return
  }

  const first = focusables[0]
  const last = focusables[focusables.length - 1]
  const active = document.activeElement

  if (event.shiftKey && (active === first || !dialogRef.value.contains(active))) {
    event.preventDefault()
    last.focus()
    return
  }

  if (!event.shiftKey && active === last) {
    event.preventDefault()
    first.focus()
  }
}

watch(() => props.modelValue, (visible) => {
  if (visible) {
    lastActiveElement = document.activeElement
    // 危险操作默认聚焦"取消"（DOM 中第一个按钮），避免回车误触发
    nextTick(() => {
      dialogRef.value?.querySelector('button')?.focus()
    })
    return
  }

  if (lastActiveElement?.focus) {
    lastActiveElement.focus()
  }
  lastActiveElement = null
})

onBeforeUnmount(() => {
  lastActiveElement = null
})
</script>

<style scoped>
.ui-confirm {
  position: fixed;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--s-6);
  background: var(--scrim-soft);
  backdrop-filter: blur(6px);
  z-index: var(--z-modal);
}

.ui-confirm__panel {
  width: min(420px, 100%);
  padding: var(--s-6);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  background: var(--bg-surface);
  box-shadow: var(--e-3), var(--edge-hi);
}

.ui-confirm__title {
  margin: 0 0 var(--s-2);
  color: var(--text-1);
  font-size: var(--fs-md);
  font-weight: 600;
}

.ui-confirm__desc {
  margin: 0 0 var(--s-6);
  color: var(--text-2);
  font-size: var(--fs-sm);
  line-height: 1.7;
  word-break: break-word;
}

.ui-confirm__actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--s-2);
}
</style>
