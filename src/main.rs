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
use rusqlite::{Connection, Result as SqlResult};
use std::sync::Arc;
use clap::{Parser, Subcommand};
use colored::{Colorize};
use std::net::SocketAddr;
use std::io::{self, Write};

const DATABASE_FILE: &str = "uid_phone_map.db";

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
) -> impl IntoResponse {
    let conn = match state.get_db_connection() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("DB Connection Error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(LookupResponse {
                    status: "DB_ERROR".to_string(),
                    uid: None,
                    phone_number: None,
                }),
            )
        }
    };

    match lookup_data(&conn, &id) {
        Ok(response) => {
            let status = match response.status.as_str() {
                "not_found" => StatusCode::NOT_FOUND,
                _ => StatusCode::OK,
            };
            (status, Json(response))
        }
        Err(e) => {
            eprintln!("Lookup Error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(LookupResponse {
                    status: "QUERY_ERROR".to_string(),
                    uid: None,
                    phone_number: None,
                }),
            )
        }
    }
}

async fn api_status(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let conn = match state.get_db_connection() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("DB Connection Error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(StatusResponse {
                    status: "DB_ERROR".to_string(),
                    id: id,
                    exists: false,
                }),
            );
        }
    };

    let result = conn.query_row(
        "SELECT COUNT(*) FROM user_mapping WHERE uid = ?1 OR phone_number = ?1",
        [id.clone()],
        |row| row.get::<_, i64>(0),
    );

    let exists = match result {
        Ok(count) => count > 0,
        Err(_) => false,
    };

    let response = StatusResponse {
        status: if exists { "found" } else { "not_found" }.to_string(),
        id: id,
        exists: exists,
    };
    
    (StatusCode::OK, Json(response))
}

// --- CLI 命令行接口 ---

#[derive(Parser)]
#[command(author, version, about = "Cyber Lookup Service CLI & API Server")]
struct Cli {
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
async fn main() {
    let state = Arc::new(AppState::new(DATABASE_FILE.to_string()));
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { bind } => {
            println!("{}", format!("Starting API server on http://{}", bind).green().bold());
            
            let app = Router::new()
                .route("/lookup/:id", get(api_lookup))
                .route("/status/:id", get(api_status))
                .with_state(state);

            let addr: SocketAddr = bind.parse().expect("Invalid bind address");
            
            let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        }
        Commands::Termux => {
            run_cli(state);
        }
    }
}