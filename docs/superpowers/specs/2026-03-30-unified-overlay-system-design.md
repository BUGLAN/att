# 统一悬浮层系统设计

## 背景

当前桌面悬浮能力由两套彼此独立的实现组成：

- 划词翻译悬浮框
- 剪切板悬浮框

两者分别维护自己的 `BrowserWindow` 生命周期、显示时序、焦点规则、关闭机制和 renderer 入口，已经暴露出以下问题：

- 显示链路分裂，难以稳定控制首屏时机
- 依赖额外 dismiss/backdrop 窗口，增加桌面级重绘和闪烁风险
- 焦点策略分散，`selection` 与 `clipboard` 的行为不一致
- 主进程和渲染层通信重复，维护成本高

本设计目标不是继续修补现有实现，而是重建一套统一的悬浮层系统。

## 目标

构建一个统一的桌面悬浮层系统，使用单一 `BrowserWindow` 和单一 renderer shell，承载两种模式：

- `selection`
- `clipboard`

新系统需要满足以下目标：

1. 统一窗口生命周期、显示时序、焦点管理、定位、拖动、钉住和关闭逻辑
2. 彻底移除当前额外的 dismiss/backdrop 子窗口
3. 保留当前产品能力差异
   - `selection` 为轻量动作面板
   - `clipboard` 为强交互操作面板
4. 消除“先 show、后 activation、再 resize、再补显示其他窗口”导致的闪烁源
5. 为后续扩展更多桌面悬浮能力提供统一基础设施

## 非目标

本次重构不包含以下内容：

- 不重做主应用窗口
- 不改动后端 API 语义
- 不新增新的 AI 动作类型
- 不把 `selection` 与 `clipboard` 融合成统一工作台
- 不同时支持两个独立悬浮窗实例并存

## 用户确认后的关键产品约束

本设计基于以下已确认决策：

- 采用“一个统一窗口，两种模式”
- `selection` 模式不抢焦点
- `clipboard` 模式打开时抢焦点
- `clipboard` 模式未钉住时失焦自动关闭
- `selection` 模式点击动作后自动关闭；无操作时不自动关闭
- `selection` 模式保留四个动作：翻译 / 解释 / 总结 / 改写
- 两种模式都保留拖动与钉住能力
- 完全移除当前 backdrop/dismiss 辅助窗口

## 总体架构

新系统拆分为三层：

### 1. 主进程统一悬浮层管理器

新增统一的 overlay manager，作为悬浮层唯一入口，负责：

- 单例 `BrowserWindow` 创建、复用、销毁
- `selection` / `clipboard` 模式切换
- 激活 payload 组装
- 位置与尺寸计算
- 显示与隐藏
- 焦点策略分流
- 关闭策略分流
- 钉住与拖动状态同步

该 manager 替代现有：

- `electron/selectionToolbar/index.js`
- `electron/clipboardOverlay/index.js`

### 2. 渲染层统一 Shell

renderer 端只保留一个悬浮入口页面和一个根组件，例如：

- `src/unified-overlay.html`
- `src/unified-overlay.js`
- `src/components/overlay/UnifiedOverlayWindow.vue`

Shell 负责：

- 接收当前模式与激活数据
- 维护公共顶栏与通用窗口控制
- 在两个业务面板间切换
- 统一处理进入态、关闭态和 ready 回执

### 3. 模式面板

业务面板按模式拆分：

- `SelectionPanel`
- `ClipboardPanel`

二者只关心各自内容和交互，不直接控制窗口生命周期。

## 核心设计原则

### 单实例窗口

系统在任意时刻只维护一个桌面悬浮窗口实例。

这意味着：

- 不支持 `selection` 与 `clipboard` 作为两个独立窗口同时存在
- 所有触发都由同一个 manager 接管
- 模式之间通过“切换”而不是“再开一个窗口”完成

### 显示前准备完成

新系统禁止“先显示空壳窗口，再补业务数据”的显示链路。

目标链路为：

