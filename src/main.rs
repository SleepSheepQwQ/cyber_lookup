// src/main.rs (最终完备版：防御性编程、高交互性、无认证)
use axum::{
    routing::{get, post},
    extract::{Path, State, Json},
    response::IntoResponse,
    http::StatusCode, 
    Router,
};
use serde::{Serialize, Deserialize};
use rusqlite::{Connection, Result as SqlResult, Error as SqlError, types::ToSql};
use std::sync::{Arc, Mutex};
use colored::{Colorize};
use std::net::SocketAddr;
use std::io::{self, Write};
use std::path::Path as FilePath; 
use std::fs; 
use tokio::task;
use std::collections::HashMap; 
use std::process;
use std::time::Duration;
use tokio::time::sleep;

// --- 默认配置和常量 ---
const DEFAULT_DATA_DIR: &str = "data";
const DEFAULT_CONFIG_FILE: &str = "config.txt";
const DEFAULT_DB_PATH: &str = "data/uid_phone_map.db";
const DEFAULT_BIND_ADDRESS: &str = "0.0.0.0:3000";
const MAX_DATA_LENGTH: usize = 100; // 防御性：数据库字段最大长度

// --- 强化后的配置结构体 ---
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceConfig {
    db_path: String,
    bind_address: String,
    api_key: String,             // 保留字段，不用于认证
    log_level: String,           
    batch_size_limit: u32,       
}

impl Default for ServiceConfig {
    fn default() -> Self {
        ServiceConfig {
            db_path: DEFAULT_DB_PATH.to_string(),
            bind_address: DEFAULT_BIND_ADDRESS.to_string(),
            api_key: "".to_string(), 
            log_level: "info".to_string(),
            batch_size_limit: 1000,
        }
    }
}

impl ServiceConfig {
    /// 检查关键配置项的有效性
    fn validate(&self) -> Result<(), String> {
        if self.batch_size_limit == 0 {
            return Err("批次大小限制必须大于 0。".to_string());
        }
        
        match self.bind_address.parse::<SocketAddr>() {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("绑定地址格式无效 (应为 IP:端口): {}", e)),
        }
    }
}

// --- 错误处理 (保持不变) ---
#[derive(Debug)]
enum AppError {
    DbError(SqlError),
    IoError(io::Error),
    ConfigError(String),
    NetworkBindError(io::Error),
    FatalError(String),
    Unauthorized, 
}

impl From<SqlError> for AppError {
    fn from(err: SqlError) -> Self { AppError::DbError(err) }
}
impl From<io::Error> for AppError {
    fn from(err: io::Error) -> Self { 
        if err.kind() == io::ErrorKind::AddrInUse || err.kind() == io::ErrorKind::PermissionDenied {
            return AppError::NetworkBindError(err);
        }
        AppError::IoError(err) 
    }
}
impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self { AppError::ConfigError(format!("JSON Parse Error: {}", err)) }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match self {
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized access.".to_string()),
            AppError::DbError(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Database error: {}", e)),
            AppError::FatalError(m) => (StatusCode::BAD_REQUEST, m),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "An unknown error occurred.".to_string()),
        };
        (status, msg).into_response()
    }
}


// --- 内部日志辅助 (利用 log_level, 保持不变) ---
fn log_debug(config: &ServiceConfig, message: &str) {
    if config.log_level.to_lowercase() == "debug" {
        println!("{} {}", "DEBUG".blue(), message);
    }
}

// --- 防御性输入辅助函数 (新增/强化) ---

/// 读取一行输入并返回清理后的字符串，包括 I/O 错误处理。
fn read_line(prompt: &str) -> Result<String, io::Error> {
    print!("{}", prompt.green());
    io::stdout().flush()?;
    let mut input = String::new();
    // 防御性：处理 I/O 读取错误
    match io::stdin().read_line(&mut input) {
        Ok(_) => Ok(input.trim().to_string()),
        Err(e) => Err(e),
    }
}

/// 读取一个可选的字符串输入，如果用户输入为空，则返回 None。
fn read_optional_string(prompt: &str, current_value: &str) -> Result<Option<String>, io::Error> {
    let input = read_line(&format!("{} (当前: {}, 回车跳过): ", prompt, current_value))?;
    if input.is_empty() {
        Ok(None)
    } else {
        Ok(Some(input))
    }
}

