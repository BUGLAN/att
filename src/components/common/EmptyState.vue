<template>
  <!-- 空状态原语：图标 + 标题 + 描述 + 可选操作，取代全仓 6 种手写空态 -->
  <div class="ui-empty" :class="[`ui-empty--${size}`]">
    <span v-if="$slots.icon" class="ui-empty__icon" aria-hidden="true">
      <slot name="icon" />
    </span>
    <strong v-if="title" class="ui-empty__title">{{ title }}</strong>
    <p v-if="description || $slots.default" class="ui-empty__desc">
      <slot>{{ description }}</slot>
    </p>
    <div v-if="$slots.actions" class="ui-empty__actions">
      <slot name="actions" />
    </div>
  </div>
</template>

<script setup>
defineOptions({ name: 'EmptyState' })

defineProps({
  title: {
    type: String,
    default: ''
  },
  description: {
    type: String,
    default: ''
  },
  size: {
    type: String,
    default: 'md',
    validator: (value) => ['sm', 'md', 'lg'].includes(value)
  }
})
</script>

<style scoped>
.ui-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--s-2);
  padding: var(--s-6);
  border: 1px dashed var(--line-strong);
  border-radius: var(--r-md);
  background: transparent;
  color: var(--text-2);
  text-align: center;
}

.ui-empty--sm {
  padding: var(--s-4);
}

.ui-empty--lg {
  min-height: 300px;
  padding: var(--s-8) var(--s-6);
  border-radius: var(--r-md);
  background: var(--bg-surface);
  border-style: solid;
  border-color: var(--line);
}

.ui-empty__icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: var(--r-md);
  background: var(--accent-dim);
  color: var(--accent);
}

.ui-empty__icon :deep(svg) {
  width: 20px;
  height: 20px;
  stroke: currentColor;
  fill: none;
  stroke-width: 1.75;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.ui-empty__title {
  color: var(--text-1);
  font-size: var(--fs-md);
  font-weight: 600;
}

.ui-empty__desc {
  max-width: 520px;
  margin: 0;
  color: var(--text-3);
  font-size: var(--fs-sm);
  line-height: 1.7;
}

.ui-empty__actions {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  margin-top: var(--s-1);
}
</style>
