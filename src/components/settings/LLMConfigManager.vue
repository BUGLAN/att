<template>
  <div class="llm-config-manager">
    <div class="llm-config-list">
      <div
        v-for="config in llmConfigs"
        :key="config.id"
        class="llm-config-item"
        :class="{ default: config.is_default }"
      >
        <div class="llm-config-info">
          <span class="llm-config-name">{{ config.name }}</span>
          <span class="llm-config-provider">{{ config.provider }}</span>
          <span v-if="config.is_default" class="default-badge">默认</span>
        </div>
        <div class="llm-config-actions">
          <button class="btn-icon" @click="$emit('edit', config)">✏️</button>
          <button class="btn-icon" @click="$emit('test', config.id)">🔍</button>
          <button v-if="!config.is_default" class="btn-icon" @click="$emit('set-default', config.id)">⭐</button>
          <button class="btn-icon" @click="$emit('delete', config.id)">🗑️</button>
        </div>
      </div>
    </div>

    <button class="btn btn-primary" @click="$emit('create')">
      + 添加配置
    </button>
  </div>
</template>

<script setup>
defineProps({
  llmConfigs: {
    type: Array,
    default: () => []
  }
})

defineEmits(['create', 'edit', 'delete', 'test', 'set-default'])
</script>