/// 读取一个 U32 输入，并处理解析错误和边界条件（如不能为 0）。
fn read_u32(prompt: &str, current_value: u32) -> Result<Option<u32>, String> {
    // 使用 read_line 保证 I/O 错误已经被处理
    let input = match read_line(&format!("{} (当前: {}, 回车跳过): ", prompt, current_value)) {
        Ok(s) => s,
        Err(e) => return Err(format!("读取输入失败: {}", e)),
    };

    if input.is_empty() {
        Ok(None)
    } else {
        match input.parse::<u32>() {
            Ok(size) => {
                // 防御性：检查是否为 0
                if size == 0 {
                    Err("输入值必须大于 0。".to_string())
                } else {
                    Ok(Some(size))
                }
            },
            // 防御性：处理解析失败
            Err(_) => Err(format!("输入 '{}' 无效，请输入一个正整数。", input)),
        }
    }
}


// --- 应用状态结构体 / 配置管理 / 数据库初始化 (保持不变) ---
struct AppState {
    config: Mutex<ServiceConfig>, 
}
impl AppState {
    fn get_db_connection(&self) -> SqlResult<Connection> {
        let path = self.config.lock().unwrap().db_path.clone();
        Connection::open(&path)
    }
    fn current_config(&self) -> ServiceConfig {
        self.config.lock().unwrap().clone()
    }
    fn set_config(&self, new_config: ServiceConfig) {
        *self.config.lock().unwrap() = new_config;
    }
}
fn load_config() -> Result<ServiceConfig, AppError> {
    let path = FilePath::new(DEFAULT_CONFIG_FILE);
    let config = if !path.exists() {
        let default_config = ServiceConfig::default();
        save_config(&default_config)?;
        println!("{} Config file created at: {}", "INFO".yellow(), DEFAULT_CONFIG_FILE);
        default_config
    } else {
        let content = fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(AppError::from)?
    };

    // 防御性：加载后立即校验
    if let Err(e) = config.validate() {
        return Err(AppError::FatalError(format!("配置校验失败: {}", e)));
    }
    
    Ok(config)
}
fn save_config(config: &ServiceConfig) -> Result<(), AppError> {
    // 防御性：写入前再次校验
    if let Err(e) = config.validate() {
        return Err(AppError::FatalError(format!("配置校验失败，未保存: {}", e)));
    }

    let content = serde_json::to_string_pretty(config).map_err(AppError::from)?;
    fs::write(DEFAULT_CONFIG_FILE, content).map_err(AppError::from)
}
fn initialize_database(conn: &Connection) -> SqlResult<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS user_mapping (
            uid TEXT NOT NULL,
            phone_number TEXT NOT NULL,
            UNIQUE(uid, phone_number)
        )",
        (),
    )?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_uid ON user_mapping (uid)",
        (),
    )?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_phone ON user_mapping (phone_number)",
        (),
    )?;
    Ok(())
}


// --- API 响应/请求模型 / 核心业务逻辑 (保持不变) ---
#[derive(Debug, Serialize, Clone)]
struct LookupResponse {
    status: String, uid: Option<String>, phone_number: Option<String>,
}
#[derive(Debug, Deserialize)] 
struct BatchRequest {
    ids: Vec<String>, 
}
#[derive(Serialize)]
struct BatchResponse {
    results: Vec<LookupResponse>,
}
#[derive(Serialize)]
struct InfoResponse {
    version: String, db_path: String, bind_address: String,
}
#[derive(Serialize)]
struct HealthResponse {
    status: String, message: String,
}

fn lookup_one(conn: &Connection, id: &str) -> SqlResult<LookupResponse> {
    let mut stmt = conn.prepare("SELECT phone_number FROM user_mapping WHERE uid = ?1")?;
    if let Ok(phone) = stmt.query_row([id], |row| row.get(0)) {
        return Ok(LookupResponse { status: "found_by_uid".to_string(), uid: Some(id.to_string()), phone_number: Some(phone) });
    }
    let mut stmt = conn.prepare("SELECT uid FROM user_mapping WHERE phone_number = ?1")?;
    if let Ok(uid) = stmt.query_row([id], |row| row.get(0)) {
        return Ok(LookupResponse { status: "found_by_phone".to_string(), uid: Some(uid), phone_number: Some(id.to_string()) });
    }
    Ok(LookupResponse { status: "not_found".to_string(), uid: None, phone_number: None })
}

