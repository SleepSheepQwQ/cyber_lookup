// src/main.rs (面向前端性能优化版)
// --- 模块和依赖导入 (新增 HashMap) ---
use axum::{
    routing::{get, post},
    extract::{Path, State, Json},
    response::{IntoResponse},
    http::StatusCode,
    Router,
};
use serde::{Serialize, Deserialize};
use rusqlite::{Connection, Result as SqlResult, Error as SqlError, types::ToSql};
use std::sync::{Arc, Mutex};
use clap::{Parser, Subcommand};
use colored::{Colorize};
use std::net::SocketAddr;
use std::io::{self, Write};
use std::path::Path as FilePath; 
use std::fs; 
use tokio::task;
use std::collections::HashMap; // <--- NEW: 引入 HashMap 用于高性能批量结果映射

// 默认配置 (仅用于创建 config.txt 时写入的默认值)
const DEFAULT_DATA_DIR: &str = "data";
const DEFAULT_CONFIG_FILE: &str = "config.txt";

// --- 错误处理 (保持不变) ---

#[derive(Debug)]
enum AppError {
    DbError(SqlError),
    BlockingTaskError,
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        eprintln!("{} Application Error: {:?}", "FATAL ERROR".red().bold(), self);
        let (status, body) = match self {
            AppError::DbError(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            ),
            AppError::BlockingTaskError => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            ),
        };
        (status, body).into_response()
    }
}

impl From<SqlError> for AppError {
    fn from(err: SqlError) -> Self {
        AppError::DbError(err)
    }
}

// 运行时配置结构体
#[derive(Clone)] 
struct ServerConfig {
    db_path: String,
    bind_address: String,
}

// --- 数据库状态管理 (保持不变) ---

struct AppState {
    db_path: Mutex<String>,
    initial_config: ServerConfig, 
}

impl AppState {
    fn new(db_path: String, initial_config: ServerConfig) -> Self {
        AppState { 
            db_path: Mutex::new(db_path),
            initial_config,
        }
    }

    fn get_db_connection(&self) -> SqlResult<Connection> {
        let path = self.db_path.lock().unwrap();
        Connection::open(&*path)
    }
    
    fn set_db_path(&self, new_path: String) {
        let mut path = self.db_path.lock().unwrap();
        *path = new_path;
    }
    
    fn current_db_path(&self) -> String {
        self.db_path.lock().unwrap().clone()
    }
}

// --- API 响应/请求结构体 (保持不变) ---

// 单个查询结果 (复用于批量处理)
#[derive(Debug, Serialize, Clone)]
struct LookupResponse {
    status: String,
    uid: Option<String>,
    phone_number: Option<String>,
}

#[derive(Serialize)]
struct StatusResponse {
    status: String,
    id: String,
    exists: bool,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    db_status: String,
    message: String,
}

#[derive(Serialize)]
struct InfoResponse {
    service_name: String,
    version: String,
    current_db_path: String,
    bind_address: String,
    initial_config_db_path: String,
}

// 批量请求体
#[derive(Debug, Deserialize)] 
struct BatchRequest {
    ids: Vec<String>, 
}

// 批量响应体
#[derive(Serialize)]
struct BatchResponse {
    results: Vec<LookupResponse>,
}

// --- 数据库查询核心逻辑 (保持不变, 仅供单次查询 API 和 CLI 使用) ---

fn lookup_data(conn: &Connection, id: &str) -> SqlResult<LookupResponse> {
    // 尝试按 UID 查找
    let mut stmt = conn.prepare("SELECT phone_number FROM user_mapping WHERE uid = ?1")?;
    if let Ok(phone) = stmt.query_row([id], |row| row.get(0)) {
        return Ok(LookupResponse {
            status: "found_by_uid".to_string(),
            uid: Some(id.to_string()),
            phone_number: Some(phone),
        });
    }

    // 尝试按 Phone 查找
    let mut stmt = conn.prepare("SELECT uid FROM user_mapping WHERE phone_number = ?1")?;
    if let Ok(uid) = stmt.query_row([id], |row| row.get(0)) {
        return Ok(LookupResponse {
            status: "found_by_phone".to_string(),
            uid: Some(uid),
            phone_number: Some(id.to_string()),
        });
    }

    // 未找到
    Ok(LookupResponse {
        status: "not_found".to_string(),
        uid: None,
        phone_number: None,
    })
}

