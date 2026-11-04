<template>
  <!-- 预设下拉选项：有自定义预设时按「内置 / 自定义」分组，否则平铺（只放 <option>，外层 select 由调用方提供） -->
  <template v-if="custom.length">
    <optgroup label="内置预设">
      <option v-for="preset in builtin" :key="preset.id" :value="preset.id">{{ preset.name }}</option>
    </optgroup>
    <optgroup label="自定义预设">
      <option v-for="preset in custom" :key="preset.id" :value="preset.id">{{ preset.name }}</option>
    </optgroup>
  </template>
  <template v-else>
    <option v-for="preset in builtin" :key="preset.id" :value="preset.id">{{ preset.name }}</option>
  </template>
</template>

<script setup>
defineProps({
  builtin: {
    type: Array,
    default: () => []
  },
  custom: {
    type: Array,
    default: () => []
  }
})
</script>
