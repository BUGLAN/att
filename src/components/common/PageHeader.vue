<template>
  <!--
    统一页头原语：与设置页页头同一套视觉——面包屑 + 标题（说明可选，默认不放：标题已经说清楚的不再重复），
    首页 / 转文字 / 记录集 / 剪贴板共用。右侧可选 #actions 插槽放文字按钮。
  -->
  <header class="page-header">
    <div class="page-header__main">
      <div v-if="crumb" class="page-header__crumb">{{ crumb }}</div>
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
  // 面包屑（如「首页 / 记录集」），与设置页「设置 / 通用」同款
  crumb: {
    type: String,
    default: ''
  },
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
  margin-bottom: var(--s-2);
}

.page-header__main {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
  flex: 1;
}

.page-header__crumb {
  font-size: var(--fs-cap);
  color: var(--text-3);
}

.page-header__title {
  margin: 0;
  font-size: var(--fs-xl);
  font-weight: 600;
  color: var(--text-1);
}

.page-header__subtitle {
  margin: 0;
  font-size: var(--fs-sm);
  line-height: 1.6;
  color: var(--text-2);
}

.page-header__actions {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex-shrink: 0;
}
</style>
