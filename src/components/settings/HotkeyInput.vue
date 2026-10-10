<template>
  <div class="hotkey-input-wrapper">
    <BaseInput
      class="hotkey-input"
      :class="{ listening: isListening }"
      :value="displayValue"
      type="text"
      readonly
      :placeholder="placeholder"
      @keydown="handleKeyDown"
      @focus="startListening"
      @blur="stopListening"
    />
    <BaseButton v-if="showReset" variant="ghost" icon title="恢复默认快捷键" @click="reset">
      <template #icon>
        <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path d="M4 10.5A8 8 0 1 1 6.6 17" />
          <path d="M4 5v5.5h5.5" />
        </svg>
      </template>
    </BaseButton>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'

const props = defineProps({
  modelValue: {
    type: Object,
    default: () => ({ modifiers: [], key: null, display: '' })
  },
  placeholder: {
    type: String,
    default: '点击设置快捷键'
  },
  showReset: {
    type: Boolean,
    default: true
  }
})

const emit = defineEmits(['update:modelValue'])

const isListening = ref(false)
const pressedKeys = ref(new Set())

const displayValue = computed(() => {
  if (props.modelValue?.display) {
    return props.modelValue.display
  }
  return ''
})

const platform = window.electronAPI?.platform || 'win32'
const modifierMap = {
  'Control': 'ctrl',
  'Alt': 'alt',
  'Shift': 'shift',
  'Meta': platform === 'darwin' ? 'cmd' : 'win',
  'Command': 'cmd'
}

function sortModifiers(modifiers = []) {
  const priority = {
    cmd: 0,
    win: 0,
    ctrl: 1,
    alt: 2,
    shift: 3
  }

  return [...modifiers].sort((left, right) => {
    const leftPriority = priority[left] ?? Number.MAX_SAFE_INTEGER
    const rightPriority = priority[right] ?? Number.MAX_SAFE_INTEGER
    if (leftPriority !== rightPriority) {
      return leftPriority - rightPriority
    }
    return left.localeCompare(right)
  })
}

function startListening() {
  isListening.value = true
  pressedKeys.value.clear()
}

function stopListening() {
  isListening.value = false
  pressedKeys.value.clear()
}

function handleKeyDown(e) {
  e.preventDefault()
  e.stopPropagation()

  const modifiers = []
  let key = null

  // 收集修饰键
  if (e.ctrlKey) modifiers.push('ctrl')
  if (e.altKey) modifiers.push('alt')
  if (e.shiftKey) modifiers.push('shift')
  if (e.metaKey) modifiers.push('cmd')

  // 收集主键
  if (!modifierMap[e.key]) {
    key = e.key.toLowerCase()
  }

  // 至少需要一个修饰键
  if (modifiers.length > 0) {
    const normalizedModifiers = sortModifiers(modifiers)
    const display = formatDisplay(normalizedModifiers, key)
    emit('update:modelValue', {
      modifiers: normalizedModifiers,
      key,
      display,
      accelerator: formatAccelerator(normalizedModifiers, key)
    })

    isListening.value = false
  }
}

function formatDisplay(modifiers, key) {
  const symbols = {
    ctrl: '⌃',
    alt: '⌥',
    shift: '⇧',
    cmd: '⌘'
  }

  let display = modifiers.map(m => symbols[m] || m.toUpperCase()).join('')

  if (key) {
    display += key.toUpperCase()
  }

  return display
}

function formatAccelerator(modifiers, key) {
  const parts = modifiers.map(m => {
    if (m === 'cmd') return 'CommandOrControl'
    return m.charAt(0).toUpperCase() + m.slice(1)
  })

  if (key) {
    parts.push(key.toUpperCase())
  }

  return parts.join('+')
}

function reset() {
  const platform = window.electronAPI?.platform || 'win32'
  const defaultHotkey = platform === 'darwin'
    ? { modifiers: ['alt', 'cmd'], key: null, display: '⌥⌘' }
    : { modifiers: ['win', 'alt'], key: null, display: 'Win+Alt' }
  emit('update:modelValue', defaultHotkey)
}
</script>
