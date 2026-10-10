<template>
  <!-- 骨架屏原语：加载态统一用它替代"正在加载…"纯文字 -->
  <div class="ui-skeleton" :class="{ 'is-inline': inline }" aria-hidden="true">
    <span
      v-for="line in lines"
      :key="line"
      class="ui-skeleton__bar"
      :style="barStyle(line)"
    ></span>
  </div>
</template>

<script setup>
defineOptions({ name: 'Skeleton' })

const props = defineProps({
  // 骨架行数
  lines: {
    type: Number,
    default: 3
  },
  width: {
    type: String,
    default: '100%'
  },
  height: {
    type: String,
    default: '14px'
  },
  radius: {
    type: String,
    default: 'var(--r-sm)'
  },
  // 行内形态：宽度按行递减，模拟文本段落
  inline: {
    type: Boolean,
    default: false
  }
})

function barStyle(line) {
  return {
    width: props.inline && line < props.lines ? `${100 - (line - 1) * 12}%` : props.width,
    height: props.height,
    borderRadius: props.radius
  }
}
</script>

<style scoped>
.ui-skeleton {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  width: 100%;
}

.ui-skeleton__bar {
  display: block;
  background: linear-gradient(
    90deg,
    var(--bg-hover) 0%,
    var(--gray-600) 50%,
    var(--bg-hover) 100%
  );
  background-size: 200% 100%;
  animation: ui-skeleton-shimmer 1.4s ease-in-out infinite;
}

@keyframes ui-skeleton-shimmer {
  0% {
    background-position: 200% 0;
  }

  100% {
    background-position: -200% 0;
  }
}

@media (prefers-reduced-motion: reduce) {
  .ui-skeleton__bar {
    animation: none;
  }
}
</style>
