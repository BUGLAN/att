# macOS 托盘图标不显示问题修复计划

## 问题诊断

### 现象
- 运行 `python run_tray.py` 后程序正常启动
- WebSocket 连接成功，日志无错误
- 但 macOS 菜单栏右侧不显示托盘图标

### 根本原因分析

经过代码审查，发现以下可能的原因：

#### 1. 图标尺寸问题（最可能）
在 [`tray/main.py:98`](tray/main.py:98) 中，图标尺寸设置为 32x32：
```python
size = 32
```

macOS 状态栏图标推荐尺寸是 **16x16 或 22x22**（取决于显示器分辨率）。32x32 的图标可能被系统缩放或裁剪导致不可见。

#### 2. 图标颜色与背景融合
图标使用深灰色 `#52525b`，在 macOS 深色模式下可能与菜单栏背景融合，难以察觉。

#### 3. pystray 在 macOS 上的已知限制
pystray 使用 AppKit 框架创建状态栏图标，某些情况下需要显式设置图标可见性。

## 解决方案

### 方案一：修复图标尺寸和样式（推荐）

修改 [`tray/main.py`](tray/main.py) 中的 `_create_icon_image` 方法：

1. **调整图标尺寸**：将 32x32 改为 22x22（适合 Retina 显示器）
2. **增加图标对比度**：使用更明显的颜色
3. **添加白色边框**：确保在深色和浅色模式下都可见

### 方案二：使用 rumps 库替代 pystray

[rumps](https://github.com/jaredks/rumps) 是专门为 macOS 设计的状态栏应用库，更加可靠。

优点：
- 原生 macOS 支持
- 更好的菜单栏图标控制
- 更简单的事件处理

缺点：
- 仅支持 macOS，需要保留 pystray 作为其他平台的备选

### 方案三：调试当前实现

添加调试代码确认 pystray 是否正确初始化。

## 推荐修复步骤

### 步骤 1：修复图标尺寸和可见性

修改 [`tray/main.py`](tray/main.py) 的 `_create_icon_image` 方法：

```python
def _create_icon_image(self, status: str = "idle") -> Image.Image:
    # macOS 状态栏推荐尺寸
    size = 22  # 改为 22x22
    
    # 更明显的颜色
    colors = {
        "idle": "#4A90D9",      # 蓝色 - 更明显
        "recording": "#FF3B30",  # 红色
        "transcribing": "#FF9500",  # 橙色
        "disconnected": "#8E8E93"  # 灰色
    }
    
    # ... 其余代码
    
    # 添加白色边框增加可见性
    draw.ellipse(
        [center - radius - 1, center - radius - 1, center + radius + 1, center + radius + 1],
        outline="#FFFFFF",
        width=1
    )
```

### 步骤 2：添加调试日志

在 `run` 方法中添加更多调试信息：

```python
def run(self) -> None:
    # ... 现有代码 ...
    
    # Create tray icon
    self.tray_icon = pystray.Icon(
        "att_tray",
        icon=self._create_icon_image("disconnected"),
        menu=self._create_menu()
    )
    
    # 添加调试
    logger.info(f"Tray icon created: {self.tray_icon}")
    logger.info(f"Icon image size: {self.tray_icon.icon.size}")
    
    # Run tray icon (blocking)
    logger.info("Starting tray icon run loop...")
    self.tray_icon.run()
```

### 步骤 3：验证 pyobjc 安装

确保 pyobjc 正确安装：

```bash
pip install pyobjc-core pyobjc-framework-Cocoa
```

## 测试验证

1. 运行 `python run_tray.py`
2. 检查菜单栏右侧是否出现蓝色圆形图标
3. 点击图标验证菜单是否正常显示
4. 测试快捷键功能

## 备选方案：使用 rumps

如果上述修复无效，建议使用 rumps 库重写托盘功能：

```python
import rumps

class TrayApp(rumps.App):
    def __init__(self):
        super(TrayApp, self).__init__("ATT", icon=None, template=True)
        self.menu = ["状态: 空闲", None, "重连", "退出"]
```

## 文件修改清单

| 文件 | 修改内容 |
|------|----------|
| `tray/main.py` | 修复图标尺寸、颜色、添加调试日志 |
| `requirements.txt` | 可选：添加 rumps 依赖 |
