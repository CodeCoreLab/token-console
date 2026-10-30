# 变更日志 — Token 消耗看板（token-console）

本文件记录本项目的变更。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [未发布]

### 修复

- GUI 进程零日志（全仓仅 `--scan` 路径 2 处 `println`）→ 引入 tracing +
  tracing-appender，日志按日轮转落 `%LOCALAPPDATA%\token-console\logs\`；
  目录遍历 / 会话解析失败 / 汇总关键路径分级落日志
- `tauri.conf.json` 的 `csp` 为 `null`（无任何内容安全策略）→ 启用完整 CSP
  （`default-src 'self'; script-src 'self'` 等）；为此把内联 500 行脚本
  外置为 `ui/app.js`（内联脚本与 `script-src 'self'` 不兼容）
- `esc()` 不转义单引号，单引号包裹的属性可被注入 → 补 `&#39;`，
  并把相关属性改用双引号
- `parse_ts` 字符串分支无条件按毫秒 (`/1000`) 处理，秒级数字字符串被
  归入错误的桶 → 与 Number 分支共用 `1e12 / 1e9` 阈值判断
- 目录遍历用 `filter_map(|e| e.ok())` 静默丢弃读取失败的目录 →
  计数并在结果中标注，避免统计口径悄悄缩水
- 单测依赖真实 `~/.workbuddy/projects` 数据 → fixture 化，测试可脱离本机数据

## [0.1.0] - 2026-10-26

### 新增

- 首个发布版本：WorkBuddy 专用 Token 消耗桌面应用（Tauri 2 单文件）
- **请求级真实 usage 统计**：扫描 `~/.workbuddy/projects/**/*.jsonl` 的
  `providerData.rawUsage` / `usage`
- **KPI**：总 Token、请求数、缓存命中率、估算费用
- **日历热力图**：按日聚合消耗强度
- **时段 × 模型分布**：0–24 小时 × 模型的双维视图
- **每日模型比例**：按日统计各模型占比
- **三级下钻**：工作空间 → 会话 → 请求
- **系统托盘**：左键双击唤起窗口，右键菜单提供显示看板 / 开机自启 / 退出；
  关窗即隐藏到托盘
- **开机自启**：写 `HKCU\Software\Microsoft\Run`，免管理员
- **CLI 冒烟模式**：`token-console.exe --scan [--scan-json out.json]`
- NSIS 安装包 `token-console_0.1.0_x64-setup.exe`（含桌面 / 开始菜单快捷方式
  与卸载入口）

### 安全

- 数据只读本机目录，不出机器；仅监听回环地址，无外部网络请求
