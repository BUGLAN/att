<template>
  <!--
    弹窗外壳（a11y 骨架补齐）：
    role="dialog" + aria-modal + aria-labelledby、Esc 关闭、
    打开时聚焦首个可交互元素、Tab / Shift+Tab 简单焦点陷阱。
  -->
  <div
    :id="modalId"
    class="modal active"
    @click.self="closeOnBackdrop && close()"
  >
    <div
      ref="dialogRef"
      class="modal-content"
      role="dialog"
      aria-modal="true"
      :aria-labelledby="titleId"
    >
      <div class="modal-header">
        <h3 :id="titleId">{{ title }}</h3>
        <button class="modal-close" type="button" aria-label="关闭弹窗" @click="close">×</button>
      </div>
      <div class="modal-body">
        <slot></slot>
      </div>
      <div v-if="$slots.footer" class="modal-footer">
        <slot name="footer"></slot>
      </div>
    </div>
  </div>
</template>

<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { useAppStore } from '@/stores/app'

const props = defineProps({
  title: {
    type: String,
    required: true
  },
  modalName: {
    type: String,
    required: true
  },
  // 点遮罩是否关闭：编辑类弹窗传 false，避免误点丢草稿（× 与 Esc 仍可关闭）
  closeOnBackdrop: {
    type: Boolean,
    default: true
  }
})

const appStore = useAppStore()
const dialogRef = ref(null)
const modalId = computed(() => `${props.modalName}Modal`)
const titleId = computed(() => `${props.modalName}ModalTitle`)
let lastActiveElement = null

function close() {
  appStore.closeModal(props.modalName)
}

function getFocusableElements() {
  if (!dialogRef.value) {
    return []
  }

  return Array.from(dialogRef.value.querySelectorAll(
    'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])'
  ))
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault()
    event.stopPropagation()
    close()
    return
  }

  if (event.key !== 'Tab') {
    return
  }

  const focusables = getFocusableElements()
  if (!focusables.length) {
    return
  }

  const first = focusables[0]
  const last = focusables[focusables.length - 1]
  const active = document.activeElement

  if (event.shiftKey && (active === first || !dialogRef.value?.contains(active))) {
    event.preventDefault()
    last.focus()
    return
  }

  if (!event.shiftKey && active === last) {
    event.preventDefault()
    first.focus()
  }
}

onMounted(() => {
  lastActiveElement = document.activeElement

  // 打开时聚焦首个可交互元素；没有可聚焦元素时回落到对话框本身
  nextTick(() => {
    const [firstFocusable] = getFocusableElements()
    if (firstFocusable) {
      firstFocusable.focus()
      return
    }
    dialogRef.value?.setAttribute('tabindex', '-1')
    dialogRef.value?.focus()
  })

  window.addEventListener('keydown', handleKeydown, true)
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', handleKeydown, true)

  if (lastActiveElement?.focus) {
    lastActiveElement.focus()
  }
  lastActiveElement = null
})
</script>
