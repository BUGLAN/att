<template>
  <!-- 多行文本原语：与 BaseInput 同一套描边 / focus 环 / 错误态 -->
  <textarea
    class="ui-textarea"
    :class="{ 'is-invalid': invalid }"
    :value="modelValue"
    :rows="rows"
    :aria-invalid="invalid ? 'true' : undefined"
    @input="handleInput"
  ></textarea>
</template>

<script setup>
defineOptions({ name: 'BaseTextarea' })

const props = defineProps({
  modelValue: {
    type: String,
    default: ''
  },
  rows: {
    type: [Number, String],
    default: 5
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

function handleInput(event) {
  let nextValue = event.target.value

  if (props.modelModifiers.trim) {
    nextValue = nextValue.trim()
  }

  emit('update:modelValue', nextValue)
}
</script>

<style scoped>
.ui-textarea {
  display: block;
  width: 100%;
  min-width: 0;
  min-height: 120px;
  padding: var(--s-2) var(--s-3);
  border: 1px solid var(--line);
  border-radius: var(--r-sm);
  background: var(--bg-raise);
  color: var(--text-1);
  font-family: var(--font-sans);
  font-size: var(--fs-base);
  line-height: 1.6;
  resize: vertical;
  transition:
    border-color var(--t-fast) var(--ease),
    box-shadow var(--t-fast) var(--ease);
  -webkit-appearance: none;
  appearance: none;
}

.ui-textarea:hover:not(:disabled) {
  border-color: var(--line-strong);
}

.ui-textarea:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: var(--focus-halo);
}

.ui-textarea:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.ui-textarea::placeholder {
  color: var(--text-3);
}

.ui-textarea:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.ui-textarea.is-invalid {
  border-color: var(--err);
  box-shadow: 0 0 0 2px var(--err-soft);
}
</style>
