//! token-console 后端：扫描 ~/.workbuddy/projects/**/*.jsonl，
//! 抽取请求级 usage（providerData.rawUsage / usage），产出与
//! token-dashboard skill（gen_dashboard.py）同构的 DATA JSON。
//! 前端模板零改动复用，仅数据注入方式不同（Tauri invoke）。
//!
//! 与 Python 版的有意差异（缺陷修复）：
//! - 会话分组键 = 规范化后的会话绝对路径（`root/<ws>/<sid>`），不再随遍历深度变化；
//! - `r[0]`（分钟）保留秒级精度，不再整除丢秒；
//! - 展示 id = 长前缀 + 短哈希后缀，避免前 8 字符碰撞；
//! - 时区取系统本地时区，不再硬编码 UTC+8。

use serde_json::{json, Value};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use walkdir::WalkDir;

// ---------- 日志设施 ----------

/// 已初始化标志（幂等，防止重复安装 subscriber）。
static LOG_INIT: AtomicUsize = AtomicUsize::new(0);

/// 初始化 tracing：GUI 进程没有控制台，日志落到
/// `%LOCALAPPDATA%/token-console/logs/token-console.log`。
///
/// 只在 GUI 入口调用；`--scan` 冒烟模式仍走 stdout，不装 subscriber。
/// 文件打开失败时静默降级为无日志——诊断设施自身不能阻塞业务。
pub fn init_tracing() {
    if LOG_INIT.swap(1, Ordering::SeqCst) != 0 {
        return;
    }

    let dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join("AppData").join("Local"))
        .join("token-console")
        .join("logs");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let file = dir.join("token-console.log");
    let Ok(f) = fs::OpenOptions::new().create(true).append(true).open(&file) else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(f))
        .try_init();
}

/// 业务侧薄封装：未初始化 tracing 时（`--scan`/测试）不丢消息。
pub fn log_tracing(msg: &str) {
    if LOG_INIT.load(Ordering::SeqCst) == 0 {
        eprintln!("[token-console] {msg}");
        return;
    }
    tracing::warn!("{msg}");
}

// ---------- 路径 ----------

fn home_dir() -> PathBuf {
    if let Ok(p) = std::env::var("USERPROFILE") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("HOME") {
        return PathBuf::from(p);
    }
    PathBuf::from(".")
}

pub fn projects_dir() -> PathBuf {
    home_dir().join(".workbuddy").join("projects")
}

// ---------- 字段抽取（逐条对齐 gen_dashboard.py）----------

/// Python `x or y or 0`：0/None 落到下一个候选
fn nz(v: Option<i64>) -> Option<i64> {
    v.filter(|&x| x != 0)
}

/// 数值字段兼容：Python `int()` 能吃 int/float/数字字符串，serde as_i64 遇浮点返回 None
fn num(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s
            .trim()
            .parse::<i64>()
            .ok()
            .or_else(|| s.trim().parse::<f64>().ok().map(|f| f as i64)),
        _ => None,
    }
}

