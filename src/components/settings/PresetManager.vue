<template>
  <div class="preset-manager">
    <div class="preset-list">
      <div
        v-for="preset in presets"
        :key="preset.id"
        class="preset-item"
        :class="{ selected: preset.id === selectedId }"
        @click="selectPreset(preset.id)"
      >
        <div class="preset-info">
          <span class="preset-name">{{ preset.name }}</span>
          <span class="preset-category">{{ preset.category || '自定义' }}</span>
        </div>
        <div class="preset-actions">
          <button class="btn-icon" @click.stop="$emit('edit', preset)">✏️</button>
          <button v-if="!preset.is_builtin" class="btn-icon" @click.stop="$emit('delete', preset.id)">🗑️</button>
        </div>
      </div>
    </div>

    <button class="btn btn-primary" @click="$emit('create')">
      + 新建预设
    </button>
  </div>
</template>

<script setup>
import { ref } from 'vue'

defineProps({
  presets: {
    type: Array,
    default: () => []
  }
})

const emit = defineEmits(['create', 'edit', 'delete', 'select'])

const selectedId = ref(null)

function selectPreset(id) {
  selectedId.value = id
  emit('select', id)
}
</script>
