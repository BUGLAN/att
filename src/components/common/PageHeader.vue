<template>
  <!--
    统一页头原语：
    - variant="plain"：首页 / 记录集 / 设置使用的普通页头（标题 + 副标题 + 短分隔线）
    - variant="hero"：剪切板 / 生词本使用的能力保留版 hero 卡（金调辉光 + 统计块）
    统计块既可用 stats 属性传入，也可用 #stats 插槽完全自定义。
  -->
  <header class="page-header" :class="`page-header--${variant}`">
    <div class="page-header__main">
      <span v-if="kicker" class="page-header__kicker">{{ kicker }}</span>
      <h1 class="page-header__title">{{ title }}</h1>
      <div v-if="$slots.meta" class="page-header__meta">
        <slot name="meta" />
      </div>
      <p v-if="subtitle || $slots.default" class="page-header__subtitle">
        <slot>{{ subtitle }}</slot>
      </p>
    </div>

    <div v-if="$slots.actions" class="page-header__actions">
      <slot name="actions" />
    </div>

    <div v-if="$slots.stats || (stats && stats.length)" class="page-header__stats">
      <slot name="stats">
        <div v-for="stat in stats" :key="stat.label" class="page-header__stat">
          <span class="page-header__stat-label">{{ stat.label }}</span>
          <strong class="page-header__stat-value">{{ stat.value }}</strong>
        </div>
      </slot>
    </div>
  </header>
</template>

<script setup>
defineOptions({ name: 'PageHeader' })

defineProps({
  title: {
    type: String,
    required: true
  },
  subtitle: {
    type: String,
    default: ''
  },
  // 英文小字 kicker（hero 视觉的一部分）
  kicker: {
    type: String,
    default: ''
  },
  variant: {
    type: String,
    default: 'plain',
    validator: (value) => ['plain', 'hero'].includes(value)
  },
  // [{ label, value }]
  stats: {
    type: Array,
    default: () => []
  }
})
</script>

<style scoped>
.page-header {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: var(--s-4);
  margin-bottom: var(--s-6);
}

.page-header__main {
  min-width: 0;
  flex: 1;
}

.page-header__title {
  margin: 0;
  color: var(--text-1);
  font-family: var(--font-sans);
  font-size: clamp(var(--fs-xl), 3vw, var(--fs-2xl));
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1.15;
}

.page-header__kicker {
  display: inline-flex;
  margin-bottom: var(--s-2);
  color: var(--accent);
  font-family: var(--font-mono);
  font-size: var(--fs-cap);
  letter-spacing: 0.18em;
}

.page-header__meta {
  margin-top: var(--s-2);
}

.page-header__subtitle {
  max-width: 760px;
  margin: var(--s-2) 0 0;
  color: var(--text-2);
  font-size: var(--fs-base);
  line-height: 1.7;
}

.page-header__actions {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex-shrink: 0;
}

.page-header__actions :deep(> *) {
  flex-shrink: 0;
}

/* ---------- plain：普通页头 ---------- */
.page-header--plain {
  position: relative;
  padding-bottom: var(--s-4);
  border-bottom: 1px solid var(--line);
}

.page-header--plain::after {
  content: '';
  position: absolute;
  left: 0;
  bottom: -1px;
  width: 72px;
  height: 1px;
  background: linear-gradient(90deg, var(--accent), transparent);
}

/* ---------- hero：保留原剪切板 / 生词本的 hero 卡能力 ---------- */
.page-header--hero {
  padding: var(--s-6);
  border: 1px solid var(--line);
  border-radius: var(--r-xl);
  background:
    radial-gradient(circle at top right, var(--accent-soft), transparent 32%),
    linear-gradient(180deg, var(--bg-card), var(--bg-base));
  box-shadow: var(--edge-hi);
}

.page-header--hero .page-header__title {
  font-size: clamp(var(--fs-2xl), 3.2vw, 3rem);
  line-height: 1.05;
}

.page-header__stats {
  display: flex;
  align-items: stretch;
  gap: var(--s-3);
  flex-wrap: wrap;
  justify-content: flex-end;
  flex-shrink: 0;
}

.page-header__stat {
  min-width: 110px;
  padding: var(--s-4);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  background: var(--bg-raise);
  text-align: left;
}

.page-header__stat-label {
  display: block;
  margin-bottom: var(--s-2);
  color: var(--text-3);
  font-size: var(--fs-cap);
  letter-spacing: 0.08em;
}

.page-header__stat-value {
  color: var(--text-1);
  font-size: var(--fs-xl);
  line-height: 1.2;
}

@media (max-width: 960px) {
  .page-header {
    flex-wrap: wrap;
    align-items: flex-start;
  }

  .page-header--hero .page-header__stats {
    justify-content: flex-start;
  }
}
</style>
