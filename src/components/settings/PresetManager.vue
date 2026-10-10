<template>
  <div class="preset-manager">
    <div v-if="presets.length" class="preset-list">
      <div
        v-for="preset in presets"
        :key="preset.id"
        class="preset-item"
        :class="{ selected: preset.id === defaultPresetId }"
      >
        <div class="preset-info" @click="$emit('select', preset.id)">
          <div class="preset-row">
            <span class="preset-name">{{ preset.name }}</span>
            <BaseBadge v-if="preset.id === defaultPresetId" tone="accent" uppercase>默认</BaseBadge>
          </div>
          <p class="preset-description">{{ preset.description || '无描述' }}</p>
        </div>
        <div class="preset-actions">
          <BaseButton variant="ghost" size="sm" @click="$emit('view', preset)">查看</BaseButton>
        </div>
      </div>
    </div>

    <EmptyState v-else size="sm" title="未发现提示词" description="请确认 prompts/*.txt 是否存在。" />
  </div>
</template>

<script setup>
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import EmptyState from '@/components/common/EmptyState.vue'

defineProps({
  presets: {
    type: Array,
    default: () => []
  },
  defaultPresetId: {
    type: String,
    default: ''
  }
})

defineEmits(['select', 'view'])
</script>
