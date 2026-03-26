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
          <span v-if="config.id === defaultConfigId || config.is_default" class="default-badge">默认</span>
          <span class="llm-config-provider">{{ config.model }}</span>
        </div>
        <div class="llm-config-actions">
          <button class="btn-icon" type="button" title="编辑配置" aria-label="编辑配置" @click="$emit('edit', config)">
            <svg viewBox="0 0 24 24" fill="none">
              <path d="M4 20h4l10-10-4-4L4 16v4Z" />
              <path d="m12 6 4 4" />
            </svg>
          </button>
          <button class="btn-icon btn-icon-danger" type="button" title="删除配置" aria-label="删除配置" @click="$emit('delete', config.id)">
            <svg viewBox="0 0 24 24" fill="none">
              <path d="M4 7h16" />
              <path d="M10 11v6" />
              <path d="M14 11v6" />
              <path d="M6 7l1 12h10l1-12" />
              <path d="M9 7V4h6v3" />
            </svg>
          </button>
        </div>
      </div>
    </div>

    <div v-else class="manager-empty">暂无 LLM 配置</div>

    <button class="btn btn-primary" type="button" @click="$emit('create')">
      新建配置
    </button>
  </div>
</template>

<script setup>
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
