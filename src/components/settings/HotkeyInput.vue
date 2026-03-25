<template>
  <div class="hotkey-input-wrapper">
    <input
      type="text"
      class="hotkey-input"
      :class="{ listening: isListening }"
      :value="displayValue"
      readonly
      :placeholder="placeholder"
      @keydown="handleKeyDown"
      @focus="startListening"
      @blur="stopListening"
    >
    <button v-if="showReset" class="hotkey-reset-btn" @click="reset">↺</button>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'

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

const modifierMap = {
  'Control': 'ctrl',
  'Alt': 'alt',
  'Shift': 'shift',
  'Meta': 'cmd',
  'Command': 'cmd'
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
    const display = formatDisplay(modifiers, key)
    emit('update:modelValue', {
      modifiers,
      key,
      display,
      accelerator: formatAccelerator(modifiers, key)
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
    : { modifiers: ['alt', 'ctrl'], key: null, display: 'Alt+Ctrl' }
  emit('update:modelValue', defaultHotkey)
}
</script>
