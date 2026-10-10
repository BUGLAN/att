<template>
  <!--
    ATT 品牌标：金色圆角方 + 繁体「錄」，与 OS 应用图标（electron/icons）同源、
    同一套金色令牌、同一个字形，避免"系统图标和应用内标长得不一样"的分裂。

    用 <img> 引 SVG 而不是内联假名：位图在任何 DPI 下都糊，SVG 由浏览器按
    设备像素比光栅化，32px 与 128px 同样锐利；Vite 会把小体积 SVG 自动内联成
    data URI（CSP img-src 已允许 data:），不产生额外请求。
  -->
  <img
    class="logo-mark"
    :src="logoMark"
    :width="size"
    :height="size"
    :alt="alt"
    draggable="false"
  >
</template>

<script setup>
import logoMark from '@/assets/logo-mark.svg'

defineOptions({ name: 'LogoMark' })

defineProps({
  // 渲染边长（px）；标题栏 30、大号展示 64
  size: {
    type: Number,
    default: 30
  },
  alt: {
    type: String,
    default: 'ATT'
  }
})
</script>

<style scoped>
.logo-mark {
  display: block;
  flex-shrink: 0;
  border-radius: var(--r-sm);
  /* 金色方块的边缘在深色底上会有一点锯齿感，用最轻一档阴影让它"坐"进背景 */
  box-shadow: var(--e-1);
  user-select: none;
  -webkit-user-drag: none;
}
</style>