fn grab(o: &Value) -> Option<(i64, i64, i64, i64, i64, String)> {
    let pd = o.get("providerData")?.as_object()?;
    let ru = pd.get("rawUsage").and_then(|v| v.as_object());
    let u = pd.get("usage").and_then(|v| v.as_object());

    let tt = nz(num(ru.and_then(|m| m.get("total_tokens"))))
        .or_else(|| nz(num(u.and_then(|m| m.get("totalTokens")))))?;
    if tt == 0 {
        return None; // Python: `if not tt: return None`
    }

    // Python `or` 链语义：空串视为缺失，落到下一个候选；全空 → "unknown"
    let model = ["model", "requestModelId", "requestModelName"]
        .iter()
        .find_map(|k| pd.get(*k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
        .unwrap_or("unknown")
        .to_string();

    let ptd = ru
        .and_then(|m| m.get("prompt_tokens_details"))
        .and_then(|v| v.as_object());

    // 缓存命中：DeepSeek 风格顶层字段 → OpenAI 风格嵌套字段（claude/qwen/glm 兜底）
    let ch = nz(num(ru.and_then(|m| m.get("prompt_cache_hit_tokens"))))
        .or_else(|| num(ptd.and_then(|m| m.get("cached_tokens"))))
        .unwrap_or(0);
    // 缓存写入（Claude 按 1.25× 输入价计）
    let cw = ptd
        .and_then(|m| m.get("cached_creation_tokens").or_else(|| m.get("cache_write_tokens")))
        .and_then(|v| num(Some(v)))
        .unwrap_or(0);

    let it = nz(num(ru.and_then(|m| m.get("prompt_tokens"))))
        .or_else(|| nz(num(u.and_then(|m| m.get("inputTokens")))))
        .unwrap_or(0);
    let ot = nz(num(ru.and_then(|m| m.get("completion_tokens"))))
        .or_else(|| nz(num(u.and_then(|m| m.get("outputTokens")))))
        .unwrap_or(0);

    Some((tt, it, ot, ch, cw, model.to_string()))
}

/// 首条非 system-reminder 的用户消息前 42 字（跳过 `<` 开头的内部标签）
fn session_title(o: &Value) -> Option<String> {
    if o.get("role").and_then(|v| v.as_str()) != Some("user") {
        return None;
    }
    let squeeze = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    match o.get("content") {
        Some(Value::String(c)) => {
            let t = squeeze(c);
            if t.is_empty() || t.starts_with('<') {
                return None;
            }
            Some(t.chars().take(42).collect())
        }
        Some(Value::Array(segs)) => {
            for seg in segs {
                let Some(so) = seg.as_object() else { continue };
                let ty = so.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if ty != "input_text" && ty != "text" {
                    continue;
                }
                let t = so.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                if t.is_empty() {
                    continue;
                }
                if t.starts_with('<') {
                    return None;
                }
                let t = squeeze(t);
                return Some(t.chars().take(42).collect());
            }
            None
        }
        _ => None,
    }
}

/// 时间戳：秒/毫秒数字、数字字符串、多种 ISO 文本（按 UTC 解析，对齐 Python timegm）
fn parse_ts(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => {
            let x = n.as_f64()?;
            if x > 1e12 {
                Some(x / 1000.0)
            } else if x > 1e9 {
                Some(x)
            } else {
                None
            }
        }
        Value::String(s) => {
            let t = s.trim();
            if let Ok(x) = t.parse::<f64>() {
                // 数字字符串与 Number 分支走同一套阈值判断：>1e12 视为毫秒，
                // >1e9 视为秒，其余（如小得可疑的值）不采信。
                if x > 1e12 {
                    return Some(x / 1000.0);
                } else if x > 1e9 {
                    return Some(x);
                }
                return None;
            }
            for fmt in [
                "%Y-%m-%dT%H:%M:%S%.fZ",
                "%Y-%m-%dT%H:%M:%SZ",
                "%Y-%m-%d %H:%M:%S",
                "%Y-%m-%dT%H:%M:%S%.f",
            ] {
                if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(t, fmt) {
                    return Some(dt.and_utc().timestamp() as f64);
                }
            }
            None
        }
        _ => None,
    }
}

/// 工作空间目录名取 `-WorkBuddy-` 之后的部分作短名
fn short_ws(name: &str) -> String {
    if let Some(i) = name.rfind("-WorkBuddy-") {
        let rest = &name[i + 11..];
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    name.to_string()
}

/// 时区偏移：默认取系统本地时区（`Date` 语义），不再硬编码 UTC+8。
///
/// 可用环境变量 `TOKEN_CONSOLE_TZ_OFFSET` 覆盖（形如 `+09:00` / `-05:00`），
/// 用于跨时区查看或自动化验证。
fn tz_offset() -> chrono::FixedOffset {
    if let Ok(spec) = std::env::var("TOKEN_CONSOLE_TZ_OFFSET") {
        if let Ok(off) = spec.trim().parse::<chrono::FixedOffset>() {
            return off;
        }
    }
    *chrono::Local::now().offset()
}

/// 扫描主流程 ----------

/// 单条请求记录。
///
/// `minute` 为分钟（含秒的小数部分，保留秒级精度）；其余字段为整数计数，
/// 序列化后仍是 7 元数组，与前端 `r[0]..r[6]` 的取用方式一致。
#[derive(Clone, Copy)]
struct Rec {
    minute: f64,
    model_idx: i64,
    tt: i64,
    it: i64,
    ot: i64,
    ch: i64,
    cw: i64,
}

impl Rec {
    fn to_json(self) -> Value {
        json!([
            self.minute,
            self.model_idx,
            self.tt,
            self.it,
            self.ot,
            self.ch,
            self.cw
        ])
    }
}

/// 会话身份：只由规范化后的路径决定，与文件在目录树中的深度无关。
///
/// `<root>/<ws>/<sid>.jsonl` 与 `<root>/<ws>/<sid>/subagents/<agent>.jsonl`
/// 归到同一个会话 `<root>/<ws>/<sid>`；`.jsonl` 后缀在统一入口处剥掉，
/// 因此同一逻辑会话不会因为遍历深度不同被拆成两条记录。
struct SessionId {
    /// 工作空间目录名（原始名，非短名）
    ws: String,
    /// 会话槽位名（`<sid>.jsonl` → `<sid>`），用于展示
    slot: String,
    /// 规范化后的会话绝对路径，用作分组键（唯一且与深度无关）
    key: String,
    /// 是否为会话根文件（工作空间下的 `<sid>.jsonl` 本身）
    is_root: bool,
}

/// 根目录规范化（绝对路径 + 消解 `.`/`..`/符号链接）；失败时退回原路径。
fn normalized_root(root: &Path) -> PathBuf {
    fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

fn session_id(root: &Path, path: &Path) -> Option<SessionId> {
    let rel = path.strip_prefix(root).ok()?;
    let comps: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    if comps.len() < 2 {
        return None;
    }
    let ws = comps[0].clone();
    if ws.is_empty() {
        return None;
    }
    // 会话槽位恒为工作空间下的第一段：文件 `<sid>.jsonl` 与目录 `<sid>/` 同名同槽位。
    let slot = comps[1].strip_suffix(".jsonl").unwrap_or(comps[1].as_str());
    if slot.is_empty() {
        return None;
    }
    let key = root.join(&ws).join(slot);
    Some(SessionId {
        ws,
        slot: slot.to_string(),
        key: key.to_string_lossy().to_string(),
        is_root: comps.len() == 2,
    })
}

/// 一个逻辑会话的聚合结果（同一会话的根文件与 subagents 派生文件合并到此）。
struct Session {
    /// 工作空间下标
    ws: usize,
    /// 会话槽位名（展示用）
    slot: String,
    /// 会话标题（根文件优先）
    title: Option<String>,
    /// 请求级记录
    recs: Vec<Rec>,
}

/// FNV-1a 64 位：给展示 id 补一段稳定短哈希，让同前缀会话可区分。
fn short_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (h & 0xffff_ffff) as u32)
}

/// 展示用会话 id：长前缀 + 短哈希后缀（不再只靠前 8 字符，避免碰撞）。
fn display_id(slot: &str, key: &str) -> String {
    let head: String = slot.chars().take(12).collect();
    format!("{head}-{}", short_hash(key))
}

pub fn scan_usage_json(projects: Option<PathBuf>) -> Result<Value, String> {
    let root = projects.unwrap_or_else(projects_dir);
    if !root.is_dir() {
        return Err(format!("数据目录不存在: {}", root.display()));
    }
    // 规范化根目录：分组键用的是规范化后的绝对路径，与遍历深度无关。
    let root = normalized_root(&root);

    let mut ws_names: Vec<String> = Vec::new();
    let mut ws_idx: HashMap<String, usize> = HashMap::new();
    let mut models: Vec<String> = Vec::new();
    let mut model_idx: HashMap<String, usize> = HashMap::new();
    // 会话分组键 = 规范化绝对路径（与遍历深度无关）；值 = (ws_idx, 标题, 记录)
    let mut sessions: HashMap<String, Session> = HashMap::new();

    // 不可读目录/文件计数：完全吞掉会让"数据变少"无从排查。
    let mut walk_errors: usize = 0;
    for entry in WalkDir::new(&root) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                walk_errors += 1;
                log_tracing(&format!("walkdir 跳过不可读条目: {}", e));
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        // 全仓唯一的会话 id 计算入口：只看规范化路径的前两段，与深度无关。
        let Some(sid) = session_id(&root, path) else {
            continue;
        };
        let w = match ws_idx.entry(sid.ws.clone()) {
            Entry::Vacant(v) => {
                let i = ws_names.len();
                ws_names.push(short_ws(&sid.ws));
                v.insert(i);
                i
            }
            Entry::Occupied(o) => *o.get(),
        };

        let mut recs: Vec<Rec> = Vec::new();
        let mut title: Option<String> = None;
        let file = fs::File::open(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(o) = serde_json::from_str::<Value>(line) else { continue };
            if !o.is_object() {
                continue;
            }
            if title.is_none() {
                title = session_title(&o);
            }
            let Some(g) = grab(&o) else { continue };
            let (tt, it, ot, ch, cw, model) = g;
            let model = if model == "glm-5.2-x" { "glm-5.2".to_string() } else { model };
            let Some(ts) = parse_ts(o.get("timestamp")) else { continue };
            let mi = match model_idx.get(&model) {
                Some(&i) => i,
                None => {
                    let i = models.len();
                    models.push(model);
                    model_idx.insert(models[i].clone(), i);
                    i
                }
            };
            // 时间桶保留秒：先归一到毫秒再换算成分钟，不做整除截断。
            let ts_ms = (ts * 1000.0).round() as i64;
            recs.push(Rec {
                minute: ts_ms as f64 / 60000.0,
                model_idx: mi as i64,
                tt,
                it,
                ot,
                ch,
                cw,
            });
        }
        if recs.is_empty() {
            continue;
        }

        match sessions.entry(sid.key.clone()) {
            Entry::Vacant(v) => {
                v.insert(Session {
                    ws: w,
                    slot: sid.slot,
                    title,
                    recs,
                });
            }
            Entry::Occupied(mut o) => {
                let s = o.get_mut();
                s.recs.extend(recs);
                // 会话根文件（工作空间下的 <sid>.jsonl）的标题优先于派生文件。
                if sid.is_root && title.is_some() {
                    s.title = title;
                }
            }
        }
    }

    if walk_errors > 0 {
        log_tracing(&format!(
            "扫描 {} 时有 {walk_errors} 个条目不可读，已跳过",
            root.display()
        ));
    }

    let mut sess_list: Vec<Value> = Vec::new();
    for (key, s) in &sessions {
        if s.recs.is_empty() {
            continue;
        }
        let mut r = s.recs.clone();
        r.sort_by(|a, b| a.minute.partial_cmp(&b.minute).unwrap_or(std::cmp::Ordering::Equal));
        let id = display_id(&s.slot, key);
        sess_list.push(json!({
            "w": s.ws,
            "id": id,
            "t": s.title.clone().unwrap_or_else(|| "(无标题会话)".to_string()),
            "st": r[0].minute,
            "r": r.iter().map(|x| x.to_json()).collect::<Vec<_>>(),
        }));
    }
    sess_list.sort_by(|a, b| {
        a["st"]
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&b["st"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 时区跟随系统本地时区（不再硬编码 UTC+8）。
    let now = chrono::Utc::now().with_timezone(&tz_offset());

    Ok(json!({
        "genMs": now.timestamp_millis(),
        "gen": now.format("%Y-%m-%d %H:%M").to_string(),
        "ws": ws_names,
        "models": models,
        "sess": sess_list,
    }))
}

// ---------- Tauri command ----------

#[tauri::command]
fn scan_usage() -> Result<Value, String> {
    scan_usage_json(None)
}

// ---------- 托盘 + 自启 ----------

use std::sync::Mutex;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

struct AutostartMenuItem(Mutex<Option<CheckMenuItem<tauri::Wry>>>);

pub fn run() {
    // GUI 入口装日志（GUI 无控制台，错误只能落文件）；--scan 分支保持 stdout。
    init_tracing();
    tracing::info!("token-console 启动");

    // CLI 冒烟模式：token-console.exe --scan → 打印统计摘要退出（无 GUI）
    if std::env::args().any(|a| a == "--scan") {
        match scan_usage_json(None) {
            Ok(v) => {
                // --scan-json <path>：导出完整 DATA JSON（与 Python 版对账用）
                if let Some(p) = std::env::args().position(|a| a == "--scan-json") {
                    if let Some(path) = std::env::args().nth(p + 1) {
                        let _ = fs::write(&path, serde_json::to_string(&v).unwrap_or_default());
                    }
                }
                let sess = v["sess"].as_array().cloned().unwrap_or_default();
                let total: i64 = sess
                    .iter()
                    .filter_map(|s| s["r"].as_array())
                    .flat_map(|r| r.iter())
                    .filter_map(|x| x.get(2).and_then(|v| v.as_i64()))
                    .sum();
                println!(
                    "OK sessions={} models={} ws={} total_tokens={}",
                    sess.len(),
                    v["models"].as_array().map(|a| a.len()).unwrap_or(0),
                    v["ws"].as_array().map(|a| a.len()).unwrap_or(0),
                    total
                );
            }
            Err(e) => {
                eprintln!("ERR {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![scan_usage])
        .on_window_event(|window, event| {
            // 关窗 = 隐藏到托盘，退出走托盘菜单
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .setup(|app| {
            use tauri_plugin_autostart::ManagerExt;

            let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);
            let show = MenuItem::with_id(app, "show", "显示看板", true, None::<&str>)?;
            let autostart =
                CheckMenuItem::with_id(app, "autostart", "开机自启", true, autostart_on, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &autostart, &quit])?;
            app.manage(AutostartMenuItem(Mutex::new(Some(autostart.clone()))));

            TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().expect("no window icon").clone())
                .tooltip("Token 消耗看板")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    "autostart" => {
                        let al = app.autolaunch();
                        let cur = al.is_enabled().unwrap_or(false);
                        let next = !cur;
                        let _ = if next { al.enable() } else { al.disable() };
                        if let Some(state) = app.try_state::<AutostartMenuItem>() {
                            if let Some(item) = state.0.lock().unwrap().clone() {
                                let _ = item.set_checked(next);
                            }
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ---------- 测试 ----------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;
    use std::sync::Mutex;

    /// 时区相关测试会改环境变量，需串行执行
    static TZ_LOCK: Mutex<()> = Mutex::new(());

    /// 自建临时数据目录，替代对真实 `~/.workbuddy/projects` 的依赖。
    /// 结构：root/<工作空间>/<会话id>.jsonl。
    fn fixture_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("token-console-test-{}", name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_session(root: &Path, ws: &str, sid: &str, ts: &str, model: &str) {
        let ws_dir = root.join(ws);
        fs::create_dir_all(&ws_dir).unwrap();
        let line = json!({
            "sessionId": sid,
            "role": "user",
            "content": "帮我看看这个测试会话的标题",
            "timestamp": ts,
            "providerData": {
                "model": model,
                "rawUsage": {
                    "total_tokens": 100,
                    "prompt_tokens": 80,
                    "completion_tokens": 20,
                    "prompt_cache_hit_tokens": 60,
                    "prompt_tokens_details": {"cached_tokens": 0}
                }
            }
        });
        fs::write(ws_dir.join(format!("{sid}.jsonl")), format!("{line}\n")).unwrap();
    }

    #[test]
    fn scan_fixture_dir_yields_isomorphic_structure() {
        let root = fixture_dir("scan");
        write_session(&root, "ws-alpha", "sess-1111", "2026-10-26T05:00:00Z", "glm-5.2-x");
        write_session(&root, "ws-alpha", "sess-2222", "2026-10-26T06:00:00Z", "deepseek-v4");
        write_session(&root, "ws-beta", "sess-3333", "2026-10-26T07:00:00Z", "kimi");

        let v = scan_usage_json(Some(root.clone())).expect("scan failed");
        let sess = v["sess"].as_array().expect("sess array");
        assert_eq!(sess.len(), 3, "all three sessions scanned");
        // 字段结构与 Python 版同构
        let s0 = &sess[0];
        assert!(s0.get("w").is_some() && s0.get("id").is_some() && s0.get("t").is_some());
        assert_eq!(s0["r"].as_array().unwrap()[0].as_array().unwrap().len(), 7);
        // 工作空间短名与模型归并
        let ws = v["ws"].as_array().unwrap();
        assert!(ws.iter().any(|w| w == "ws-alpha"));
        assert!(v["models"].as_array().unwrap().iter().any(|m| m == "glm-5.2"));
        assert!(!v["models"].as_array().unwrap().iter().any(|m| m == "glm-5.2-x"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_missing_dir_is_an_error() {
        let root = std::env::temp_dir().join("token-console-test-nonexistent");
        let _ = fs::remove_dir_all(&root);
        assert!(scan_usage_json(Some(root)).is_err());
    }

    /// 写一条 usage 记录到指定文件。
    fn write_usage_line(path: &Path, session_id: &str, ts: &str, model: &str) {
        let line = json!({
            "sessionId": session_id,
            "role": "user",
            "content": "标题内容",
            "timestamp": ts,
            "providerData": {
                "model": model,
                "rawUsage": {
                    "total_tokens": 100,
                    "prompt_tokens": 80,
                    "completion_tokens": 20,
                    "prompt_cache_hit_tokens": 60,
                    "prompt_tokens_details": {"cached_tokens": 0}
                }
            }
        });
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(path, format!("{line}\n")).unwrap();
    }

    /// A27：同一逻辑会话出现在不同目录深度（根文件 + subagents 派生文件）时，
    /// 必须归并成**一条**记录，而不是被拆成两条。
    ///
    /// 旧实现下，深度 2 的记录用 JSON 里的 `sessionId` 命名，深度 4 的记录用
    /// 目录名命名；两者不同时同一条会话被拆成两条独立记录。
    #[test]
    fn a27_same_session_at_different_depths_merges_into_one_record() {
        let root = fixture_dir("a27-depth");
        let ws_dir = root.join("ws-alpha");
        // 深度 2：工作空间下的会话根文件（JSON 里的 sessionId 与文件名不同）
        write_usage_line(
            &ws_dir.join("sess-abc.jsonl"),
            "legacy-id-999",
            "2026-10-26T05:00:00Z",
            "glm-5.2",
        );
        // 深度 4：<sid>/subagents/agent-*.jsonl（其 JSON 内 sessionId 是子代理自己的）
        write_usage_line(
            &ws_dir.join("sess-abc").join("subagents").join("agent-x.jsonl"),
            "agent-x-own-id",
            "2026-10-26T05:10:00Z",
            "glm-5.2",
        );

        let v = scan_usage_json(Some(root.clone())).expect("scan failed");
        let sess = v["sess"].as_array().expect("sess array");
        assert_eq!(
            sess.len(),
            1,
            "同一会话在两种深度下必须合并为一条记录，实际: {:?}",
            sess
        );
        // 两条 usage 都归到这一条记录里
        assert_eq!(sess[0]["r"].as_array().unwrap().len(), 2);
        // 展示 id 基于路径槽位，不带 .jsonl 后缀
        let id = sess[0]["id"].as_str().unwrap();
        assert!(id.starts_with("sess-abc"), "展示 id 应来自会话槽位: {id}");

        let _ = fs::remove_dir_all(&root);
    }

    /// A27：会话 id 计算入口与遍历深度无关——同一路径无论出现在哪一层，
    /// 计算出的分组键都相同（根文件与派生文件同组）。
    #[test]
    fn a27_session_key_is_depth_independent() {
        let root = fixture_dir("a27-key");
        let ws_dir = root.join("ws-alpha");
        let root_file = ws_dir.join("sess-abc.jsonl");
        let sub_file = ws_dir.join("sess-abc").join("subagents").join("agent-x.jsonl");
        fs::create_dir_all(&ws_dir).unwrap();

        let a = session_id(&root, &root_file).expect("root file id");
        let b = session_id(&root, &sub_file).expect("sub file id");
        assert_eq!(a.key, b.key, "不同深度的同一会话必须得到相同分组键");
        assert_eq!(a.slot, b.slot);
        assert!(a.is_root && !b.is_root);

        // 不同会话槽位必须得到不同分组键
        let c = session_id(&root, &ws_dir.join("sess-other.jsonl")).expect("other id");
        assert_ne!(a.key, c.key);

        let _ = fs::remove_dir_all(&root);
    }

    /// A27：不同工作空间下的同名会话不能被合并到同一条记录。
    #[test]
    fn a27_same_slot_in_different_workspaces_stays_separate() {
        let root = fixture_dir("a27-ws");
        write_usage_line(
            &root.join("ws-a").join("sess-x.jsonl"),
            "sess-x",
            "2026-10-26T05:00:00Z",
            "glm-5.2",
        );
        write_usage_line(
            &root.join("ws-b").join("sess-x.jsonl"),
            "sess-x",
            "2026-10-26T06:00:00Z",
            "glm-5.2",
        );

        let v = scan_usage_json(Some(root.clone())).expect("scan failed");
        assert_eq!(v["sess"].as_array().unwrap().len(), 2);

        let _ = fs::remove_dir_all(&root);
    }

    /// A28①：展示 id 不能只靠前 8 字符——前 12 字符相同的两个会话要能区分开。
    #[test]
    fn a28_display_id_distinguishes_same_prefix_sessions() {
        let root = fixture_dir("a28-id");
        write_usage_line(
            &root.join("ws-a").join("aaaaaaaaaaaa-1.jsonl"),
            "aaaaaaaaaaaa-1",
            "2026-10-26T05:00:00Z",
            "glm-5.2",
        );
        write_usage_line(
            &root.join("ws-a").join("aaaaaaaaaaaa-2.jsonl"),
            "aaaaaaaaaaaa-2",
            "2026-10-26T06:00:00Z",
            "glm-5.2",
        );

        let v = scan_usage_json(Some(root.clone())).expect("scan failed");
        let sess = v["sess"].as_array().unwrap();
        assert_eq!(sess.len(), 2);
        let id0 = sess[0]["id"].as_str().unwrap();
        let id1 = sess[1]["id"].as_str().unwrap();
        assert_ne!(id0, id1, "展示 id 必须能区分同前缀会话: {id0} vs {id1}");
        // 前 8 字符相同（模拟旧实现无法区分的场景）
        assert_eq!(&id0[..8], &id1[..8]);
        // 带短哈希后缀
        assert!(id0.contains('-') && id0.len() > 8, "展示 id 应带 hash 后缀: {id0}");

        let _ = fs::remove_dir_all(&root);
    }

    /// A28②：时间桶不得整除丢秒——秒级余量必须保留下来。
    #[test]
    fn a28_time_bucket_keeps_seconds() {
        let root = fixture_dir("a28-sec");
        write_usage_line(
            &root.join("ws-a").join("sess-sec.jsonl"),
            "sess-sec",
            "2026-10-26T05:00:37Z",
            "glm-5.2",
        );

        let v = scan_usage_json(Some(root.clone())).expect("scan failed");
        let r = &v["sess"][0]["r"][0];
        let minute = r[0].as_f64().expect("r[0] 应为数字");
        // 还原成秒后应包含 37 秒余量（整除实现会得到 0）
        let sec_in_minute = (minute - minute.floor()) * 60.0;
        assert!(
            (sec_in_minute - 37.0).abs() < 0.01,
            "时间桶丢失了秒级余量: minute={minute} 余量={sec_in_minute}"
        );
        // 整分钟的时间戳仍应落在整分钟上（向下取整到分钟起点）
        let base = minute.floor();
        assert!(
            (minute - base).abs() < 1.0,
            "分钟基线不应漂移: {minute}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// A28③：时区不再硬编码 UTC+8——渲染出的 "gen" 跟随配置/本地时区。
    #[test]
    fn a28_timezone_is_not_hardcoded_utc_plus_8() {
        // 串行化：两个时区测试都会改环境变量
        let _guard = TZ_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = fixture_dir("a28-tz");
        write_usage_line(
            &root.join("ws-a").join("sess-tz.jsonl"),
            "sess-tz",
            "2026-10-26T05:00:00Z",
            "glm-5.2",
        );

        // 用 +09:00 渲染：与 UTC+8 相差一小时，缺陷实现（写死 +08:00）必失败。
        std::env::set_var("TOKEN_CONSOLE_TZ_OFFSET", "+09:00");
        let v = scan_usage_json(Some(root.clone())).expect("scan failed");
        std::env::remove_var("TOKEN_CONSOLE_TZ_OFFSET");

        let gen = v["gen"].as_str().expect("gen");
        let gen_ms = v["genMs"].as_i64().expect("genMs");
        let expect = chrono::DateTime::from_timestamp_millis(gen_ms)
            .expect("genMs -> datetime")
            .with_timezone(&chrono::FixedOffset::east_opt(9 * 3600).unwrap())
            .format("%Y-%m-%d %H:%M")
            .to_string();
        assert_eq!(gen, expect, "gen 应按 +09:00 渲染，而不是写死的 +08:00");

        let _ = fs::remove_dir_all(&root);
    }

    /// A28③：未显式配置时区时跟随系统本地时区。
    #[test]
    fn a28_timezone_defaults_to_local() {
        let _guard = TZ_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("TOKEN_CONSOLE_TZ_OFFSET");
        let local = *chrono::Local::now().offset();
        assert_eq!(
            tz_offset().local_minus_utc(),
            local.local_minus_utc(),
            "默认时区应为系统本地时区"
        );
    }

    #[test]
    fn grab_parses_deepseek_and_openai_styles() {
        let mut o = Map::new();
        o.insert(
            "providerData".into(),
            json!({
                "model": "glm-5.2-x",
                "rawUsage": {
                    "total_tokens": 100, "prompt_tokens": 80, "completion_tokens": 20,
                    "prompt_cache_hit_tokens": 60,
                    "prompt_tokens_details": {"cached_tokens": 0}
                }
            }),
        );
        let (tt, it, ot, ch, _cw, model) = grab(&Value::Object(o)).unwrap();
        assert_eq!((tt, it, ot, ch), (100, 80, 20, 60));
        assert_eq!(model, "glm-5.2-x"); // merge 在 scan 层做
    }

    #[test]
    fn parse_ts_handles_ms_and_iso() {
        assert_eq!(parse_ts(Some(&json!(1_700_000_000_000i64))), Some(1_700_000_000.0));
        assert_eq!(
            parse_ts(Some(&json!("2026-10-26T05:00:00Z"))),
            Some(
                chrono::NaiveDateTime::parse_from_str("2026-10-26T05:00:00", "%Y-%m-%dT%H:%M:%S")
                    .unwrap()
                    .and_utc()
                    .timestamp() as f64
            )
        );
    }

    #[test]
    fn parse_ts_numeric_string_applies_thresholds() {
        // 数字字符串与 Number 分支同一套阈值：毫秒串降级为秒。
        assert_eq!(
            parse_ts(Some(&json!("1700000000000"))),
            Some(1_700_000_000.0)
        );
        // 秒级串原样采信。
        assert_eq!(parse_ts(Some(&json!("1700000000"))), Some(1_700_000_000.0));
        // 小于 1e9 的值不采信（既非毫秒也非秒级时间戳）。
        assert_eq!(parse_ts(Some(&json!("12345"))), None);
        assert_eq!(parse_ts(Some(&json!("12345"))), None);
    }
}