// --- 数据库索引检查 (保持不变) ---
fn check_db_indices(conn: &Connection) -> bool {
    // ... (函数体不变) ...
    let check_column_indexed = |conn: &Connection, column_name: &str| -> bool {
        let mut stmt_index_list = match conn.prepare("PRAGMA index_list(user_mapping)") {
            Ok(s) => s,
            Err(_) => return false,
        };
        
        let mut rows_index_list = match stmt_index_list.query([]) {
            Ok(r) => r,
            Err(_) => return false,
        };
        
        while let Ok(Some(row_index)) = rows_index_list.next() {
            if let Ok(index_name) = row_index.get::<_, String>(1) {
                let mut col_stmt = match conn.prepare(&format!("PRAGMA index_info({})", index_name)) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                
                let mut col_rows = match col_stmt.query([]) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                
                while let Ok(Some(col_row)) = col_rows.next() {
                    if let Ok(col_name) = col_row.get::<_, String>(2) { 
                        if col_name == column_name {
                            return true;
                        }
                    }
                }
            }
        }
        false
    };

    let uid_indexed = check_column_indexed(conn, "uid");
    let phone_indexed = check_column_indexed(conn, "phone_number");
    
    if uid_indexed && phone_indexed {
        println!("{} Database indices confirmed (uid, phone_number). Query performance OK.", "INDEX OK".green().bold());
        true
    } else {
        println!("{} WARNING: Missing critical indices!", "INDEX WARNING".yellow().bold());
        println!("{}", "Hint: Create indices on `uid` and `phone_number` columns for performance.".yellow());
        false
    }
}


// --- API 路由处理函数 ---

// api_lookup (保持不变)
async fn api_lookup(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    // ... (函数体不变) ...
    let id_clone = id.clone(); 
    
    let conn_result = task::spawn_blocking(move || {
        state.get_db_connection().and_then(|conn| lookup_data(&conn, &id_clone))
    })
    .await;

    let response = match conn_result {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => {
            eprintln!("{} DB Query Error for ID {}: {:?}", "API ERR".red().bold(), id, e);
            return Err(AppError::DbError(e)); 
        },
        Err(_) => {
            return Err(AppError::BlockingTaskError); 
        }
    };

    let status = match response.status.as_str() {
        "not_found" => {
            println!("{} Lookup ID: {} -> NOT FOUND (404)", "API REQ".red(), id);
            StatusCode::NOT_FOUND 
        },
        _ => {
            println!("{} Lookup ID: {} -> FOUND ({})", "API REQ".green(), id, response.status.green());
            StatusCode::OK
        },
    };
    
    Ok((status, Json(response)))
}

// api_status (保持不变)
async fn api_status(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    // ... (函数体不变) ...
    let id_clone = id.clone();

    let conn_result = task::spawn_blocking(move || {
        let conn = state.get_db_connection()?;
        
        let result = conn.query_row(
            "SELECT COUNT(*) FROM user_mapping WHERE uid = ?1 OR phone_number = ?1",
            [id_clone.clone()],
            |row| row.get::<_, i64>(0),
        );
        
        let exists = match result {
            Ok(count) => count > 0,
            Err(SqlError::QueryReturnedNoRows) => false,
            Err(e) => return Err(e), 
        };
        
        Ok((exists, id_clone))
    })
    .await;

    let (exists, id) = match conn_result {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => {
            eprintln!("{} DB Query Error for ID {}: {:?}", "API ERR".red().bold(), id, e);
            return Err(AppError::DbError(e)); 
        },
        Err(_) => {
            return Err(AppError::BlockingTaskError); 
        }
    };
    
    let status = if exists {
        println!("{} Status ID: {} -> Exists: {}", "API REQ".cyan(), &id, exists.to_string().cyan());
        StatusCode::OK
    } else {
        println!("{} Status ID: {} -> NOT FOUND (404)", "API REQ".red(), &id);
        StatusCode::NOT_FOUND
    };

    let status_response = StatusResponse {
        status: if exists { "found" } else { "not_found" }.to_string(),
        id: id,
        exists: exists,
    };
    
    Ok((status, Json(status_response)))
}