1. 主进程准备完整 activation payload
2. renderer shell 接收并渲染目标模式
3. renderer 返回 ready
4. 主进程执行一次真正的显示

这条规则用于替代当前“show -> activate -> resize -> dismiss windows show”的多阶段可见化流程。

### 同模式内不频繁改窗尺寸

每个模式使用稳定尺寸，不在同一模式内因内容变化频繁驱动主进程 `resize`。

策略如下：

- `selection` 模式使用固定紧凑尺寸
- `clipboard` 模式使用固定较大尺寸
- 仅在 mode 切换时调整窗口尺寸
- 内容溢出由内部滚动承担

## 模式定义

### `selection` 模式

### 定位

`selection` 模式定义为轻量、持续停留、非抢焦点动作面板。

### 展示内容

- 当前选中文本
- 四个动作按钮：翻译 / 解释 / 总结 / 改写
- 关闭按钮
- 钉住按钮

### 焦点规则

- 打开时不主动抢焦点
- 不依赖焦点保持可用

### 关闭规则

- 无操作时不自动关闭
- 点击动作后：
  - 未钉住：动作完成后自动关闭
  - 已钉住：动作完成后保持打开
- 支持显式点击关闭
- 支持 `Esc` 关闭
- 不使用失焦关闭

### 拖动与钉住

- 支持拖动
- 支持钉住
- 钉住后转为半持久化工具窗

### `clipboard` 模式

### 定位

`clipboard` 模式定义为强交互、短驻留、抢焦点操作面板。

### 展示内容

- 搜索框
- 剪贴板列表
- 键盘导航状态
- 关闭按钮
- 钉住按钮

### 焦点规则

- 打开时主动聚焦搜索框
- 再次触发 `clipboard` 时不做 toggle hide，只刷新并重新聚焦

### 关闭规则

- 未钉住：失焦自动关闭
- 已钉住：失焦不关闭
- 支持显式点击关闭
- 支持 `Esc` 关闭

### 拖动与钉住

- 支持拖动
- 支持钉住

## 模式切换规则

### 空闲状态

- 收到 `selection` 触发：打开 `selection`
- 收到 `clipboard` 触发：打开 `clipboard`

### 当前为 `selection`

- 再次收到 `selection`
  - 更新选中文本与定位
  - 不销毁窗口
  - 不重建 renderer
- 收到 `clipboard`
  - 切换到 `clipboard`
  - 应用 `clipboard` 尺寸与焦点策略

### 当前为 `clipboard`

- 再次收到 `clipboard`
  - 刷新内容
  - 重新聚焦搜索框
  - 不 toggle 隐藏
- 收到 `selection`
  - 默认忽略
  - 不抢占当前 `clipboard` 强交互态

该规则用于保护剪贴板模式中的连续键盘操作不被划词提示打断。

## 主进程职责细化

统一 overlay manager 需要承担以下职责：

### 触发归一化

把当前分散的触发入口统一为标准命令：

- `openSelection(payload)`
- `openClipboard(payload)`
- `closeOverlay(reason)`
- `pinOverlay(active)`
- `updateDragState(active)`

### 位置策略

- `selection` 尽量靠近选区或触发点，但需要服从安全边界
- `clipboard` 使用稳定、可预测的位置策略
- 若存在用户拖动后的钉住位置，则优先复用

### 显示时序控制

- 创建窗口但不立即 show
- 向 renderer 下发激活数据
- 等待 renderer 的 ready 回执
- 一次性 show

### 通信契约

需要明确主进程与 renderer 的统一事件协议，例如：

- 主进程 -> renderer
  - `overlay:activate`
  - `overlay:pin-changed`
  - `overlay:mode-changed`
- renderer -> 主进程
  - `overlay:ready`
  - `overlay:close`
  - `overlay:pin`
  - `overlay:drag-state`
  - `overlay:action-complete`

具体命名可以在实现计划阶段定稿，但契约结构必须统一。

## 渲染层职责细化

统一 shell 负责以下状态：

- 当前 mode
- 当前 activation payload
- 当前 pinned 状态
- 当前 ready 状态
- 当前 panel 是否 busy

