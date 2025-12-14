use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Arg, ArgAction, Command};
use dokan::{
	init, shutdown, unmount, CreateFileInfo, DiskSpaceInfo, FileInfo, FileSystemHandler,
	FileSystemMounter, FileTimeOperation, FillDataError, FillDataResult, FindData,
	MountFlags, MountOptions, OperationInfo, OperationResult, VolumeInfo, IO_SECURITY_CONTEXT,
};
use dokan_sys::win32::{
	FILE_CREATE, FILE_DELETE_ON_CLOSE, FILE_DIRECTORY_FILE, FILE_MAXIMUM_DISPOSITION,
	FILE_OPEN, FILE_OPEN_IF, FILE_OVERWRITE, FILE_OVERWRITE_IF, FILE_SUPERSEDE,
};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use widestring::{U16CStr, U16CString};
use winapi::{shared::ntstatus::*, um::winnt};

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteFileInfo {
	name: String,
	is_directory: bool,
	size: u64,
	created: u64,
	modified: u64,
	accessed: u64,
}

struct FileContext {
	path: String,
	delete_on_close: bool,
}

impl FileContext {
	fn new(path: String, delete_on_close: bool) -> Self {
		Self {
			path,
			delete_on_close,
		}
	}
}

struct HttpFsHandler {
	base_url: String,
	client: Client,
}

impl HttpFsHandler {
	fn new(base_url: String) -> Self {
		Self {
			base_url,
			client: Client::builder()
				.timeout(Duration::from_secs(30))
				.build()
				.unwrap(),
		}
	}

	fn normalize_path(&self, file_name: &U16CStr) -> String {
		let path_str = file_name.to_string_lossy();
		let trimmed = path_str.trim_start_matches('\\').replace('\\', "/");
		if trimmed.is_empty() {
			".".to_string()
		} else {
			trimmed
		}
	}

	fn get_remote_file_info(&self, path: &str) -> Result<RemoteFileInfo, reqwest::Error> {
		// 根目录使用特殊标识符
		let api_path = if path == "." { "$ROOT" } else { path };
		let url = format!("{}/info/{}", self.base_url, api_path);
		let response = self.client.get(&url).send()?;
		
		if !response.status().is_success() {
			eprintln!("[ERROR] get_remote_file_info: server returned status {} for path '{}'", response.status(), path);
			return Err(response.error_for_status().unwrap_err());
		}
		
		response.json::<RemoteFileInfo>()
	}

	fn list_remote_directory(&self, path: &str) -> Result<Vec<RemoteFileInfo>, reqwest::Error> {
		// 根目录使用特殊标识符
		let api_path = if path == "." { "$ROOT" } else { path };
		let url = format!("{}/list/{}", self.base_url, api_path);
		let response = self.client.get(&url).send()?;
		
		if !response.status().is_success() {
			eprintln!("[ERROR] list_remote_directory: server returned status {}", response.status());
			return Err(response.error_for_status().unwrap_err());
		}
		
		response.json::<Vec<RemoteFileInfo>>()
	}

