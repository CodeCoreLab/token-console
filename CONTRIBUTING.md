# 贡献指南

感谢你愿意为 token-console（墨数）出力。

本项目是 **Rust + Tauri 2** 的 Windows 桌面工具，
工程要求只有一条：**能跑、测得住、说得清**。

---

## 快速判断：改哪个目录

| 你想改的东西 | 去哪 |
|---|---|
| 采集引擎（扫描会话、算 usage） | `src-tauri/src/lib.rs` |
| 托盘 / 窗口行为 | `src-tauri/src/main.rs` |
| 应用配置、权限、窗口尺寸 | `src-tauri/tauri.conf.json` |
| 界面（纯 HTML/CSS/SVG） | `ui/index.html` |
| 产品介绍页 | `landing.html` |
| 依赖与许可声明 | `THIRD-PARTY-NOTICES.md` |
| 变更记录 | `CHANGELOG.md` |
| Issue / PR 模板 | `.github/` |

---

## 版本号只有一个权威源

`package.json` 的 `version` 是本仓的**唯一版本真相源**：

| 位置 | 值 | 说明 |
|---|---|---|
| `package.json` | `0.1.0` | **权威源**，改这里 |
| `src-tauri/Cargo.toml` | `version = "0.1.0"` | 跟随 `package.json`，勿手工改 |
| `src-tauri/tauri.conf.json` | `"version": "0.1.0"` | 跟随 `package.json`，勿手工改 |

发版时 `CHANGELOG.md` 必须新增对应的版本段，格式见该文件头部说明。

---

## 开发环境

| 必需 | 版本 | 用途 |
|---|---|---|
| Rust | stable-x86_64-pc-windows-gnu | 编译（Tauri 2） |
| Node.js | 22.x | 仅用于 `tauri` CLI |

不需要数据库、不需要联网服务。**渲染依赖系统自带 WebView2**，
Win10 21H2+ / Win11 已预装。

### 构建

```bash
npm install
cd src-tauri && cargo build --release
```

安装包：

```bash
npx tauri build --bundles nsis
```

> 本机 Rust 默认工具链为 `x86_64-pc-windows-gnu`（非 MSVC），
> `src-tauri/.cargo/config.toml` 已把链接器钉死到 rustup 自带的 self-contained gcc，
> 避免 PATH 中的 MSYS2 gcc 被误选。请勿随意改动该文件。

---

## 验证要求

改完必须实测，**不接受「看起来对」**：

```bash
src-tauri/target/release/token-console.exe --scan
src-tauri/target/release/token-console.exe --scan --scan-json out.json
```

数值改动必须**与 Python 版对账**（`token-dashboard/scripts/gen_dashboard.py`
同一时刻全量双扫），要求 `total_tokens` 完全一致（delta = 0）、
会话级零差异。

> `cargo test` 在 windows-gnu 工具链下测试二进制会加载失败
> （资源/清单链接问题），验证走 `--scan` CLI 冒烟 + 对账，不依赖 cargo test。

---

## 提交约定

| 类型 | 前缀 | 例子 |
|---|---|---|
| 功能 | `feat:` | `feat: 支持 Claude 会话格式解析` |
| 修复 | `fix:` | `fix: 跨月边界下日历热力图日期错位` |
| 构建 | `build:` | `build: 版本源对齐 0.1.0 → 0.1.1` |
| 文档 | `docs:` | `docs: 补充 CLI 参数说明` |
| 版本记录 | `docs(changelog):` | 由发版脚本自动生成 |

**跨项目的改动**请开一个 PR 只做这件事，不要夹带在功能 PR 里。

---

## 许可证

MIT，见 [`LICENSE`](LICENSE)。新增依赖请登记到
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md)。