// **优化后的批量查询接口 (性能核心增强)**
async fn api_batch_lookup(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BatchRequest>,
) -> Result<impl IntoResponse, AppError> {
    
    let ids = payload.ids;
    let count = ids.len();
    println!("{} Batch lookup request received. Count: {}", "API REQ".magenta().bold(), count);

    // 检查请求体是否为空
    if count == 0 {
        return Ok((StatusCode::BAD_REQUEST, Json(BatchResponse { results: vec![] })));
    }

    // 在阻塞线程中处理数据库操作
    let results = task::spawn_blocking(move || {
        let conn = state.get_db_connection()?;
        
        // --- 性能优化核心逻辑开始 ---

        // 1. 动态生成占位符字符串: "?, ?, ..."
        let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<&str>>().join(", ");
        
        // 2. 构造高性能的批量查询 SQL
        // 查询所有 uid 或 phone_number 匹配输入 ID 的记录
        let sql = format!(
            "SELECT uid, phone_number FROM user_mapping WHERE uid IN ({}) OR phone_number IN ({})",
            placeholders, placeholders
        );
        
        // 3. 准备参数列表 (IDs 需要重复两次用于 IN 语句的两个部分)
        let mut bound_params: Vec<&dyn ToSql> = Vec::with_capacity(count * 2);
        
        // 将 ids 转换为 &dyn ToSql 引用
        let ids_refs: Vec<String> = ids.into_iter().collect();
        // 第一次添加 (用于 uid IN (...))
        bound_params.extend(ids_refs.iter().map(|s| s as &dyn ToSql));
        // 第二次添加 (用于 phone_number IN (...))
        bound_params.extend(ids_refs.iter().map(|s| s as &dyn ToSql));
        
        // 4. 执行查询并处理结果
        let mut stmt = conn.prepare(&sql)?;
        let result_rows = stmt.query_map(&*bound_params, |row| {
            // 返回 (uid, phone_number) 组合
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)) 
        })?;

        // 5. 将查询结果映射到 HashMap 中，以便快速查找
        // 键是输入 ID，值是 LookupResponse
        let mut matched_results: HashMap<String, LookupResponse> = HashMap::new();
        for row in result_rows {
            let (uid, phone_number) = row?;
            
            // 结果行匹配了原始的 UID (即 uid 是请求的 ID 之一)
            if ids_refs.contains(&uid) {
                 matched_results.insert(
                    uid.clone(),
                    LookupResponse {
                        status: "found_by_uid".to_string(),
                        uid: Some(uid.clone()),
                        phone_number: Some(phone_number.clone()),
                    },
                );
            }
            
            // 结果行匹配了原始的 Phone (即 phone_number 是请求的 ID 之一)
            if ids_refs.contains(&phone_number) {
                matched_results.insert(
                    phone_number.clone(),
                    LookupResponse {
                        status: "found_by_phone".to_string(),
                        uid: Some(uid.clone()),
                        phone_number: Some(phone_number.clone()),
                    },
                );
            }
        }
        
        // 6. 遍历原始 ID 列表，构建最终响应
        let final_output: Vec<LookupResponse> = ids_refs.into_iter().map(|id| {
            matched_results.get(&id).cloned().unwrap_or_else(|| LookupResponse {
                status: "not_found".to_string(),
                uid: None,
                phone_number: None,
            })
        }).collect();

        // --- 性能优化核心逻辑结束 ---
        
        Ok(final_output)
    })
    .await;

    let final_results = match results {
        Ok(Ok(res)) => res,
        Ok(Err(e)) => {
            eprintln!("{} DB Query Error during batch: {:?}", "API ERR".red().bold(), e);
            return Err(AppError::DbError(e)); 
        },
        Err(_) => {
            return Err(AppError::BlockingTaskError); 
        }
    };
    
    println!("{} Batch lookup completed. {} results returned.", "API REQ".green().bold(), final_results.len());
    
    Ok((StatusCode::OK, Json(BatchResponse { results: final_results })))
}


// api_health (保持不变)
async fn api_health(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    // ... (函数体不变) ...
    let (db_status, message, http_status) = match state.get_db_connection() {
        Ok(conn) => {
            match conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0)) {
                Ok(_) => ("ok".to_string(), "Service and database connection are healthy.".to_string(), StatusCode::OK),
                Err(e) => ("error".to_string(), format!("Service is running, but database query failed: {}", e), StatusCode::SERVICE_UNAVAILABLE),
            }
        },
        Err(e) => ("error".to_string(), format!("Service is running, but cannot connect to database: {}", e), StatusCode::SERVICE_UNAVAILABLE),
    };
    
    println!("{} Health check: DB Status = {}", "API REQ".yellow().bold(), db_status.cyan());

    let response = HealthResponse {
        status: "ok".to_string(), 
        db_status,
        message,
    };

    Ok((http_status, Json(response)))
}

