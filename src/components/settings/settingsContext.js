// 设置页父子组件共享的上下文键：表单状态与自动保存逻辑只在 SettingsPage.vue 一处，
// 各分页组件经 inject 读写同一份 form（改动由父组件的 watch 统一防抖保存）。
export const SETTINGS_CTX = Symbol('att-settings-context')
