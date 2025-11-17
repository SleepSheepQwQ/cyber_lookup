// src/main.rs
// --- 模块和依赖导入 ---
use axum::{
    routing::get,
    extract::{Path, State},
    response::{IntoResponse, Json},
    http::StatusCode,
    Router,
};
use serde::{Serialize};
use rusqlite::{Connection, Result as SqlResult, Error as SqlError};
use std::sync::Arc;
use clap::{Parser, Subcommand};
use colored::{Colorize};
use std::net::SocketAddr;
use std::io::{self, Write};
use tokio::task;

// 默认数据库文件路径
const DEFAULT_DATABASE_FILE: &str = "uid_phone_map.db";

// --- 自定义错误类型 ---

#[derive(Debug)]
enum AppError {
    DbError(SqlError),
    BlockingTaskError,
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        eprintln!("Application Error: {:?}", self);
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

// --- 数据库状态管理 ---

struct AppState {
    db_path: String,
}

impl AppState {
    fn new(db_path: String) -> Self {
        AppState { db_path }
    }

    fn get_db_connection(&self) -> SqlResult<Connection> {
        Connection::open(&self.db_path)
    }
}

// --- API 响应结构体 ---

#[derive(Serialize)]
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

// --- 数据库操作函数 ---

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

// --- API 路由处理函数 ---

async fn api_lookup(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    
    let id_clone = id.clone(); 

    let response = task::spawn_blocking(move || {
        let conn = state.get_db_connection()?;
        lookup_data(&conn, &id_clone)
    })
    .await
    .map_err(|_| AppError::BlockingTaskError)? 
    .map_err(AppError::DbError)?;              

    let status = match response.status.as_str() {
        "not_found" => StatusCode::NOT_FOUND,
        _ => StatusCode::OK,
    };
    
    Ok((status, Json(response)))
}

async fn api_status(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {

    let id_clone = id.clone();

    let response = task::spawn_blocking(move || {
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
    .await
    .map_err(|_| AppError::BlockingTaskError)?
    .map_err(AppError::DbError)?;

    let (exists, id) = response;

    let status_response = StatusResponse {
        status: if exists { "found" } else { "not_found" }.to_string(),
        id: id,
        exists: exists,
    };
    
    Ok((StatusCode::OK, Json(status_response)))
}

// --- CLI 命令行接口 ---

#[derive(Parser)]
#[command(author, version, about = "Cyber Lookup Service CLI & API Server")]
struct Cli {
    #[arg(short, long, default_value = DEFAULT_DATABASE_FILE)]
    db_path: String,
    
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 启动 API 服务器
    Serve {
        #[arg(short, long, default_value = "127.0.0.1:5000")]
        bind: String,
    },
    /// 终端交互查询
    Termux,
}

// 终端交互函数
fn run_cli(state: Arc<AppState>) {
    println!("{}", "--- CYBER LOOKUP V0.1 ---".green().bold());
    println!("{}", "Enter 'lookup <ID>' or 'status <ID>' or 'exit'".cyan());
    println!("{}", format!("DB Path: {}", state.db_path).yellow());

    loop {
        print!("{}", "[CYBER-LOOKUP]$ ".yellow());
        io::stdout().flush().unwrap();
        
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            continue;
        }

        let parts: Vec<&str> = input.trim().split_whitespace().collect();
        if parts.is_empty() { continue; }

        match parts[0].to_lowercase().as_str() {
            "exit" => {
                println!("{}", "Shutting down...".red());
                break;
            }
            "lookup" if parts.len() == 2 => {
                handle_cli_lookup(&state, parts[1]);
            }
            "status" if parts.len() == 2 => {
                handle_cli_status(&state, parts[1]);
            }
            _ => {
                println!("{}", "Error: Invalid command. Use 'lookup <ID>' or 'status <ID>'.".red());
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
                    println!("{} UID: {} -> PHONE: {}", "FOUND BY UID".green().bold(), resp.uid.unwrap(), resp.phone_number.unwrap());
                }
                "found_by_phone" => {
                    println!("{} PHONE: {} -> UID: {}", "FOUND BY PHONE".green().bold(), resp.phone_number.unwrap(), resp.uid.unwrap());
                }
                "not_found" => {
                    println!("{} ID: {} not found.", "NOT FOUND".red().bold(), id);
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
            println!("{} ID: {} -> {}", "STATUS OK".green().bold(), id, "数据库里有他 (Found)".green());
        } else {
            println!("{} ID: {} -> {}", "STATUS FAIL".red().bold(), id, "数据库里没有他 (Not Found)".red());
        }
    } else {
         println!("{} Query Error for status check.", "ERROR".red().bold());
    }
}


// --- 主函数 ---

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    
    let db_path = cli.db_path;
    let state = Arc::new(AppState::new(db_path.clone()));

    match cli.command {
        Commands::Serve { bind } => {
            println!("{}", format!("Starting API server on http://{}", bind).green().bold());
            println!("{}", format!("Using Database: {}", db_path).cyan());
            
            let app = Router::new()
                .route("/lookup/:id", get(api_lookup))
                .route("/status/:id", get(api_status))
                .with_state(state);

            let addr: SocketAddr = bind.parse()?; 
            
            let listener = tokio::net::TcpListener::bind(addr).await?; 
            axum::serve(listener, app).await?; 
        }
        Commands::Termux => {
            run_cli(state);
        }
    }
    
    Ok(())
}