// api_info (保持不变)
async fn api_info(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    // ... (函数体不变) ...
    let info_response = InfoResponse {
        service_name: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        current_db_path: state.current_db_path(),
        bind_address: state.initial_config.bind_address.clone(),
        initial_config_db_path: state.initial_config.db_path.clone(),
    };
    
    println!("{} Info request served.", "API REQ".blue().bold());
    
    Ok((StatusCode::OK, Json(info_response)))
}


// --- CLI 命令行接口 (保持不变) ---
// ... (Cli, Commands, run_manage_shell, handle_cli_lookup, handle_cli_status, prompt_for_input, load_config 保持不变) ...

#[derive(Parser)]
#[command(author, version, about = "Cyber Lookup Service CLI & API Server")]
struct Cli {
    /// 覆盖配置文件中的数据库路径
    #[arg(short, long)] 
    db_path: Option<String>,
    
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 启动 API 服务器
    Serve {
        /// 覆盖配置文件中的绑定地址
        #[arg(short, long)]
        bind: Option<String>,
    },
    /// 启动管理终端，进行配置修改和查询 (中途反悔/更改的入口)
    Manage, 
}

fn run_manage_shell(state: Arc<AppState>) {
    println!("{}", "--- CYBER LOOKUP MANAGEMENT SHELL V0.1 ---".green().bold());
    println!("{}", "输入 'help' 获取命令列表".cyan());
    
    loop {
        let current_db = state.current_db_path();
        print!("{}", format!("[MANAGE:{}]> ", FilePath::new(&current_db).file_name().unwrap_or_default().to_string_lossy()).yellow()); 
        io::stdout().flush().unwrap();
        
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            continue;
        }

        let parts: Vec<&str> = input.trim().split_whitespace().collect();
        if parts.is_empty() { continue; }

        match parts[0].to_lowercase().as_str() {
            "exit" => {
                println!("{}", "退出管理终端...".red());
                break;
            }
            "lookup" if parts.len() == 2 => {
                handle_cli_lookup(&state, parts[1]);
            }
            "status" if parts.len() == 2 => {
                handle_cli_status(&state, parts[1]);
            }
            "db-switch" if parts.len() == 2 => {
                let new_path = parts[1].to_string();
                let new_file_path = FilePath::new(&new_path);
                
                if new_file_path.exists() {
                    match Connection::open(&new_path) {
                        Ok(conn) => {
                            if conn.query_row("SELECT name FROM sqlite_master WHERE type='table' AND name='user_mapping'", [], |_| Ok(1)).is_ok() {
                                state.set_db_path(new_path.clone());
                                println!("{} 成功将数据库切换到: {}", "SUCCESS".green().bold(), new_path.cyan());
                                println!("{} 温馨提示: API 服务器需要重启才能加载新的数据库路径。", "INFO".yellow());
                            } else {
                                println!("{} 错误: 文件 '{}' 看起来不是有效的查找数据库 (缺少 user_mapping 表)。", "ERROR".red().bold(), new_path);
                            }
                        },
                        Err(e) => {
                            println!("{} 错误: 无法打开文件 '{}'。错误: {}", "ERROR".red().bold(), new_path, e);
                        }
                    }
                } else {
                    state.set_db_path(new_path.clone());
                    println!("{} 成功将数据库切换到: {}", "SUCCESS".green().bold(), new_path.cyan());
                    println!("{} 警告: 文件 '{}' 不存在，它将在 API 启动时被创建。", "WARNING".yellow().bold(), new_path.yellow());
                }
            }
            "db-current" => {
                println!("{} 当前数据库路径: {}", "INFO".cyan().bold(), state.current_db_path().cyan());
                println!("{} 提示: 使用 'db-switch <新路径>' 进行更改。", "INFO".yellow());
            }
            "help" => {
                println!("{}", "--- 命令列表 ---".green().bold());
                println!("{}", "lookup <ID>       : 通过 ID/手机号查询数据并返回结果。".cyan());
                println!("{}", "status <ID>       : 检查 ID/手机号是否存在于数据库。".cyan());
                println!("{}", "db-switch <路径>  : 动态切换当前会话的数据库文件路径 (API 重启生效)。".cyan());
                println!("{}", "db-current        : 显示当前使用的数据库路径。".cyan());
                println!("{}", "exit              : 退出管理终端。".cyan());
            }
            _ => {
                println!("{}", "Error: 无效命令。输入 'help' 查看命令列表。".red());
            }
        }
    }
}

