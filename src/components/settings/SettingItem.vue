<template>
  <div class="setting-item">
    <div class="setting-info">
      <span class="setting-label">{{ label }}</span>
      <span v-if="description" class="setting-desc">{{ description }}</span>
    </div>
    <div class="setting-control">
      <slot></slot>
    </div>
  </div>
</template>

<script setup>
defineProps({
  label: {
    type: String,
    required: true
  },
  description: {
    type: String,
    default: ''
  }
})
</script>