	fn read_file_data(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>, reqwest::Error> {
		// 根目录使用特殊标识符（虽然不应该读取目录，但为了一致性）
		let api_path = if path == "." { "$ROOT" } else { path };
		let url = format!("{}/read/{}", self.base_url, api_path);
		let response = self
			.client
			.get(&url)
			.query(&[("offset", offset.to_string()), ("length", length.to_string())])
			.send()?;
			
		if !response.status().is_success() {
			eprintln!("[ERROR] read_file_data: server returned status {} for path '{}'", response.status(), path);
			return Err(response.error_for_status().unwrap_err());
		}
		
		let data = response.bytes()?.to_vec();
		Ok(data)
	}

	fn write_file_data(&self, path: &str, offset: u64, data: &[u8]) -> Result<(), reqwest::Error> {
		// 根目录使用特殊标识符（虽然不应该写入目录，但为了一致性）
		let api_path = if path == "." { "$ROOT" } else { path };
		let url = format!("{}/write/{}", self.base_url, api_path);
		self.client
			.post(&url)
			.query(&[("offset", offset.to_string())])
			.body(data.to_vec())
			.send()?;
		Ok(())
	}

	fn create_remote(&self, path: &str, is_directory: bool) -> Result<(), reqwest::Error> {
		// 根目录使用特殊标识符（虽然不应该创建根目录，但为了一致性）
		let api_path = if path == "." { "$ROOT" } else { path };
		let url = format!("{}/create/{}", self.base_url, api_path);
		self.client
			.put(&url)
			.query(&[("is_directory", is_directory.to_string())])
			.send()?;
		Ok(())
	}

	fn delete_remote(&self, path: &str) -> Result<(), reqwest::Error> {
		// 根目录使用特殊标识符（虽然不应该删除根目录，但为了一致性）
		let api_path = if path == "." { "$ROOT" } else { path };
		let url = format!("{}/delete/{}", self.base_url, api_path);
		self.client.delete(&url).send()?;
		Ok(())
	}

	fn move_remote(&self, old_path: &str, new_path: &str) -> Result<(), reqwest::Error> {
		// 根目录使用特殊标识符
		let api_old_path = if old_path == "." { "$ROOT" } else { old_path };
		let api_new_path = if new_path == "." { "$ROOT" } else { new_path };
		let url = format!("{}/move/{}", self.base_url, api_old_path);
		self.client
			.post(&url)
			.json(&serde_json::json!({ "new_path": api_new_path }))
			.send()?;
		Ok(())
	}

	fn truncate_file(&self, path: &str, size: u64) -> Result<(), reqwest::Error> {
		// 根目录使用特殊标识符（虽然不应该截断目录，但为了一致性）
		let api_path = if path == "." { "$ROOT" } else { path };
		let url = format!("{}/truncate/{}", self.base_url, api_path);
		self.client
			.post(&url)
			.json(&serde_json::json!({ "size": size }))
			.send()?;
		Ok(())
	}

	fn timestamp_to_systime(ts: u64) -> SystemTime {
		UNIX_EPOCH + Duration::from_secs(ts)
	}
}

impl<'c, 'h: 'c> FileSystemHandler<'c, 'h> for HttpFsHandler {
	type Context = FileContext;

	fn create_file(
		&'h self,
		file_name: &U16CStr,
		_security_context: &IO_SECURITY_CONTEXT,
		_desired_access: winnt::ACCESS_MASK,
		_file_attributes: u32,
		_share_access: u32,
		create_disposition: u32,
		create_options: u32,
		_info: &mut OperationInfo<'c, 'h, Self>,
	) -> OperationResult<CreateFileInfo<Self::Context>> {
		if create_disposition > FILE_MAXIMUM_DISPOSITION {
			return Err(STATUS_INVALID_PARAMETER);
		}

		let path = self.normalize_path(file_name);
		let delete_on_close = create_options & FILE_DELETE_ON_CLOSE != 0;

		// 根目录特殊处理：总是存在，总是目录
		if path == "." {
			return Ok(CreateFileInfo {
				context: FileContext::new(path, false),
				is_dir: true,
				new_file_created: false,
			});
		}

		// 检查远程是否存在
		let remote_info = self.get_remote_file_info(&path).ok();
		let exists = remote_info.is_some();
		
		// 确定是否是目录
		let is_directory = if let Some(ref info) = remote_info {
			info.is_directory
		} else {
			create_options & FILE_DIRECTORY_FILE != 0
		};

		let mut new_file_created = false;

		// 根据 create_disposition 处理
		match create_disposition {
			FILE_CREATE => {
				if exists {
					return Err(STATUS_OBJECT_NAME_COLLISION);
				}
				self.create_remote(&path, is_directory)
					.map_err(|e| {
						eprintln!("[ERROR] create_remote failed: {:?}", e);
						STATUS_ACCESS_DENIED
					})?;
				new_file_created = true;
			}
			FILE_OPEN => {
				if !exists {
					return Err(STATUS_OBJECT_NAME_NOT_FOUND);
				}
			}
			FILE_OPEN_IF => {
				if !exists {
					self.create_remote(&path, is_directory)
						.map_err(|e| {
							eprintln!("[ERROR] create_remote (FILE_OPEN_IF) failed: {:?}", e);
							STATUS_ACCESS_DENIED
						})?;
					new_file_created = true;
				}
			}
			FILE_OVERWRITE => {
				if !exists {
					return Err(STATUS_OBJECT_NAME_NOT_FOUND);
				}
				if !is_directory {
					self.truncate_file(&path, 0)
						.map_err(|e| {
							eprintln!("[ERROR] truncate_file (FILE_OVERWRITE) failed: {:?}", e);
							STATUS_ACCESS_DENIED
						})?;
				}
			}
			FILE_OVERWRITE_IF | FILE_SUPERSEDE => {
				if !exists {
					self.create_remote(&path, is_directory)
						.map_err(|e| {
							eprintln!("[ERROR] create_remote (FILE_OVERWRITE_IF) failed: {:?}", e);
							STATUS_ACCESS_DENIED
						})?;
					new_file_created = true;
				} else if !is_directory {
					self.truncate_file(&path, 0)
						.map_err(|e| {
							eprintln!("[ERROR] truncate_file (FILE_OVERWRITE_IF) failed: {:?}", e);
							STATUS_ACCESS_DENIED
						})?;
				}
			}
			_ => return Err(STATUS_INVALID_PARAMETER),
		}

		Ok(CreateFileInfo {
			context: FileContext::new(path, delete_on_close),
			is_dir: is_directory,
			new_file_created,
		})
	}