// --- API 路由处理器 (保持不变) ---
async fn api_lookup(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let result = task::spawn_blocking(move || {
        state.get_db_connection().and_then(|conn| lookup_one(&conn, &id))
    }).await.map_err(|_| AppError::FatalError("Blocking task failed".to_string()))?;

    match result {
        Ok(resp) => {
            let code = if resp.status == "not_found" { StatusCode::NOT_FOUND } else { StatusCode::OK };
            Ok((code, Json(resp)))
        },
        Err(e) => {
            eprintln!("{} DB Error in /lookup: {}", "ERR".red(), e);
            Err(AppError::DbError(e))
        }
    }
}

async fn api_batch_lookup(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BatchRequest>, 
) -> Result<impl IntoResponse, AppError> {
    
    let config = state.current_config();
    
    let ids = payload.ids;
    // 防御性：检查批次大小是否超限
    if ids.len() > config.batch_size_limit as usize {
        println!("{} Request batch size {} exceeds limit {}", "WARN".yellow(), ids.len(), config.batch_size_limit);
        return Err(AppError::FatalError(format!("Batch size {} exceeds limit {}", ids.len(), config.batch_size_limit)));
    }

    log_debug(&config, &format!("Batch Request received: {} items", ids.len()));

    let results = task::spawn_blocking(move || {
        let conn = state.get_db_connection()?;
        
        let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<&str>>().join(",");
        let sql = format!("SELECT uid, phone_number FROM user_mapping WHERE uid IN ({0}) OR phone_number IN ({0})", placeholders);
        let mut params: Vec<&dyn ToSql> = Vec::with_capacity(ids.len() * 2);
        for id in &ids { params.push(id); }
        for id in &ids { params.push(id); } 
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(&*params, |row| {Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))})?;
        
        let mut map = HashMap::new();
        for r in rows {
            if let Ok((u, p)) = r {
                let resp = LookupResponse { status: "found".to_string(), uid: Some(u.clone()), phone_number: Some(p.clone()) };
                map.insert(u.clone(), resp.clone());
                map.insert(p, resp);
            }
        }
        let final_res: Vec<LookupResponse> = ids.iter().map(|id| {
            map.get(id).cloned().unwrap_or(LookupResponse { status: "not_found".to_string(), uid: None, phone_number: None })
        }).collect();
        
        Ok::<_, SqlError>(final_res)
    }).await.map_err(|_| AppError::FatalError("Blocking task failed".to_string()))?;

    match results {
        Ok(data) => Ok(Json(BatchResponse { results: data })),
        Err(e) => {
            eprintln!("{} Batch DB Error: {}", "ERR".red(), e);
            Err(AppError::DbError(e))
        }
    }
}


