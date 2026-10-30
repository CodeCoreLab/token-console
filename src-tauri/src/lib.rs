//! token-console 后端：扫描 ~/.workbuddy/projects/**/*.jsonl，
//! 抽取请求级 usage（providerData.rawUsage / usage），产出与
//! token-dashboard skill（gen_dashboard.py）完全同构的 DATA JSON。
//! 前端模板零改动复用，仅数据注入方式不同（Tauri invoke）。

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

// ---------- 扫描主流程 ----------

type Rec = [i64; 7]; // [minute, model_idx, tt, it, ot, ch, cw]

pub fn scan_usage_json(projects: Option<PathBuf>) -> Result<Value, String> {
    let root = projects.unwrap_or_else(projects_dir);
    if !root.is_dir() {
        return Err(format!("数据目录不存在: {}", root.display()));
    }

    let mut ws_names: Vec<String> = Vec::new();
    let mut ws_idx: HashMap<String, usize> = HashMap::new();
    let mut models: Vec<String> = Vec::new();
    let mut model_idx: HashMap<String, usize> = HashMap::new();
    // sid -> (ws_idx, title, recs)
    let mut sessions: HashMap<String, (usize, Option<String>, Vec<Rec>)> = HashMap::new();

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
        let Ok(rel) = path.strip_prefix(&root) else { continue };
        let comps: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        if comps.len() < 2 {
            continue;
        }
        let folder = comps[0].clone();
        // 二级文件 = 会话文件（取 stem）；三级及更深（subagents/agent-*.jsonl）归属父会话
        let mut sid = if comps.len() == 2 {
            Path::new(&comps[1])
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| comps[1].clone())
        } else {
            comps[1].clone()
        };
        let w = match ws_idx.entry(folder.clone()) {
            Entry::Vacant(v) => {
                let i = ws_names.len();
                ws_names.push(short_ws(&folder));
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
            if comps.len() == 2 {
                if let Some(sidr) = o.get("sessionId").and_then(|v| v.as_str()) {
                    if !sidr.is_empty() {
                        sid = sidr.to_string(); // 子代理文件保持归属父会话
                    }
                }
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
            recs.push([(ts as i64) / 60, mi as i64, tt, it, ot, ch, cw]);
        }
        if recs.is_empty() {
            continue;
        }

        match sessions.entry(sid) {
            Entry::Vacant(v) => {
                v.insert((w, title, recs));
            }
            Entry::Occupied(mut o) => {
                o.get_mut().2.extend(recs);
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
    for (sid, (w, title, recs)) in &sessions {
        if recs.is_empty() {
            continue;
        }
        let mut r = recs.clone();
        r.sort_by_key(|x| x[0]);
        let id: String = sid.chars().take(8).collect();
        sess_list.push(json!({
            "w": w,
            "id": id,
            "t": title.clone().unwrap_or_else(|| "(无标题会话)".to_string()),
            "st": r[0][0],
            "r": r,
        }));
    }
    sess_list.sort_by_key(|s| s["st"].as_i64().unwrap_or(0));

    let bj = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    let now = chrono::Utc::now().with_timezone(&bj);

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
