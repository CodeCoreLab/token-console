# 第三方组件与许可声明 — token-console（墨数 · WorkBuddy Token 消耗看板）

本文件列出本项目使用的第三方组件及其许可协议。

**本仓库不包含任何第三方二进制文件**——所有依赖一律通过语言原生的包管理机制
（`package.json` / `Cargo.toml`）在构建时还原。克隆本仓库后执行对应的构建命令，
即可获得完整的依赖树。

## 技术栈

Rust（Tauri 2）+ 纯前端 HTML/SVG 图表（零前端构建）

### Rust 侧

| 组件 | 版本 | 许可 | 用途 |
|---|---|---|---|
| [tauri](https://github.com/tauri-apps/tauri) | 2.12.5 | MIT / Apache-2.0 | 桌面壳、托盘、系统托盘菜单 |
| [tauri-build](https://github.com/tauri-apps/tauri) | 2.7.4 | MIT / Apache-2.0 | 构建脚本、权限清单生成 |
| [tauri-plugin-autostart](https://github.com/tauri-apps/plugins-workspace) | 2.7.0 | MIT / Apache-2.0 | 开机自启（写 `HKCU\Software\Microsoft\Run`，免管理员） |
| [serde](https://github.com/serde-rs/serde) | 1.0.229 | MIT / Apache-2.0 | 会话记录反序列化 |
| [serde_json](https://github.com/serde-rs/json) | 1.0.154 | MIT / Apache-2.0 | JSON 解析 |
| [walkdir](https://github.com/BurntSushi/walkdir) | 2.5.0 | MIT / Apache-2.0 | 递归扫描 `~/.workbuddy/projects/**/*.jsonl` |
| [chrono](https://github.com/chronotope/chrono) | 0.4.45 | MIT / Apache-2.0 | 按日聚合与时段分布 |

### 前端侧

| 组件 | 版本 | 许可 | 用途 |
|---|---|---|---|
| [@tauri-apps/cli](https://github.com/tauri-apps/tauri) | 2.12.4 | MIT / Apache-2.0 | 本地构建 CLI（devDependency） |

界面为自研的纯 HTML/CSS/SVG 实现，**不引入任何前端运行时框架或图表库**，
因此没有额外的浏览器端第三方许可需要登记。

## 附带的原生二进制

Tauri 在 Windows 下依赖系统自带的 **Microsoft Edge WebView2 Runtime**
（Win10 21H2+ / Win11 已预装），无需随包分发。

NSIS 安装包模板由 Tauri 官方构建链生成（Tauri 自身为 MIT / Apache-2.0），
不构成本仓的第三方依赖。
