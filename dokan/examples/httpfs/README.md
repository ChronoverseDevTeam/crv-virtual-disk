# HTTP File System Example

通过 HTTP REST API 访问远程存储的虚拟文件系统。所有文件操作通过 HTTP 请求实时处理。

## 启动方法

### 1. 启动 HTTP 存储服务器

```bash
# 终端 1
cargo run --bin httpfs-server --features httpfs -- <存储目录> [端口]

# 示例
cargo run --bin httpfs-server --features httpfs -- D:\http-storage 8080
```

### 2. 挂载文件系统

```bash
# 终端 2
cargo run --example httpfs -- -u <服务器地址> -m <挂载点>

# 示例
cargo run --example httpfs -- -u http://localhost:8080 -m M:\
```

### 参数说明

**httpfs-server**:
- `<存储目录>`: 实际文件存储的本地目录
- `[端口]`: HTTP 服务器端口（默认 8080）

**httpfs**:
- `-u, --url`: HTTP 服务器地址（必需）
- `-m, --mount-point`: 挂载点（必需）
- `-t, --single-thread`: 单线程模式
- `-d, --dokan-debug`: 启用调试输出

## HTTP API

- `GET /info/:path` - 获取文件/目录信息
- `GET /list/:path` - 列出目录内容
- `GET /read/:path` - 读取文件内容
- `POST /write/:path` - 写入文件内容
- `PUT /create/:path` - 创建文件/目录
- `DELETE /delete/:path` - 删除文件/目录
- `POST /move/:path` - 移动/重命名
- `POST /truncate/:path` - 调整文件大小

## 使用示例

```powershell
# 访问虚拟磁盘
dir M:\

# 创建文件
echo "Hello" > M:\test.txt

# 读取文件
type M:\test.txt

# 创建目录
mkdir M:\mydir
```

所有操作会实时通过 HTTP 请求同步到远程存储服务器。