	fn close_file(
		&'h self,
		_file_name: &U16CStr,
		_info: &OperationInfo<'c, 'h, Self>,
		context: &'c Self::Context,
	) {
		// 处理删除
		if context.delete_on_close {
			let _ = self.delete_remote(&context.path);
		}
	}

	fn read_file(
		&'h self,
		_file_name: &U16CStr,
		offset: i64,
		buffer: &mut [u8],
		_info: &OperationInfo<'c, 'h, Self>,
		context: &'c Self::Context,
	) -> OperationResult<u32> {
		let data = self
			.read_file_data(&context.path, offset as u64, buffer.len())
			.map_err(|e| {
				eprintln!("[ERROR] read_file_data failed for '{}': {:?}", context.path, e);
				STATUS_ACCESS_DENIED
			})?;

		let len = data.len().min(buffer.len());
		buffer[..len].copy_from_slice(&data[..len]);
		Ok(len as u32)
	}

	fn write_file(
		&'h self,
		_file_name: &U16CStr,
		offset: i64,
		buffer: &[u8],
		info: &OperationInfo<'c, 'h, Self>,
		context: &'c Self::Context,
	) -> OperationResult<u32> {
		let offset = if info.write_to_eof() {
			// 获取当前文件大小
			let file_info = self
				.get_remote_file_info(&context.path)
				.map_err(|e| {
					eprintln!("[ERROR] get_remote_file_info (write_to_eof) failed for '{}': {:?}", context.path, e);
					STATUS_ACCESS_DENIED
				})?;
			file_info.size
		} else {
			offset as u64
		};

		self.write_file_data(&context.path, offset, buffer)
			.map_err(|e| {
				eprintln!("[ERROR] write_file_data failed for '{}': {:?}", context.path, e);
				STATUS_ACCESS_DENIED
			})?;

		Ok(buffer.len() as u32)
	}

	fn flush_file_buffers(
		&'h self,
		_file_name: &U16CStr,
		_info: &OperationInfo<'c, 'h, Self>,
		_context: &'c Self::Context,
	) -> OperationResult<()> {
		Ok(())
	}

	fn get_file_information(
		&'h self,
		_file_name: &U16CStr,
		_info: &OperationInfo<'c, 'h, Self>,
		context: &'c Self::Context,
	) -> OperationResult<FileInfo> {
		// 根目录特殊处理
		if context.path == "." {
			return Ok(FileInfo {
				attributes: winnt::FILE_ATTRIBUTE_DIRECTORY,
				creation_time: SystemTime::now(),
				last_access_time: SystemTime::now(),
				last_write_time: SystemTime::now(),
				file_size: 0,
				number_of_links: 1,
				file_index: 0,
			});
		}

		let remote_info = self
			.get_remote_file_info(&context.path)
			.map_err(|e| {
				eprintln!("[ERROR] get_remote_file_info (get_file_information) failed for '{}': {:?}", context.path, e);
				STATUS_OBJECT_NAME_NOT_FOUND
			})?;

		let mut attributes = winnt::FILE_ATTRIBUTE_NORMAL;
		if remote_info.is_directory {
			attributes = winnt::FILE_ATTRIBUTE_DIRECTORY;
		}

		Ok(FileInfo {
			attributes,
			creation_time: Self::timestamp_to_systime(remote_info.created),
			last_access_time: Self::timestamp_to_systime(remote_info.accessed),
			last_write_time: Self::timestamp_to_systime(remote_info.modified),
			file_size: remote_info.size,
			number_of_links: 1,
			file_index: 0,
		})
	}

