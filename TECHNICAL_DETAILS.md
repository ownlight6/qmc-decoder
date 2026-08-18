# 🔐 解密原理与技术细节

本文档介绍 QMC Decoder 的加密方案、解密算法、ekey 获取方式与核心 API。
端到端解密流程的完整说明请参阅 [MGG 解密流程](MGG_DECRYPTION_FLOW.md)。

---

## 关于 QMC 加密

QQ 音乐使用两种加密方案：

### QMC1（旧版）
使用基于固定种子的 XOR 密码。无需外部密钥——解密算法基于字节偏移量是完全确定性的。适用于旧版 `.qmc0`、`.qmc3`、`.qmcflac`、`.qmcogg` 文件。

### QMC2（新版）
使用 ekey（加密密钥），配合 Map XOR 密码（密钥 ≤ 300 字节）或修改版 RC4 流密码（密钥 > 300 字节）。ekey 经过 base64 编码，并使用腾讯的 TC-TEA 算法进一步加密。

ekey 有三种存储方式：

1. **QTag 格式**（旧版 QMC2）：ekey 与歌曲 ID 一起嵌入在文件末尾，后跟 "QTag" 魔术标记。解码器会自动提取。

2. **V1 格式**：文件末尾 4 字节存储原始密钥大小，密钥字节位于大小字段之前。解码器会自动提取。

3. **musicex 格式**（最新版，QQ 音乐 ≥ 19.57）：ekey **未嵌入**文件中。文件尾部仅包含元数据（歌曲 ID、歌曲 mid、文件名）。必须提供 ekey 或使用 `fetch_ekey` 自动从 QQ 音乐 API 获取。

## 🔧 算法细节

### QMC1 算法
- 通过 8×7 种子映射表使用 Z 字形遍历模式生成 64 字节密钥表
- 每个字节与密钥表中索引 `(offset % 0x7FFF) & 0x7F` 处的值进行异或，索引 > 0x3F 时进行反射
- 偏移量 0x8000、0x10000 等处的字节被跳过

### QMC2 密钥推导
1. 对 ekey 进行 base64 解码
2. 如果以 "QQMusic EncV2,Key:" 开头，则为 EncV2 ekey：
   - 使用阶段 1 密钥 `386ZJY!@#*$%^&)(` 通过 TC-TEA 解密前缀后的数据
   - 使用阶段 2 密钥 `**#!(#$%&^a1cZ,T` 通过 TC-TEA 解密上一步结果
   - 对结果进行 base64 解码得到 EncV1 ekey
3. 尝试 EncV1 格式解析：拆分前 8 字节为头部，推导 TEA 密钥，TC-TEA 解密主体
4. 如果 TC-TEA 解密失败（API 获取的原始密钥），则直接使用完整解码数据作为密钥
5. 最终密钥为 头部 + 解密后主体（EncV1）或原始解码字节（API 密钥）

### QMC2 Map 密码（密钥长度 ≤ 300）
- 偏移量 `i` 处的每个字节与 `scramble(key[(i² + 71214) % key_len], (i² + 71214) % key_len)` 进行异或
- `scramble(value, index)` = `value.rotate_left((index + 4) & 7) | value.rotate_right((index + 4) & 7)`

### QMC2 RC4 密码（密钥长度 > 300）
- 前 128 字节（第一段）使用直接密钥查找和段密钥计算
- 剩余字节使用修改版 RC4 流密码，以 5120 字节为一块进行分段
- 每段根据段 ID 计算丢弃计数来重新初始化 RC4 状态

### musicex 尾部格式

使用 musicex 尾部的文件在文件末尾存储元数据：
- 偏移 0x00：歌曲 ID（4 字节，小端序）
- 偏移 0x04：音质类型字段（8 字节）
- 偏移 0x0C：Media MID（60 字节，UTF-16LE）
- 偏移 0x48：文件名（68 字节，UTF-16LE，包含扩展名如 `.mgg`）
- 偏移（尾部大小 - 16）：尾部大小（4 字节 LE）
- 偏移（尾部大小 - 12）：版本号（4 字节 LE，= 1）
- 偏移（尾部大小 - 8）：魔术标记 `"musicex\0"`

## 🔑 获取 ekey

对于使用 **musicex** 尾部的文件（最新版 QQ 音乐），ekey 并未存储在文件中。可选方案：

