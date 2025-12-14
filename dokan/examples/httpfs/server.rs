use std::{
	fs::{self, File, OpenOptions},
	io::{Read, Seek, SeekFrom, Write},
	path::{Path, PathBuf},
	sync::Arc,
};

use axum::{
	body::Bytes,
	extract::{Path as AxumPath, Query, State},
	http::StatusCode,
	response::{IntoResponse, Response},
	routing::{delete, get, post, put},
	Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

#[derive(Clone)]
struct ServerState {
	root_path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct FileInfo {
	name: String,
	is_directory: bool,
	size: u64,
	created: u64,
	modified: u64,
	accessed: u64,
}

#[derive(Debug, Deserialize)]
struct ReadQuery {
	offset: Option<u64>,
	length: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct WriteQuery {
	offset: Option<u64>,
	append: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct CreateQuery {
	is_directory: Option<bool>,
}

impl ServerState {
	fn get_real_path(&self, path: &str) -> PathBuf {
		let normalized = path.trim_start_matches('/');
		// 处理根目录：如果是 "$ROOT", "." 或空字符串，返回 root_path
		if normalized.is_empty() || normalized == "." || normalized == "$ROOT" {
			self.root_path.clone()
		} else {
			self.root_path.join(normalized)
		}
	}

	fn path_to_file_info(&self, path: &Path) -> Result<FileInfo, std::io::Error> {
		let metadata = fs::metadata(path)?;
		let name = path
			.file_name()
			.and_then(|n| n.to_str())
			.map(|s| s.to_string())
			.unwrap_or_else(|| {
				// 根目录使用 "." 作为名称
				".".to_string()
			});

		Ok(FileInfo {
			name,
			is_directory: metadata.is_dir(),
			size: metadata.len(),
			created: metadata
				.created()
				.ok()
				.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
				.map(|d| d.as_secs())
				.unwrap_or(0),
			modified: metadata
				.modified()
				.ok()
				.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
				.map(|d| d.as_secs())
				.unwrap_or(0),
			accessed: metadata
				.accessed()
				.ok()
				.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
				.map(|d| d.as_secs())
				.unwrap_or(0),
		})
	}
}

// GET /info/:path - 获取文件/目录信息
async fn get_info(
	State(state): State<Arc<ServerState>>,
	AxumPath(path): AxumPath<String>,
) -> Response {
	eprintln!("[SERVER] get_info: path='{}'", path);
	let real_path = state.get_real_path(&path);
	eprintln!("[SERVER] get_info: real_path={:?}", real_path);
	
	match state.path_to_file_info(&real_path) {
		Ok(info) => {
			eprintln!("[SERVER] get_info: success, is_directory={}", info.is_directory);
			Json(info).into_response()
		}
		Err(e) => {
			eprintln!("[SERVER] get_info: failed: {:?}", e);
			StatusCode::NOT_FOUND.into_response()
		}
	}
}

// GET /list/:path - 列出目录内容
async fn list_directory(
	State(state): State<Arc<ServerState>>,
	AxumPath(path): AxumPath<String>,
) -> Response {
	eprintln!("[SERVER] list_directory: path='{}', ", path);
	let real_path = state.get_real_path(&path);
	eprintln!("[SERVER] list_directory: real_path={:?}", real_path);
	
	if !real_path.exists() {
		eprintln!("[SERVER] list_directory: path does not exist");
		return StatusCode::NOT_FOUND.into_response();
	}
	
	if !real_path.is_dir() {
		eprintln!("[SERVER] list_directory: path is not a directory");
		return StatusCode::BAD_REQUEST.into_response();
	}

	match fs::read_dir(&real_path) {
		Ok(entries) => {
			let mut items = Vec::new();
			for entry in entries {
				if let Ok(entry) = entry {
					if let Ok(info) = state.path_to_file_info(&entry.path()) {
						items.push(info);
					}
				}
			}
			eprintln!("[SERVER] list_directory: returning {} items", items.len());
			Json(items).into_response()
		}
		Err(e) => {
			eprintln!("[SERVER] list_directory: read_dir failed: {:?}", e);
			StatusCode::INTERNAL_SERVER_ERROR.into_response()
		}
	}
}

// GET /read/:path - 读取文件内容
async fn read_file(
	State(state): State<Arc<ServerState>>,
	AxumPath(path): AxumPath<String>,
	Query(query): Query<ReadQuery>,
) -> Response {
	let real_path = state.get_real_path(&path);
	match File::open(&real_path) {
		Ok(mut file) => {
			let offset = query.offset.unwrap_or(0);
			let length = query.length.unwrap_or(usize::MAX);

			if offset > 0 {
				if file.seek(SeekFrom::Start(offset)).is_err() {
					return StatusCode::INTERNAL_SERVER_ERROR.into_response();
				}
			}

			let mut buffer = vec![0u8; length];
			match file.read(&mut buffer) {
				Ok(n) => {
					buffer.truncate(n);
					Bytes::from(buffer).into_response()
				}
				Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
			}
		}
		Err(_) => StatusCode::NOT_FOUND.into_response(),
	}
}

// POST /write/:path - 写入文件内容
async fn write_file(
	State(state): State<Arc<ServerState>>,
	AxumPath(path): AxumPath<String>,
	Query(query): Query<WriteQuery>,
	body: Bytes,
) -> Response {
	let real_path = state.get_real_path(&path);

	let mut opts = OpenOptions::new();
	opts.write(true);

	if query.append.unwrap_or(false) {
		opts.append(true);
	} else {
		opts.create(true);
	}

	match opts.open(&real_path) {
		Ok(mut file) => {
			let offset = query.offset.unwrap_or(0);
			if offset > 0 && !query.append.unwrap_or(false) {
				if file.seek(SeekFrom::Start(offset)).is_err() {
					return StatusCode::INTERNAL_SERVER_ERROR.into_response();
				}
			}

			match file.write_all(&body) {
				Ok(_) => StatusCode::OK.into_response(),
				Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
			}
		}
		Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
	}
}

// PUT /create/:path - 创建文件或目录
async fn create_file(
	State(state): State<Arc<ServerState>>,
	AxumPath(path): AxumPath<String>,
	Query(query): Query<CreateQuery>,
) -> Response {
	let real_path = state.get_real_path(&path);

	if real_path.exists() {
		return StatusCode::CONFLICT.into_response();
	}

	if query.is_directory.unwrap_or(false) {
		match fs::create_dir_all(&real_path) {
			Ok(_) => StatusCode::CREATED.into_response(),
			Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
		}
	} else {
		// Create parent directories if needed
		if let Some(parent) = real_path.parent() {
			let _ = fs::create_dir_all(parent);
		}

		match File::create(&real_path) {
			Ok(_) => StatusCode::CREATED.into_response(),
			Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
		}
	}
}

// DELETE /delete/:path - 删除文件或目录
async fn delete_path(
	State(state): State<Arc<ServerState>>,
	AxumPath(path): AxumPath<String>,
) -> Response {
	let real_path = state.get_real_path(&path);

	if !real_path.exists() {
		return StatusCode::NOT_FOUND.into_response();
	}

	let result = if real_path.is_dir() {
		fs::remove_dir_all(&real_path)
	} else {
		fs::remove_file(&real_path)
	};

	match result {
		Ok(_) => StatusCode::OK.into_response(),
		Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
	}
}

// POST /move/:path - 移动/重命名文件或目录
#[derive(Debug, Deserialize)]
struct MoveRequest {
	new_path: String,
}

async fn move_path(
	State(state): State<Arc<ServerState>>,
	AxumPath(path): AxumPath<String>,
	Json(req): Json<MoveRequest>,
) -> Response {
	let old_path = state.get_real_path(&path);
	let new_path = state.get_real_path(&req.new_path);

	if !old_path.exists() {
		return StatusCode::NOT_FOUND.into_response();
	}

	match fs::rename(&old_path, &new_path) {
		Ok(_) => StatusCode::OK.into_response(),
		Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
	}
}

// POST /truncate/:path - 设置文件大小
#[derive(Debug, Deserialize)]
struct TruncateRequest {
	size: u64,
}

async fn truncate_file(
	State(state): State<Arc<ServerState>>,
	AxumPath(path): AxumPath<String>,
	Json(req): Json<TruncateRequest>,
) -> Response {
	let real_path = state.get_real_path(&path);

	match File::open(&real_path) {
		Ok(file) => match file.set_len(req.size) {
			Ok(_) => StatusCode::OK.into_response(),
			Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
		},
		Err(_) => StatusCode::NOT_FOUND.into_response(),
	}
}

pub async fn run_server(root_path: String, port: u16) -> Result<(), Box<dyn std::error::Error>> {
	let root_path_display = root_path.clone();
	let state = Arc::new(ServerState {
		root_path: PathBuf::from(root_path),
	});

	let app = Router::new()
		.route("/info/*path", get(get_info))
		.route("/list/*path", get(list_directory))
		.route("/read/*path", get(read_file))
		.route("/write/*path", post(write_file))
		.route("/create/*path", put(create_file))
		.route("/delete/*path", delete(delete_path))
		.route("/move/*path", post(move_path))
		.route("/truncate/*path", post(truncate_file))
		.with_state(state);

	let addr = format!("127.0.0.1:{}", port);
	println!("HTTP Storage Server listening on {}", addr);
	println!("Serving files from: {}", root_path_display);

	let listener = TcpListener::bind(&addr).await?;
	axum::serve(listener, app).await?;

	Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let args: Vec<String> = std::env::args().collect();

	let root_path = args.get(1).cloned().unwrap_or_else(|| ".".to_string());
	let port = args
		.get(2)
		.and_then(|s| s.parse().ok())
		.unwrap_or(8080);

	run_server(root_path, port).await
}