	fn find_files(
		&'h self,
		_file_name: &U16CStr,
		mut fill_find_data: impl FnMut(&FindData) -> FillDataResult,
		_info: &OperationInfo<'c, 'h, Self>,
		context: &'c Self::Context,
	) -> OperationResult<()> {
		let items = self
			.list_remote_directory(&context.path)
			.map_err(|e| {
				eprintln!("[ERROR] list_remote_directory (find_files) failed for '{}': {:?}", context.path, e);
				STATUS_ACCESS_DENIED
			})?;

		for item in items {
			let mut attributes = winnt::FILE_ATTRIBUTE_NORMAL;
			if item.is_directory {
				attributes = winnt::FILE_ATTRIBUTE_DIRECTORY;
			}

			let file_name =
				U16CString::from_str(&item.name).unwrap_or_else(|_| U16CString::from_str("?").unwrap());

			let find_data = FindData {
				attributes,
				creation_time: Self::timestamp_to_systime(item.created),
				last_access_time: Self::timestamp_to_systime(item.accessed),
				last_write_time: Self::timestamp_to_systime(item.modified),
				file_size: item.size,
				file_name,
			};

			fill_find_data(&find_data).map_err(|e| match e {
				FillDataError::BufferFull => STATUS_BUFFER_OVERFLOW,
				FillDataError::NameTooLong => STATUS_SUCCESS,
			})?;
		}

		Ok(())
	}

	fn set_file_attributes(
		&'h self,
		_file_name: &U16CStr,
		_file_attributes: u32,
		_info: &OperationInfo<'c, 'h, Self>,
		_context: &'c Self::Context,
	) -> OperationResult<()> {
		Ok(())
	}

	fn set_file_time(
		&'h self,
		_file_name: &U16CStr,
		_creation_time: FileTimeOperation,
		_last_access_time: FileTimeOperation,
		_last_write_time: FileTimeOperation,
		_info: &OperationInfo<'c, 'h, Self>,
		_context: &'c Self::Context,
	) -> OperationResult<()> {
		Ok(())
	}

	fn delete_file(
		&'h self,
		_file_name: &U16CStr,
		_info: &OperationInfo<'c, 'h, Self>,
		_context: &'c Self::Context,
	) -> OperationResult<()> {
		Ok(())
	}

	fn delete_directory(
		&'h self,
		_file_name: &U16CStr,
		info: &OperationInfo<'c, 'h, Self>,
		context: &'c Self::Context,
	) -> OperationResult<()> {
		if info.delete_pending() {
			let items = self
				.list_remote_directory(&context.path)
				.map_err(|e| {
					eprintln!("[ERROR] list_remote_directory (delete_directory) failed for '{}': {:?}", context.path, e);
					STATUS_ACCESS_DENIED
				})?;

			if !items.is_empty() {
				return Err(STATUS_DIRECTORY_NOT_EMPTY);
			}
		}

		Ok(())
	}

	fn move_file(
		&'h self,
		_file_name: &U16CStr,
		new_file_name: &U16CStr,
		_replace_if_existing: bool,
		_info: &OperationInfo<'c, 'h, Self>,
		context: &'c Self::Context,
	) -> OperationResult<()> {
		let new_path = self.normalize_path(new_file_name);

		self.move_remote(&context.path, &new_path)
			.map_err(|e| {
				eprintln!("[ERROR] move_remote failed from '{}' to '{}': {:?}", context.path, new_path, e);
				STATUS_ACCESS_DENIED
			})?;

		Ok(())
	}

	fn set_end_of_file(
		&'h self,
		_file_name: &U16CStr,
		offset: i64,
		_info: &OperationInfo<'c, 'h, Self>,
		context: &'c Self::Context,
	) -> OperationResult<()> {
		self.truncate_file(&context.path, offset as u64)
			.map_err(|e| {
				eprintln!("[ERROR] truncate_file (set_end_of_file) failed for '{}': {:?}", context.path, e);
				STATUS_ACCESS_DENIED
			})?;

		Ok(())
	}

