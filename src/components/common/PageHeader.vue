<template>
  <!--
    统一页头原语：标题 + 副标题 + 金色渐变短分隔线，
    首页 / 记录集 / 剪切板 / 设置共用同一视觉。
    右侧可选 #actions 插槽放状态徽章或操作按钮。
  -->
  <header class="page-header">
    <div class="page-header__main">
      <h1 class="page-header__title">{{ title }}</h1>
      <p v-if="subtitle || $slots.default" class="page-header__subtitle">
        <slot>{{ subtitle }}</slot>
      </p>
    </div>

    <div v-if="$slots.actions" class="page-header__actions">
      <slot name="actions" />
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

.page-header {
  position: relative;
  padding-bottom: var(--s-4);
  border-bottom: 1px solid var(--line);
}

.page-header::after {
  content: '';
  position: absolute;
  left: 0;
  bottom: -1px;
  width: 72px;
  height: 1px;
  background: linear-gradient(90deg, var(--accent), transparent);
}

@media (max-width: 960px) {
  .page-header {
    flex-wrap: wrap;
    align-items: flex-start;
  }
}
</style>
