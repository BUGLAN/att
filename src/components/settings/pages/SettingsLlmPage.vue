<template>
  <div class="settings-stack">
    <section class="settings-section">
      <!-- 页面标题已由设置页头部给出，分区内不再重复 -->
      <LLMServiceSection :show-title="false" />
    </section>
  </div>
</template>

<script setup>
import LLMServiceSection from '@/components/settings/llm/LLMServiceSection.vue'
</script>