	fn set_allocation_size(
		&'h self,
		_file_name: &U16CStr,
		alloc_size: i64,
		_info: &OperationInfo<'c, 'h, Self>,
		context: &'c Self::Context,
	) -> OperationResult<()> {
		self.truncate_file(&context.path, alloc_size as u64)
			.map_err(|e| {
				eprintln!("[ERROR] truncate_file (set_allocation_size) failed for '{}': {:?}", context.path, e);
				STATUS_ACCESS_DENIED
			})?;

		Ok(())
	}

	fn get_disk_free_space(&'h self, _info: &OperationInfo<'c, 'h, Self>) -> OperationResult<DiskSpaceInfo> {
		Ok(DiskSpaceInfo {
			byte_count: 10 * 1024 * 1024 * 1024,
			free_byte_count: 5 * 1024 * 1024 * 1024,
			available_byte_count: 5 * 1024 * 1024 * 1024,
		})
	}

	fn get_volume_information(&'h self, _info: &OperationInfo<'c, 'h, Self>) -> OperationResult<VolumeInfo> {
		Ok(VolumeInfo {
			name: U16CString::from_str("HTTP FS").unwrap(),
			serial_number: 0x19831116,
			max_component_length: 255,
			fs_flags: winnt::FILE_CASE_PRESERVED_NAMES | winnt::FILE_UNICODE_ON_DISK,
			fs_name: U16CString::from_str("HTTPFS").unwrap(),
		})
	}

	fn mounted(
		&'h self,
		_mount_point: &U16CStr,
		_info: &OperationInfo<'c, 'h, Self>,
	) -> OperationResult<()> {
		Ok(())
	}

	fn unmounted(&'h self, _info: &OperationInfo<'c, 'h, Self>) -> OperationResult<()> {
		Ok(())
	}
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let matches = Command::new("dokan-rust httpfs example")
		.author(env!("CARGO_PKG_AUTHORS"))
		.arg(
			Arg::new("server_url")
				.short('u')
				.long("url")
				.num_args(1)
				.value_name("SERVER_URL")
				.required(true)
				.help("HTTP storage server URL (e.g., http://localhost:8080)"),
		)
		.arg(
			Arg::new("mount_point")
				.short('m')
				.long("mount-point")
				.num_args(1)
				.value_name("MOUNT_POINT")
				.required(true)
				.help("Mount point path."),
		)
		.arg(
			Arg::new("single_thread")
				.short('t')
				.long("single-thread")
				.help("Force a single thread.")
				.action(ArgAction::SetTrue),
		)
		.arg(
			Arg::new("dokan_debug")
				.short('d')
				.long("dokan-debug")
				.help("Enable Dokan's debug output.")
				.action(ArgAction::SetTrue),
		)
		.get_matches();

	let server_url = matches.get_one::<String>("server_url").unwrap().to_string();
	let mount_point = U16CString::from_str(matches.get_one::<String>("mount_point").unwrap())?;

	let mut flags = MountFlags::empty();
	flags |= MountFlags::CURRENT_SESSION;
	if matches.get_flag("dokan_debug") {
		flags |= MountFlags::DEBUG | MountFlags::STDERR;
	}

	let options = MountOptions {
		single_thread: matches.get_flag("single_thread"),
		flags,
		..Default::default()
	};

	let handler = HttpFsHandler::new(server_url.clone());

	init();

	let mut mounter = FileSystemMounter::new(&handler, &mount_point, &options);

	println!("HTTP File System");
	println!("  Server: {}", server_url);
	println!("  Mount:  {}", mount_point.to_string_lossy());

	let file_system = mounter.mount()?;

	let mount_point_clone = mount_point.clone();
	ctrlc::set_handler(move || {
		if unmount(&mount_point_clone) {
			println!("File system will unmount...")
		} else {
			eprintln!("Failed to unmount file system.");
		}
	})
	.expect("failed to set Ctrl-C handler");

	println!("\nHTTP file system is mounted, press Ctrl-C to unmount.");

	drop(file_system);

	println!("File system is unmounted.");

	shutdown();

	Ok(())
}