// --- 交互式数据库管理 (高交互性 & 防御性增强) ---
fn run_db_management(state: Arc<AppState>) {
    println!("{}", "\n--- 交互式数据库管理模式 ---".magenta().bold());
    println!("{}", "命令: 'insert' (增), 'lookup' (查), 'delete' (删), 'count' (查总数), 'clear' (清空), 'back' (返回)".cyan());

    // 第一次连接尝试
    let initial_conn_check = state.get_db_connection();
    if initial_conn_check.is_err() {
        eprintln!("{} 无法连接数据库: {}", "DB ERR".red(), initial_conn_check.unwrap_err());
        return;
    }

    loop {
        match read_line(&format!("{} (DB) > ", "MANAGE".magenta())) {
            Ok(command) => {
                let command = command.to_lowercase();
                
                if command.is_empty() { continue; }
                if command == "back" || command == "exit" { break; }

                // 核心：在每次 DB 操作前都重新获取连接，并检查是否成功
                let conn = match state.get_db_connection() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("{} 数据库连接中断，退出管理模式: {}", "DB ERR".red(), e);
                        break;
                    }
                };

                match command.as_str() {
                    "insert" => {
                        let uid = match read_line("请输入 UID: ") {
                            Ok(s) if !s.is_empty() => s,
                            _ => { println!("{}", "UID不能为空。".red()); continue; },
                        };
                        let phone = match read_line("请输入 Phone Number: ") {
                            Ok(s) if !s.is_empty() => s,
                            _ => { println!("{}", "手机号不能为空。".red()); continue; },
                        };

                        // 防御性：检查数据长度
                        if uid.len() > MAX_DATA_LENGTH || phone.len() > MAX_DATA_LENGTH {
                            eprintln!("{} 输入数据过长，请保持在 {} 字符以内。", "DB ERR".red(), MAX_DATA_LENGTH);
                            continue;
                        }
                        
                        let result = conn.execute(
                            "INSERT OR REPLACE INTO user_mapping (uid, phone_number) VALUES (?1, ?2)",
                            [&uid, &phone],
                        );
                        
                        match result {
                            Ok(_) => println!("{} 插入/更新成功：UID={}, Phone={}", "OK".green(), uid, phone),
                            Err(e) => eprintln!("{} 插入失败: {}", "DB ERR".red(), e),
                        }
                    },
                    "lookup" => {
                        let id = match read_line("请输入要查找的 UID 或 Phone Number: ") {
                            Ok(s) if !s.is_empty() => s,
                            _ => continue,
                        };
                        
                        match lookup_one(&conn, &id) {
                            Ok(resp) => {
                                match resp.status.as_str() {
                                    "not_found" => println!("{} 未找到 ID: {}", "NOT FOUND".yellow(), id),
                                    _ => println!("{} 找到匹配: UID={}, Phone={}", "FOUND".green(), resp.uid.unwrap_or_default(), resp.phone_number.unwrap_or_default()),
                                }
                            },
                            Err(e) => eprintln!("{} 查找失败: {}", "DB ERR".red(), e),
                        }
                    },
                    "delete" => {
                        let id = match read_line("请输入要删除的 UID 或 Phone Number: ") {
                            Ok(s) if !s.is_empty() => s,
                            _ => continue,
                        };

                        // 防御性：确认删除
                        let confirm = match read_line(&format!("{} 警告: 确认删除 ID '{}'? (yes/no): ", "WARN".yellow(), id)) {
                            Ok(s) => s.to_lowercase(),
                            _ => continue,
                        };

                        if confirm == "yes" {
                            let result = conn.execute(
                                "DELETE FROM user_mapping WHERE uid = ?1 OR phone_number = ?1",
                                [&id],
                            );
                            
                            match result {
                                Ok(count) => println!("{} 成功删除 {} 条记录 (ID: {})", "OK".green(), count, id),
                                Err(e) => eprintln!("{} 删除失败: {}", "DB ERR".red(), e),
                            }
                        } else {
                            println!("{} 操作取消。", "INFO".cyan());
                        }
                    },
                    "count" => {
                        let count: SqlResult<i64> = conn.query_row("SELECT COUNT(*) FROM user_mapping", [], |row| row.get(0));
                        match count {
                            Ok(c) => println!("{} 总记录数: {}", "INFO".yellow(), c),
                            Err(e) => eprintln!("{} 查询失败: {}", "DB ERR".red(), e),
                        }
                    },
                    "clear" => {
                        // 防御性：确认清空
                        let confirm = match read_line(&format!("{} 警告：这将清空所有数据。确认清空? (yes/no): ", "WARN".red())) {
                            Ok(s) => s.to_lowercase(),
                            _ => continue,
                        };
                        
                        if confirm == "yes" {
                            match conn.execute("DELETE FROM user_mapping", []) {
                                Ok(count) => println!("{} 成功清空 {} 条记录。", "OK".green(), count),
                                Err(e) => eprintln!("{} 清空失败: {}", "DB ERR".red(), e),
                            }
                        } else {
                            println!("{} 操作取消。", "INFO".cyan());
                        }
                    },
                    _ => println!("{} 未知命令: {}", "WARN".yellow(), command),
                }
            }
            Err(e) => {
                eprintln!("{} I/O 读取失败，退出管理模式: {}", "FATAL".red(), e);
                break;
            }
        }
    }
    println!("{}", "返回主管理菜单...".magenta());
}


