# sherpa-onnx 运行时资源

打包前由 `scripts/prepare-sherpa-onnx.mjs` 按平台下载、校验并解压 sherpa-onnx 共享库到这里。
动态库本身体积大且各平台不同，不入库；保留本说明与 `.gitignore` 让 Tauri 资源目录始终存在。