Shell 不直接承担业务动作本身，而是把业务行为下沉到 panel。

### `SelectionPanel`

负责：

- 展示选中文本
- 执行动作
- 呈现 loading / error / result
- 在动作完成后通知 shell 与主进程

### `ClipboardPanel`

负责：

- 加载剪贴板数据
- 搜索
- 键盘导航
- 复制与回填

## 闪烁问题的解决路径

新设计针对当前闪烁问题的处理不是做动画优化，而是移除根因：

### 根因 1：多窗口协同显示

旧实现通过额外 dismiss/backdrop 窗口覆盖工作区，导致桌面合成阶段出现第二轮显示。

新设计完全移除这些窗口。

### 根因 2：窗口先显示，内容后激活

旧实现存在 `show` 早于业务数据完成注入的问题。

新设计要求 renderer ready 后再 show。

### 根因 3：内容态驱动尺寸频繁变化

旧实现中同一模式内会随内容状态驱动 resize。

新设计只在 mode 切换时改尺寸。

## 错误处理

### `selection` 模式

- 动作执行失败时，面板内部展示错误态
- 未钉住时，错误态允许用户看见结果后自行关闭
- 已钉住时维持当前面板状态，不强制关闭

### `clipboard` 模式

- 数据加载失败时展示空态或错误态
- 不因一次加载失败导致窗口自动关闭
- 搜索与刷新失败需要保证键盘交互不崩溃

## 测试策略

需要把测试重点从单一组件渲染扩展到“模式切换 + 时序 + 焦点规则”。

### 主进程层

- 单例窗口创建与复用
- `selection` / `clipboard` 模式切换
- `clipboard -> selection` 忽略规则
- 钉住状态切换
- renderer ready 前不 show

### 渲染层

- Shell 在不同 mode 下渲染正确 panel
- `SelectionPanel` 动作成功 / 失败状态
- `ClipboardPanel` 搜索、键盘导航、复制行为
- `selection` 动作完成后的自动关闭通知

### 集成验证

- 首次打开 `selection` 不抢焦点
- 首次打开 `clipboard` 抢焦点
- `clipboard` 失焦关闭
- `selection` 不因失焦关闭
- 模式切换时窗口不闪烁、不重建

## 迁移策略

建议采用替换式迁移，而不是逐步在旧代码上叠补：

1. 新建统一 overlay 基础设施
2. 保留旧实现直到新实现可用
3. 把触发入口逐步改接到新 manager
4. 删除旧的 selection / clipboard 专用 manager 和入口页面

这样可以减少“旧逻辑和新逻辑互相污染”的风险。

## 预期交付结果

重构完成后，代码库中应形成以下新形态：

- 一个统一的 Electron 悬浮窗口 manager
- 一个统一的 overlay renderer 入口
- 两个业务 panel
- 一套统一的 IPC 协议
- 删除旧的 dismiss/backdrop 子窗口实现

## 风险

### 焦点行为平台差异

Windows 下 `showInactive()`、`focus()`、失焦事件的实际表现需要实机验证。

### 模式切换时旧状态残留

如果 shell 和 panel 的状态边界不清晰，容易出现：

- `selection` 结果残留到下一次激活
- `clipboard` 搜索状态污染下一次打开

因此状态重置必须是设计中的显式步骤，而不是依赖组件卸载碰运气。

### 现有入口迁移不完整

若主进程仍遗留旧入口、旧事件名、旧窗口管理器并行存在，会导致新旧系统竞争。

迁移计划中必须包含“彻底断开旧入口”的步骤。

## 结论

本设计选择以“单窗口、双模式、统一 shell、统一 manager”为核心重建桌面悬浮层系统。

它比继续修补当前实现更适合解决现有问题，因为它直接收敛了以下不稳定因素：

- 多窗口协同显示
- 首屏激活晚于 show
- 模式能力分裂
- 焦点与关闭规则不一致

在该设计下，`selection` 和 `clipboard` 仍保持不同产品体验，但底层基础设施完全统一。