// --- 交互式配置编辑函数 (使用防御性辅助函数) ---
fn edit_config(state: Arc<AppState>) {
    println!("{}", "\n--- 正在编辑配置 ---".blue().bold());
    let config = state.current_config();
    let mut new_config = config.clone();
    
    // 1. 修改 DB 路径
    if let Ok(Some(path)) = read_optional_string("[1] DB路径", &new_config.db_path) {
        new_config.db_path = path;
    }
    
    // 2. 修改 绑定地址 (IP:端口)
    if let Ok(Some(addr)) = read_optional_string("[2] 绑定地址 (IP:端口)", &new_config.bind_address) {
        // 防御性：即时验证地址格式
        match addr.parse::<SocketAddr>() {
            Ok(_) => new_config.bind_address = addr,
            Err(e) => eprintln!("{} 地址格式无效 ('{}')，未修改: {}", "ERROR".red(), addr, e),
        }
    }
    
    // 3. 修改 批次大小限制
    match read_u32("[3] 批次大小限制", new_config.batch_size_limit) {
        Ok(Some(size)) => new_config.batch_size_limit = size,
        Err(e) => eprintln!("{} {}", "ERROR".red(), e),
        _ => {},
    }
    
    // 4. 修改 日志级别
    if let Ok(Some(level)) = read_optional_string("[4] 日志级别 (info/debug)", &new_config.log_level) {
        let level_lower = level.to_lowercase();
        if level_lower == "info" || level_lower == "debug" {
            new_config.log_level = level_lower;
        } else {
            eprintln!("{} 日志级别无效 ('{}')，保持不变。", "ERROR".red(), level);
        }
    }

    // 保存并验证新配置
    if let Err(e) = save_config(&new_config) {
        eprintln!("{} 配置保存失败: {:?}", "ERROR".red(), e);
    } else {
        state.set_config(new_config);
        println!("{}", "\n配置已更新并保存到 config.txt".green().bold());
    }
}


// --- 尝试启动服务器 / 主循环 / 主入口点 (保持与上个版本一致的逻辑流程) ---
async fn try_start_server(state: Arc<AppState>) -> Result<(), AppError> {
    let config = state.current_config();
    
    if let Err(e) = config.validate() {
        return Err(AppError::FatalError(format!("配置校验失败: {}", e)));
    }
    
    let bind_addr = config.bind_address.clone();
    let db_path = config.db_path.clone();

    println!("{} 正在尝试连接数据库: {}", "INFO".yellow(), db_path);
    let conn = state.get_db_connection().map_err(|e| {
        eprintln!("{} 数据库连接失败: {}", "FAIL".red(), e);
        eprintln!("{} 提示: 请确保 {} 路径下的数据库文件存在且可访问。", "HINT".yellow(), db_path);
        AppError::DbError(e)
    })?;

    println!("{} 正在检查/创建数据库表结构和索引...", "INFO".yellow());
    match initialize_database(&conn) {
        Ok(_) => println!("{} 数据库结构健全。", "OK".green()),
        Err(e) => {
            eprintln!("{} 数据库初始化失败: {}", "FAIL".red(), e);
            return Err(AppError::DbError(e));
        }
    }

    let addr: SocketAddr = bind_addr.parse()
        .map_err(|e| AppError::FatalError(format!("Config Error: Invalid bind address format: {}", e)))?;
    
    let listener = tokio::net::TcpListener::bind(addr).await
        .map_err(|e| AppError::NetworkBindError(e))?; 

    println!("{} 服务启动，监听地址: http://{}", "STARTED".green().bold(), addr);
    println!("{} Endpoints: /lookup/:id, /batch_lookup (POST)", "INFO".cyan());
    println!("{} 提示: 批量查询接口无需认证。", "HINT".yellow());
    println!("{} 按 Ctrl+C 停止服务并进入管理模式。", "HINT".yellow());

    let app = Router::new()
        .route("/lookup/:id", get(api_lookup))
        .route("/health", get(api_health))
        .route("/info", get(api_info))
        .route("/batch_lookup", post(api_batch_lookup))
        .with_state(state);

    axum::serve(listener, app).await
        .map_err(|e| AppError::IoError(e))?;
        
    Ok(())
}

