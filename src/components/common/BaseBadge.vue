<template>
  <!-- 状态徽章原语：tone × size 取代 3 套以上手写徽章（状态 / 分类 / 计数） -->
  <span
    class="ui-badge"
    :class="[
      `ui-badge--${tone}`,
      `ui-badge--${size}`,
      {
        'is-pill': pill,
        'is-mono': mono,
        'is-upper': uppercase
      }
    ]"
  >
    <slot />
  </span>
</template>

<script setup>
defineOptions({ name: 'BaseBadge' })

defineProps({
  // 语义色：成功 / 警告 / 错误 / 信息 / 强调金 / 中性
  tone: {
    type: String,
    default: 'neutral',
    validator: (value) => ['ok', 'warn', 'err', 'info', 'accent', 'neutral'].includes(value)
  },
  size: {
    type: String,
    default: 'sm',
    validator: (value) => ['sm', 'md'].includes(value)
  },
  pill: {
    type: Boolean,
    default: false
  },
  mono: {
    type: Boolean,
    default: false
  },
  uppercase: {
    type: Boolean,
    default: false
  }
})
</script>

<style scoped>
.ui-badge {
  display: inline-flex;
  align-items: center;
  gap: var(--s-1);
  padding: 2px var(--s-2);
  border: 1px solid transparent;
  border-radius: var(--r-sm);
  font-size: var(--fs-cap);
  font-weight: 500;
  line-height: 1.5;
  white-space: nowrap;
}

.ui-badge--md {
  padding: 3px var(--s-3);
  font-size: var(--fs-sm);
}

.ui-badge.is-pill {
  border-radius: var(--r-sm);
}

.ui-badge.is-mono {
  font-family: var(--font-mono);
}

.ui-badge.is-upper {
  letter-spacing: 0.1em;
  text-transform: uppercase;
}

.ui-badge--neutral {
  background: var(--bg-raise);
  border-color: var(--line);
  color: var(--text-3);
}

.ui-badge--accent {
  background: var(--accent-dim);
  border-color: var(--accent-line);
  color: var(--accent-hi);
}

.ui-badge--ok {
  background: var(--ok-soft);
  border-color: var(--ok-line);
  color: var(--ok);
}

.ui-badge--warn {
  background: var(--warn-soft);
  border-color: var(--warn-line);
  color: var(--warn);
}

.ui-badge--err {
  background: var(--err-soft);
  border-color: var(--err-line);
  color: var(--err);
}

.ui-badge--info {
  background: var(--info-soft);
  border-color: var(--info-line);
  color: var(--info);
}
</style>