fn handle_cli_lookup(state: &Arc<AppState>, id: &str) {
    let conn = match state.get_db_connection() {
        Ok(c) => c,
        Err(e) => return println!("{} DB Connection Error: {}", "ERROR".red().bold(), e),
    };

    match lookup_data(&conn, id) {
        Ok(resp) => {
            match resp.status.as_str() {
                "found_by_uid" => {
                    println!("{} UID: {} -> PHONE: {}", "CLI LOOKUP".green().bold(), resp.uid.unwrap().cyan(), resp.phone_number.unwrap().cyan());
                }
                "found_by_phone" => {
                    println!("{} PHONE: {} -> UID: {}", "CLI LOOKUP".green().bold(), resp.phone_number.unwrap().cyan(), resp.uid.unwrap().cyan());
                }
                "not_found" => {
                    println!("{} ID: {} not found.", "CLI LOOKUP".red().bold(), id.red());
                }
                _ => (),
            }
        }
        Err(e) => println!("{} Query Error: {}", "ERROR".red().bold(), e),
    }
}

fn handle_cli_status(state: &Arc<AppState>, id: &str) {
    let conn = match state.get_db_connection() {
        Ok(c) => c,
        Err(e) => return println!("{} DB Connection Error: {}", "ERROR".red().bold(), e),
    };

    let result = conn.query_row(
        "SELECT COUNT(*) FROM user_mapping WHERE uid = ?1 OR phone_number = ?1",
        [id],
        |row| row.get::<_, i64>(0),
    );

    if let Ok(count) = result {
        if count > 0 {
            println!("{} ID: {} -> {}", "CLI STATUS".green().bold(), id.cyan(), "数据库里有他 (Found)".green());
        } else {
            println!("{} ID: {} -> {}", "CLI STATUS".red().bold(), id.red(), "数据库里没有他 (Not Found)".red());
        }
    } else {
         println!("{} Query Error for status check.", "ERROR".red().bold());
    }
}

fn prompt_for_input(prompt: &str, default_value: &str, is_valid: impl Fn(&str) -> bool) -> String {
    loop {
        print!("{}", format!("{} (Default: {}): ", prompt, default_value).yellow());
        io::stdout().flush().unwrap();
        
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            println!(); 
            return default_value.to_string();
        }

        let value = input.trim();

        if value.is_empty() {
            return default_value.to_string();
        }
        
        if is_valid(value) {
            return value.to_string();
        }
        
        println!("{}", "Error: Invalid format or path. Please try again.".red());
    }
}

fn load_config() -> ServerConfig {
    let default_config = ServerConfig {
        db_path: format!("{}/uid_phone_map.db", DEFAULT_DATA_DIR),
        bind_address: "127.0.0.1:3000".to_string(),
    };

    let config_path = FilePath::new(DEFAULT_DATA_DIR).join(DEFAULT_CONFIG_FILE);

    if config_path.exists() {
        println!("{} Found config file at: {}", "CONFIG".green().bold(), config_path.display().to_string().cyan());
        let content = fs::read_to_string(&config_path).unwrap_or_default();
        let mut loaded_config = default_config.clone(); 

        for line in content.lines() {
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                
                match key {
                    "db_path" => {
                        loaded_config.db_path = value.to_string();
                    },
                    "bind_address" => {
                        loaded_config.bind_address = value.to_string();
                    },
                    _ => {}
                }
            }
        }
        
        println!("{} Config parameters loaded.", "CONFIG".green().bold());
        return loaded_config;
        
    } else {
        println!("{} Config file not found.", "CONFIG".yellow().bold());
        let default_content = format!(
            "db_path = {}\nbind_address = {}\n",
            default_config.db_path,
            default_config.bind_address
        );

        let dir_path = FilePath::new(DEFAULT_DATA_DIR);
        if !dir_path.exists() {
             if let Err(e) = fs::create_dir_all(dir_path) {
                eprintln!("\n{} Failed to create data directory for config. Error: {}", "FATAL ERROR".red().bold(), e);
            }
        }

        if let Err(e) = fs::write(&config_path, default_content) {
            eprintln!("{} Failed to write default config file. Error: {}", "FATAL ERROR".red().bold(), e);
        } else {
            println!("{} Created default config file at: {}", "CONFIG".green().bold(), config_path.display().to_string().cyan());
            println!("{} Please modify {} to customize settings.", "CONFIG".yellow(), DEFAULT_CONFIG_FILE);
        }
        return default_config;
    }
}