async fn api_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.get_db_connection().and_then(|c| c.query_row("SELECT 1", [], |_| Ok(()))) {
        Ok(_) => (StatusCode::OK, Json(HealthResponse { status: "ok".to_string(), message: "Ready".to_string() })),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, Json(HealthResponse { status: "error".to_string(), message: e.to_string() })),
    }
}

async fn api_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config = state.current_config();
    Json(InfoResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        db_path: config.db_path,
        bind_address: config.bind_address,
    })
}

async fn interactive_manage_loop(state: Arc<AppState>) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n{}", "--- 欢迎进入交互式服务管理模式 ---".green().bold());
    println!("{}", "命令: 'start', 'config', 'db-manage', 'info', 'exit'".cyan());
    
    loop {
        let current_config = state.current_config();
        // 使用防御性读取
        let command = match read_line(&format!("{} ({}@{}) > ", "MANAGE".magenta(), current_config.log_level, current_config.bind_address)) {
            Ok(s) => s.to_lowercase(),
            Err(e) => {
                eprintln!("{} I/O 读取失败: {}", "FATAL".red(), e);
                break;
            }
        };

        match command.as_str() {
            "start" => {
                // ... (启动逻辑不变)
                println!("{}", "尝试启动服务...".yellow());
                match try_start_server(state.clone()).await {
                    Ok(_) => {
                        println!("{}", "服务已停止。".red());
                    }
                    Err(AppError::NetworkBindError(e)) => {
                        eprintln!("{} 启动失败 (端口冲突或权限不足): {}", "FAIL".red(), e);
                        eprintln!("{} 请使用 'config' 修改 bind_address。", "HINT".yellow());
                    }
                    Err(AppError::DbError(_)) => {} 
                    Err(AppError::FatalError(m)) => eprintln!("{} 启动失败 (致命配置错误): {}", "FAIL".red(), m),
                    Err(e) => {
                        eprintln!("{} 发生未知错误: {:?}", "FAIL".red(), e);
                    }
                }
            }
            "config" => {
                edit_config(state.clone());
            }
            "db-manage" => {
                run_db_management(state.clone());
            }
            "exit" => {
                println!("{}", "退出程序。".red());
                process::exit(0);
            }
            "info" => {
                println!("{}", format!("{:#?}", current_config).yellow());
            }
            _ => {
                if !command.is_empty() {
                    println!("{} 未知命令: {}", "WARN".yellow(), command);
                }
            }
        }
    }
    Ok(())
}


// --- 程序主入口点 ---
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(DEFAULT_DATA_DIR).ok();

    let initial_config = match load_config() {
        Ok(c) => c,
        Err(AppError::FatalError(m)) => {
            eprintln!("{} 致命错误: {}", "FATAL".red(), m);
            return Err("Configuration failed validation.".into());
        }
        Err(e) => {
            eprintln!("{} 致命错误: 配置加载失败: {:?}", "FATAL".red(), e);
            return Err("Configuration failed to load.".into());
        }
    };

    let state = Arc::new(AppState { config: Mutex::new(initial_config) });

    match try_start_server(state.clone()).await {
        Ok(_) => {
            println!("{}", "服务已停止，进入交互式管理模式...".yellow());
            interactive_manage_loop(state).await?;
        }
        Err(AppError::NetworkBindError(e)) => {
            eprintln!("{} 服务启动失败 (网络绑定错误): {}", "FAIL".red().bold(), e);
            eprintln!("{}", "自动进入交互式管理模式，您可以使用 'config' 命令修改地址。", "YELLOW".yellow());
            sleep(Duration::from_secs(1)).await;
            interactive_manage_loop(state).await?;
        }
        Err(AppError::DbError(_)) | Err(AppError::FatalError(_)) => {
            interactive_manage_loop(state).await?;
        }
        Err(e) => {
            eprintln!("{} 服务启动失败: {:?}", "FAIL".red().bold(), e);
            interactive_manage_loop(state).await?;
        }
    }

    Ok(())
}
