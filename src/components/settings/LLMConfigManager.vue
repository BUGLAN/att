<template>
  <div class="llm-config-manager">
    <div v-if="llmConfigs.length" class="llm-config-list">
      <div
        v-for="config in llmConfigs"
        :key="config.id"
        class="llm-config-item"
        :class="{ default: config.id === defaultConfigId || config.is_default }"
      >
        <div class="llm-config-info">
          <span class="llm-config-name">{{ config.name }}</span>
          <span class="llm-config-provider">{{ config.provider }}</span>
          <BaseBadge v-if="config.id === defaultConfigId || config.is_default" tone="accent">默认</BaseBadge>
          <span class="llm-config-provider">{{ config.model }}</span>
        </div>
        <div class="llm-config-actions">
          <BaseButton
            variant="ghost"
            size="sm"
            icon
            title="编辑配置"
            aria-label="编辑配置"
            @click="$emit('edit', config)"
          >
            <template #icon>
              <svg viewBox="0 0 24 24" fill="none">
                <path d="M4 20h4l10-10-4-4L4 16v4Z" />
                <path d="m12 6 4 4" />
              </svg>
            </template>
          </BaseButton>
          <BaseButton
            variant="danger"
            size="sm"
            icon
            title="删除配置"
            aria-label="删除配置"
            @click="$emit('delete', config.id)"
          >
            <template #icon>
              <svg viewBox="0 0 24 24" fill="none">
                <path d="M4 7h16" />
                <path d="M10 11v6" />
                <path d="M14 11v6" />
                <path d="M6 7l1 12h10l1-12" />
                <path d="M9 7V4h6v3" />
              </svg>
            </template>
          </BaseButton>
        </div>
      </div>
    </div>

    <EmptyState v-else size="sm" title="暂无 LLM 配置" description="新建一个配置后即可用于文本优化。" />

    <BaseButton variant="solid" block @click="$emit('create')">
      新建配置
    </BaseButton>
  </div>
</template>

<script setup>
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import EmptyState from '@/components/common/EmptyState.vue'

defineProps({
  llmConfigs: {
    type: Array,
    default: () => []
  },
  defaultConfigId: {
    type: String,
    default: ''
  }
})

defineEmits(['create', 'edit', 'delete'])
</script>