// --- 主函数 (路由更新) ---

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    
    // 0. 初始化：创建 data/ 目录
    let dir_path = FilePath::new(DEFAULT_DATA_DIR);
    if !dir_path.exists() {
        println!("{} Creating data directory: {}", "INIT".yellow().bold(), DEFAULT_DATA_DIR);
        if let Err(e) = fs::create_dir_all(dir_path) {
            eprintln!("\n{} Failed to create data directory. Error: {}", "FATAL ERROR".red().bold(), e);
            eprintln!("{}", "Please check Termux file permissions (termux-setup-storage).".red());
            return Ok(());
        }
    } else {
         println!("{} Data directory found.", "INIT".green().bold());
    }

    // 1. 加载配置
    let initial_config = load_config();
    
    // 2. 解析 CLI 参数
    let cli = Cli::parse();
    
    // 3. 确定最终的 db_path
    let config_db_path = cli.db_path.as_ref().unwrap_or(&initial_config.db_path);
    let db_path_final = prompt_for_input(
        "Database file path", 
        config_db_path, 
        |p| !p.is_empty()
    );
    
    let state = Arc::new(AppState::new(db_path_final.clone(), initial_config.clone()));

    // 4. 检查数据库连接
    let db_file_path = FilePath::new(&db_path_final);
    if !db_file_path.exists() {
        println!("{}", format!("\n{} WARNING: Database file not found at '{}'.", "DB WARNING".yellow().bold(), db_path_final).yellow());
        println!("{}", "The program will create an empty file. Ensure your data is in place.".yellow());
    }
    
    let conn = match state.get_db_connection() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("\n{} FATAL: Cannot open/create database at '{}'. Error: {}", "DB FATAL".red().bold(), db_path_final, e);
            eprintln!("{}", "Check path, permissions, and database file integrity.".red());
            return Ok(());
        }
    };
    println!("{} Database connection established: {}", "DB CONNECT".green().bold(), db_file_path.display().to_string().cyan());

    // 5. 检查索引
    check_db_indices(&conn);
    drop(conn); 

    match cli.command {
        Commands::Serve { bind } => {
            
            // 6. 确定最终的 bind 地址
            let config_bind_addr = bind.as_ref().unwrap_or(&initial_config.bind_address);
            let bind_addr = prompt_for_input(
                "Bind Address (IP:PORT)", 
                config_bind_addr, 
                |a| a.parse::<SocketAddr>().is_ok()
            );

            let addr: SocketAddr = bind_addr.parse()?; 
            
            // 7. 绑定端口
            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => {
                    println!("{} Successfully bound to address: {}", "NETWORK OK".green().bold(), bind_addr.cyan());
                    l
                },
                Err(e) => {
                    eprintln!("{} Failed to bind to {}. Error: {}", "NETWORK FATAL".red().bold(), bind_addr, e);
                    eprintln!("{}", "Hint: Address might be in use or you lack permission (e.g., binding to a privileged port <1024).".red());
                    return Ok(());
                }
            };

            println!("{}", format!("\n--- API SERVER STARTED ---").green().bold());
            println!("{} API Access URL: {}", "SERVER INFO".green().bold(), format!("http://{}", bind_addr).cyan());
            println!("{} Available Endpoints: /lookup/:id, /status/:id, /health, /info, {}", "SERVER INFO".yellow(), "/batch_lookup (POST)".bold());
            println!("{} To stop the service, press {}", "SERVER INFO".yellow(), "Ctrl+C".bold());
            println!("--------------------------\n");

            let app = Router::new()
                .route("/lookup/:id", get(api_lookup))
                .route("/status/:id", get(api_status))
                .route("/health", get(api_health))
                .route("/info", get(api_info))
                .route("/batch_lookup", post(api_batch_lookup)) // 注册批量查询接口
                .with_state(state);

            axum::serve(listener, app).await?; 
        }
        Commands::Manage => {
            run_manage_shell(state);
        }
    }
    
    Ok(())
}
