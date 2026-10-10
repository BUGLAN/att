<template>
  <!-- 下拉选择原语：与 BaseInput 同一套高度 / 圆角 / focus 环，箭头为内联 SVG -->
  <select
    class="ui-select"
    :class="[`ui-select--${size}`, { 'is-invalid': invalid }]"
    :value="modelValue"
    :aria-invalid="invalid ? 'true' : undefined"
    @change="handleChange"
  >
    <slot />
  </select>
</template>

<script setup>
defineOptions({ name: 'BaseSelect' })

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
  modelModifiers: {
    type: Object,
    default: () => ({})
  }
})

const emit = defineEmits(['update:modelValue'])

function handleChange(event) {
  const rawValue = event.target.value
  emit('update:modelValue', props.modelModifiers.number ? Number(rawValue) : rawValue)
}
</script>

<style scoped>
.ui-select {
  min-width: 0;
  height: 36px;
  padding: 0 calc(var(--s-6) + var(--s-1)) 0 var(--s-3);
  border: 1px solid var(--line);
  border-radius: var(--r-sm);
  background-color: var(--bg-raise);
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 12 12' fill='none'%3E%3Cpath d='M2.5 4.5 6 8l3.5-3.5' stroke='%23a1a1aa' stroke-width='1.4' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right var(--s-3) center;
  color: var(--text-1);
  font-family: var(--font-sans);
  font-size: var(--fs-base);
  cursor: pointer;
  text-overflow: ellipsis;
  transition:
    border-color var(--t-fast) var(--ease),
    box-shadow var(--t-fast) var(--ease),
    background-color var(--t-fast) var(--ease);
  -webkit-appearance: none;
  appearance: none;
}

.ui-select--sm {
  height: 30px;
  font-size: var(--fs-sm);
}

.ui-select:hover:not(:disabled) {
  border-color: var(--line-strong);
  background-color: var(--bg-hover);
}

.ui-select:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: var(--focus-halo);
}

.ui-select:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.ui-select:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.ui-select.is-invalid {
  border-color: var(--err);
  box-shadow: 0 0 0 2px var(--err-soft);
}
</style>
