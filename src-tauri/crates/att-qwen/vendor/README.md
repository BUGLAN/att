# vendor/c-api.h

`sherpa-onnx` 的 C API 头文件，**原样**取自官方仓库：

- 来源：<https://github.com/k2-fsa/sherpa-onnx/blob/v2.0.1/sherpa-onnx/c-api/c-api.h>
- 版本：`v2.0.1`（与 `models/sherpa-onnx/` 里的预编译动态库、以及 README 里给的 release 资产一一对应）
- 许可：Apache-2.0（版权归 k2-fsa / sherpa-onnx 项目所有）

为什么要 vendor：

1. `src/shim.c` 直接 `#include "c-api.h"` 用官方结构体，避免在 Rust 侧手写几十个嵌套 `#[repr(C)]`
   结构体（字段错位会变成静默崩溃）；
2. 头文件版本必须与运行期 dlopen 的动态库版本**严格一致**，vendoring 才能保证这一点。

升级动态库时请同步替换本文件（同一个 tag 取），并重新跑 `cargo test -p att-qwen` 与真实模型测试。
