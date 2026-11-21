<template>
  <!-- 导出按钮 + 格式菜单（记录集「导出当前结果」「导出选中」共用）：点外面或按 Esc 收起 -->
  <div ref="rootRef" class="export-menu" @keydown.esc="open = false">
    <button
      class="ui-tbtn"
      :class="buttonClass"
      type="button"
      :disabled="disabled"
      aria-haspopup="menu"
      :aria-expanded="open ? 'true' : 'false'"
      @click="open = !open"
    >
      <span class="ui-tbtn-glyph">{{ G.export }}</span>{{ label }}
    </button>
    <div v-if="open" class="export-menu-list" :class="`is-${placement}`" role="menu">
      <button
        v-for="option in EXPORT_FORMATS"
        :key="option.value"
        class="export-menu-item"
        type="button"
        role="menuitem"
        @click="choose(option.value)"
      >
        {{ option.label }}
      </button>
    </div>
  </div>
</template>

<script setup>
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { ACTION_GLYPHS as G } from '@/components/common/glyphs.mjs'
import { EXPORT_FORMATS } from '@/utils/exportDownload.mjs'

const props = defineProps({
  label: {
    type: String,
    default: '导出'
  },
  disabled: {
    type: Boolean,
    default: false
  },
  // 菜单向下（工具栏）还是向上（贴底的选择栏）展开
  placement: {
    type: String,
    default: 'down',
    validator: (value) => ['down', 'up'].includes(value)
  },
  buttonClass: {
    type: [String, Object, Array],
    default: ''
  }
})

const emit = defineEmits(['select'])
const open = ref(false)
const rootRef = ref(null)

function choose(format) {
  open.value = false
  emit('select', format)
}

function handleDocumentClick(event) {
  if (open.value && rootRef.value && !rootRef.value.contains(event.target)) {
    open.value = false
  }
}

watch(() => props.disabled, (disabled) => {
  if (disabled) {
    open.value = false
  }
})

onMounted(() => {
  document.addEventListener('mousedown', handleDocumentClick)
})

onBeforeUnmount(() => {
  document.removeEventListener('mousedown', handleDocumentClick)
})
</script>

<style scoped>
.export-menu {
  position: relative;
  display: inline-flex;
}

.export-menu-list {
  position: absolute;
  right: 0;
  z-index: 20;
  display: flex;
  flex-direction: column;
  min-width: 168px;
  padding: 4px;
  border: 1px solid var(--line-strong);
  border-radius: var(--r-sm);
  background: var(--bg-raise);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
}

.export-menu-list.is-down {
  top: calc(100% + 4px);
}

.export-menu-list.is-up {
  bottom: calc(100% + 4px);
}

.export-menu-item {
  padding: 6px 10px;
  border: 0;
  border-radius: calc(var(--r-sm) - 2px);
  background: transparent;
  color: var(--text-2);
  font: inherit;
  font-size: var(--fs-sm);
  text-align: left;
  cursor: pointer;
}

.export-menu-item:hover,
.export-menu-item:focus-visible {
  outline: none;
  background: var(--accent-veil);
  color: var(--text-1);
}
</style>
