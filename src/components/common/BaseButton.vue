<template>
  <!-- 按钮原语：variant × size 两个维度取代全仓约 30 种手写按钮类 -->
  <button
    class="ui-btn"
    :class="[
      `ui-btn--${variant}`,
      `ui-btn--${size}`,
      {
        'is-active': active,
        'is-loading': loading,
        'is-block': block,
        'is-icon': icon,
        'is-pill': pill
      }
    ]"
    :type="type"
    :disabled="disabled || loading"
    :aria-busy="loading ? 'true' : undefined"
  >
    <span v-if="loading" class="ui-btn__spinner" aria-hidden="true"></span>
    <span v-else-if="$slots.icon" class="ui-btn__icon" aria-hidden="true">
      <slot name="icon" />
    </span>
    <span v-if="$slots.default" class="ui-btn__label"><slot /></span>
  </button>
</template>

<script setup>
defineOptions({ name: 'BaseButton' })

defineProps({
  // 语义变体：实心金 / 描边 / 幽灵 / 危险
  variant: {
    type: String,
    default: 'outline',
    validator: (value) => ['solid', 'outline', 'ghost', 'danger'].includes(value)
  },
  size: {
    type: String,
    default: 'md',
    validator: (value) => ['sm', 'md'].includes(value)
  },
  type: {
    type: String,
    default: 'button'
  },
  disabled: {
    type: Boolean,
    default: false
  },
  loading: {
    type: Boolean,
    default: false
  },
  // 选中态（筛选按钮、收藏按钮等）
  active: {
    type: Boolean,
    default: false
  },
  // 占满父容器宽度
  block: {
    type: Boolean,
    default: false
  },
  // 纯图标按钮：正方形，无文字
  icon: {
    type: Boolean,
    default: false
  },
  // 胶囊形态
  pill: {
    type: Boolean,
    default: false
  }
})
</script>

<style scoped>
.ui-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(--s-1);
  height: 36px;
  padding: 0 var(--s-4);
  border: 1px solid transparent;
  border-radius: var(--ui-btn-radius, var(--r-sm));
  background: transparent;
  color: var(--text-2);
  font-family: var(--font-sans);
  font-size: var(--fs-sm);
  font-weight: 500;
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  -webkit-appearance: none;
  appearance: none;
  transition:
    background-color var(--t-fast) var(--ease),
    border-color var(--t-fast) var(--ease),
    color var(--t-fast) var(--ease),
    transform var(--t-fast) var(--ease);
}

.ui-btn--sm {
  height: 30px;
  padding: 0 var(--s-3);
  font-size: var(--fs-sm);
}

.ui-btn--md {
  height: 36px;
}

/* 正方形图标按钮：与 sm/md 高度对齐；尺寸可用 --ui-icon-size 覆盖（内联 style 即可） */
.ui-btn.is-icon {
  padding: 0;
  width: var(--ui-icon-size, 36px);
  height: var(--ui-icon-size, 36px);
}

.ui-btn.is-icon.ui-btn--sm {
  width: var(--ui-icon-size, 30px);
  height: var(--ui-icon-size, 30px);
}

.ui-btn.is-pill {
  border-radius: var(--r-sm);
}

.ui-btn.is-block {
  width: 100%;
}

.ui-btn__icon,
.ui-btn__label {
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.ui-btn__icon :deep(svg) {
  width: 15px;
  height: 15px;
  stroke: currentColor;
  fill: none;
  stroke-width: 1.75;
  stroke-linecap: round;
  stroke-linejoin: round;
}

/* ---------- 变体 1：实心金（原 .btn-primary） ---------- */
.ui-btn--solid {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--accent-ink);
  font-weight: 600;
}

.ui-btn--solid:hover:not(:disabled) {
  background: var(--accent-hi);
  border-color: var(--accent-hi);
}

/* ---------- 变体 2：描边（原 .btn / .btn-secondary） ---------- */
.ui-btn--outline {
  background: var(--bg-raise);
  border-color: var(--line);
  color: var(--text-2);
}

.ui-btn--outline:hover:not(:disabled) {
  background: var(--bg-hover);
  border-color: var(--line-strong);
  color: var(--text-1);
}

/* ---------- 变体 3：幽灵（原 .text-button / .icon-button / .action-button） ---------- */
.ui-btn--ghost {
  background: transparent;
  border-color: transparent;
  color: var(--text-2);
}

.ui-btn--ghost:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-1);
}

/* ---------- 变体 4：危险主按钮（原 .btn-delete-selected 的实心升级） ----------
   红色实心 + 深色文字：既满足"危险操作一眼可辨"，又保证反色对比度达标。 */
.ui-btn--danger {
  background: var(--err);
  border-color: var(--err);
  color: var(--gray-950);
  font-weight: 600;
}

.ui-btn--danger:hover:not(:disabled) {
  filter: brightness(1.08);
}

/* ---------- 选中态（筛选 / 收藏），与金色强调体系一致 ---------- */
.ui-btn.is-active {
  background: var(--accent-dim);
  border-color: var(--accent-line);
  color: var(--accent-hi);
}

/* ---------- 按下与焦点：与主窗口按钮族同一套触感 ---------- */
.ui-btn:active:not(:disabled) {
  transform: translateY(1px);
  filter: brightness(0.94);
}

.ui-btn:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.ui-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.ui-btn.is-loading {
  cursor: progress;
}

.ui-btn__spinner {
  width: 12px;
  height: 12px;
  border: 2px solid currentColor;
  border-top-color: transparent;
  border-radius: var(--r-full);
  animation: ui-btn-spin 0.8s linear infinite;
}

@keyframes ui-btn-spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .ui-btn__spinner {
    animation-duration: 1.6s;
  }
}
</style>
