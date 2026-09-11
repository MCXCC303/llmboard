# LLMBoard

## 编译

### 依赖
- Rust 1.97+
- wasm 依赖
- Python 3

```bash
# 编译 wasm
cargo build --release

# 打包为 abp 插件
python3 scripts/build_dist.py --release --package
```

## 新增供应商

- **纯 JSON(零代码)**:把符合规范的 JSON 放入 `presets/` 并重新打包即可(`presets/` 整目录随 abp)。
- **自定义逻辑(fork 模式)**:实现代码内预设并在 `src/capabilities/mod.rs` 注册表登记一行,预设端点以
  `"fetcher": "<能力名>"` 引用;启动日志会审计接线(`[capabilities] 启动审计通过`)。
- 文档:规范 [docs/PRESETS.md](../docs/PRESETS.md) · fork 全流程 [docs/EXTENSIONS.md](../docs/EXTENSIONS.md) ·
  架构分析 [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md)。
