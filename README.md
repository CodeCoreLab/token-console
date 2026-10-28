# token-console — WorkBuddy Token 消耗看板（Tauri 2 桌面版）

> 在线产品页：<https://xmtooldev.gf.com.cn/token-console/>
> 版本记录（变更日志）：[`CHANGELOG.md`](CHANGELOG.md) · 发行版下载：<https://github.com/CodeCoreLab/token-console/releases>

把 `token-dashboard` skill（Python 生成单文件 HTML）产品化为独立桌面应用：
Rust 解析引擎 + 复用原 HTML/SVG 图表模板 + 系统托盘 + 开机自启。

## 功能

- 请求级真实 usage 统计：扫描 `~/.workbuddy/projects/**/*.jsonl` 的 `providerData.rawUsage/usage`
- KPI / 日历热力图 / 0-24 时模型分布 / 每日模型比例 / 工作空间→会话→请求三级下钻
- 系统托盘（左键双击唤起窗口；右键：显示看板 / 开机自启 / 退出）
- 关窗即隐藏到托盘；开机自启写 HKCU Run（免管理员）
- CLI 冒烟模式：`token-console.exe --scan [--scan-json out.json]`

## 产物

`src-tauri/target/release/token-console.exe` — **单文件约 5.6MB**，无外部运行时依赖
（渲染用系统 WebView2，Win10 21H2+ / Win11 自带）。

安装包：`src-tauri/target/release/bundle/nsis/token-console_0.1.0_x64-setup.exe`（**1.76MB**，
NSIS，含桌面/开始菜单快捷方式与卸载入口；`npx tauri build --bundles nsis` 重建）。

## 构建环境（本机已就位，迁移时按此备置）

本机 Rust 默认工具链为 `x86_64-pc-windows-gnu`（非 MSVC），踩坑后固化的最小环境：

| 组件 | 位置 | 用途 |
|---|---|---|
| Rust stable-gnu（rustup 自带） | `~/.rustup/toolchains/stable-x86_64-pc-windows-gnu` | 编译 + self-contained 链接器（libpthread 等） |
| MSYS2 binutils 2.47 | `~/.workbuddy/tools/mingw64-binutils/mingw64/bin` | `dlltool`（修 rustup 自带版 CreateProcess 崩溃）、`windres`（tauri-winres 资源编译）、`as` |
| MSYS2 gcc 16.2 + gmp/mpc/mpfr/isl/crt/headers/zlib/zstd 等 | 同上（已并入同一目录） | windres 预处理需要真 gcc |

构建命令（Git Bash）：

```bash
export PATH="$HOME/.workbuddy/tools/mingw64-binutils/mingw64/bin:$PATH"
cd src-tauri && cargo build --release
```

关键配置（已固化在仓库内）：

- `src-tauri/.cargo/config.toml`：链接器**钉死** rustup self-contained gcc，
  避免 PATH 中 MSYS2 gcc（缺 libpthread.a）被误选为链接器
- 免装 VS Build Tools / Windows SDK：整个链路只依赖上述用户目录工具

### 已知坑（本机实测）

1. rustup 自带 `dlltool.exe` 缺 `as` → CreateProcess 失败（MSYS2 dlltool 顶上）
2. `tauri-winres` 需要 `windres` + 真 `gcc` 做资源预处理
3. windows-gnu 下 cargo build script panic 信息会被吞（stderr 空）——
   定位要手动带全 env 复跑 build-script-build
4. `crate-type` 含 `cdylib` 时自带旧 ld 报 `export ordinal too large` → 桌面应用只用 `["rlib"]`
5. `cargo test` 的测试二进制在 windows-gnu 下加载失败（资源/清单链接问题）→
   验证走 `--scan` CLI 冒烟 + 与 Python 版对账，不依赖 cargo test

## 正确性对账（2026-10-26 实测，同刻双扫）

与 `token-dashboard/scripts/gen_dashboard.py` 同一时刻全量双扫对账
（1407 工作空间 / 4192 会话 / 46 模型）：

- **total_tokens 完全一致（delta = 0），0 个会话存在差异**
- 曾出现的 ±0.03% 差异为 TOCTOU 假差：两侧扫描时刻不同 + 用户会话在扫描间隙
  活跃写入（差异记录 minute 落在构建期间的 15:35-15:36）。同刻重扫即归零。

### 与 Python 版的口径差异（有意为之）

- 数值兼容更宽：浮点/数字字符串形式的 tokens 按 Python `int()` 语义截断（Python 会崩的场景此处跳过）
- `ch/cw` 缓存字段 0 值会落到嵌套兜底字段（Python `or` 链同语义）

## 待办（P1/P2）

- [x] 深查 4 会话差异根因 → 确认 TOCTOU 假差，同刻双扫 delta=0（2026-10-26）
- [ ] NSIS 安装包（`tauri-plugin-autostart` 已就位，打包配置在 `tauri.conf.json bundle`）
- [ ] 托盘图标升级为多尺寸 ico（当前 32x32 手工生成）
- [ ] WebView2 缺失兜底（`webviewInstallMode: embedBootstrapper`，面向 Win10 老版本）