### 方案 1：自动从 QQ 音乐 API 获取（推荐）

调用 `fetch_ekey(input_path).await` 从 QQ 音乐服务器获取 ekey。**需要本机已登录 QQ 音乐**。

解码器会自动：
1. 解析 musicex 尾部，提取歌曲的 media_mid 和文件名
2. 读取本机 QQ 音乐的认证凭据
3. 调用 `music.vkey.GetEVkey` API 获取 ekey
4. 使用获取的 ekey 解密文件

**平台差异：**

| 平台 | 认证凭据来源 | 前置条件 |
|------|-------------|---------|
| **macOS** | QQ Music plist 文件 | 已登录 QQ Music 即可（无需运行中） |
| **Windows** | QQMusic.exe 进程内存 | QQ 音乐**必须正在运行且已登录**（authst 仅存在于运行时内存中） |

> 可调用 `get_qqmusic_credentials()` 检查本机凭据状态，确认自动获取是否可用。

### 方案 2：使用旧版 QQ 音乐客户端

使用 QQ 音乐 ≤ 19.51 版本重新下载文件。旧版本会在文件尾部嵌入 ekey（QTag 格式），解码器可以自动提取。

### 方案 3：手动提供 ekey

`decrypt_file` 的 `ekey` 参数接受 base64 编码的 ekey（例如 `QTag`/`musicex` 元数据中解出的值）。

> **注意：** 显式提供的 ekey 优先于自动获取。

## 🛠️ 命令层与核心 API

Rust 侧（`src-tauri/src/commands.rs`）通过 `invoke` 向前端暴露以下命令：

| 命令 | 说明 |
|------|------|
| `decrypt_paths` | 批量 / 单文件解密，返回逐文件结果 |
| `get_file_info` | 不解密，返回格式、尾部类型与元数据 |
| `fetch_ekey_musicex` | 从 QQ 音乐 API 自动获取 ekey |
| `check_credentials` | 检查本机 QQ 音乐凭据是否可读 |
| `pick_files` / `pick_folder` | 原生文件 / 文件夹选择（rfd） |
| `get_default_download_dir` | 返回 QQ 音乐下载目录，作为选择器默认位置 |

核心逻辑在 `qmc_decoder` 库中，命令层只是薄封装：

```rust
use qmc_decoder::{decrypt_file, process_directory, info_file, Format};

// 解密单个文件（QMC1 无需密钥；QMC2 需 ekey 或 fetch_ekey）
let out = qmc_decoder::determine_output_path(input, None, format);
let result = decrypt_file(input, &out, format, ekey, fetch_ekey)?;

// 批量解密目录中所有支持的加密文件
let results = process_directory(input_dir, Some(output_dir), ekey, fetch_ekey);

// 仅读取文件信息（检测格式、尾部类型、元数据）
let info = info_file(input, format, /* check_credentials */ true)?;

// 从 QQ 音乐 API 自动获取 ekey（musicex 文件）
let ekey = qmc_decoder::fetch_ekey(input).await?;
```

关键函数：

| 函数 | 说明 |
|------|------|
| `decrypt_file` | 解密单个文件并写入磁盘，返回 `DecryptResult` |
| `process_directory` | 批量处理目录下所有支持的加密文件，逐文件收集结果 |
| `info_file` | 不解密，仅检测格式、尾部类型（QTag / V1 / musicex / 未知） |
| `detect_footer` | 从文件字节检测尾部类型 |
| `fetch_ekey` | 从 QQ 音乐 API 拉取 ekey（需本机已登录 QQ 音乐） |
| `get_qqmusic_credentials` | 读取本机 QQ 音乐认证凭据 |
| `Format::from_extension` | 依扩展名识别格式，如 `.qmcflac` → `Format::QmcFlac` |

## 🧪 测试覆盖

```bash
cargo test
```

包含 17 个单元测试，覆盖：

- QMC1 密钥表生成、边界行为、周期性
- QMC2 密钥推导（EncV1、EncV2、TC-TEA 加解密往返）
- QMC2 Map 密码和 RC4 密码
- musicex 尾部解析（正常数据、无效数据）
- UTF-16LE 字符串解码
- ekey 解析往返测试

> 自动获取 ekey 相关接口需要本机已登录 QQ 音乐，单元测试不覆盖实际网络请求。
