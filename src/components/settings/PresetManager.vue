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
            <span v-if="preset.id === defaultPresetId" class="preset-category">默认</span>
          </div>
          <p class="preset-description">{{ preset.description || '无描述' }}</p>
        </div>
        <div class="preset-actions">
          <button class="btn-icon" type="button" @click="$emit('view', preset)">查看</button>
        </div>
      </div>
    </div>

    <div v-else class="manager-empty">未发现 prompts/*.txt</div>
  </div>
</template>

<script setup>
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
