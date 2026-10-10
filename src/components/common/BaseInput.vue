<template>
  <!--
    单行输入框原语：统一高度 / 圆角 / focus 环 / 错误态。
    - 默认形态：自带底色与描边（原 .form-input / .setting-input / .settings-records-search）
    - bare 形态：透明无边框，用于与图标包一层容器做搜索框（原 .clipboard-search input）
    - 其余属性（type / placeholder / min / max / step / readonly 等）与事件自动透传到原生 input
  -->
  <input
    class="ui-input"
    :class="[
      `ui-input--${size}`,
      {
        'is-invalid': invalid,
        'is-bare': bare
      }
    ]"
    :value="modelValue"
    :aria-invalid="invalid ? 'true' : undefined"
    @input="handleInput"
  >
</template>

<script setup>
defineOptions({ name: 'BaseInput' })

const props = defineProps({
  modelValue: {
    type: [String, Number],
    default: ''
  },
  size: {
    type: String,
    default: 'md',
    validator: (value) => ['sm', 'md'].includes(value)
  },
  invalid: {
    type: Boolean,
    default: false
  },
  // 透明无边框形态：外层容器负责底色与 focus 环
  bare: {
    type: Boolean,
    default: false
  },
  // 承接 v-model.trim / v-model.number 的修饰符，行为与原生 input 对齐
  modelModifiers: {
    type: Object,
    default: () => ({})
  }
})

const emit = defineEmits(['update:modelValue'])

function looseToNumber(value) {
  const parsed = Number.parseFloat(value)
  return Number.isNaN(parsed) ? value : parsed
}

function handleInput(event) {
  let nextValue = event.target.value

  if (props.modelModifiers.trim) {
    nextValue = nextValue.trim()
  }

  emit('update:modelValue', props.modelModifiers.number ? looseToNumber(nextValue) : nextValue)
}
</script>

<style scoped>
.ui-input {
  width: 100%;
  min-width: 0;
  height: 36px;
  padding: 0 var(--s-3);
  border: 1px solid var(--line);
  border-radius: var(--r-sm);
  background: var(--bg-raise);
  color: var(--text-1);
  font-family: var(--font-sans);
  font-size: var(--fs-base);
  transition:
    border-color var(--t-fast) var(--ease),
    box-shadow var(--t-fast) var(--ease),
    background-color var(--t-fast) var(--ease);
  -webkit-appearance: none;
  appearance: none;
}

.ui-input--sm {
  height: 30px;
  font-size: var(--fs-sm);
}

.ui-input:hover:not(:disabled) {
  border-color: var(--line-strong);
}

.ui-input:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: var(--focus-halo);
}

.ui-input:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.ui-input::placeholder {
  color: var(--text-3);
}

.ui-input:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.ui-input.is-invalid {
  border-color: var(--err);
  box-shadow: 0 0 0 2px var(--err-soft);
}

/* 透明形态：颜色/尺寸交给外层容器，仅保留文字样式 */
.ui-input.is-bare {
  height: 100%;
  padding: 0;
  border: 0;
  border-radius: 0;
  background: transparent;
  font-size: inherit;
  color: inherit;
}

.ui-input.is-bare:focus {
  border: 0;
  box-shadow: none;
}
</style>
