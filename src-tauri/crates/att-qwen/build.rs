//! 构建脚本：只把自带的 C 桥 `src/shim.c` 编译成静态库。
//!
//! sherpa-onnx 本体**不在构建期链接**：`shim.c` 用 `LoadLibrary`/`dlopen` 在运行期按路径加载
//! `libsherpa-onnx-c-api`，所以交叉编译、CI 与没装模型/推理库的机器都能编译通过，
//! 缺库只在真正用到 Qwen 引擎时给出可读错误（见 [`att_qwen::LocalQwen::load`]）。

fn main() {
    println!("cargo:rerun-if-changed=src/shim.c");
    println!("cargo:rerun-if-changed=vendor/c-api.h");
    let mut build = cc::Build::new();
    build
        .file("src/shim.c")
        .include("vendor")
        .warnings(true)
        // sherpa-onnx 的 C 头是纯 C；MSVC 的 C 编译单元用 /W3 即可，避免第三方头的告警噪音
        .flag_if_supported("-Wno-unused-parameter");
    // 源文件里的错误文案是中文 UTF-8：MSVC 默认按系统代码页（GBK）解析会报「常量中有换行符」
    if build.get_compiler().is_like_msvc() {
        build.flag("/utf-8");
    } else {
        build.flag_if_supported("-finput-charset=UTF-8");
    }
    build.compile("att_qwen_shim");
}
