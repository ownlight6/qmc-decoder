# QMC 解码器

> ⚠️ **仅支持 macOS**

使用 Rust 编写的 QQ 音乐加密音频文件（QMC1/QMC2 格式）解密工具，支持自动从 QQ 音乐 API 获取 ekey。

## 支持的格式

| 格式 | 扩展名 | 输出 | 需要密钥 |
|------|--------|------|----------|
| QMC1 | `.qmc0`, `.qmc3` | `.mp3` | 否（固定密码） |
| QMC1 | `.qmc2`, `.qmcogg` | `.ogg` | 否（固定密码） |
| QMC1 | `.qmcflac` | `.flac` | 否（固定密码） |
| QMC2 | `.mgg`, `.mgg0`, `.mgg1`, `.mggl` | `.ogg` | 是（ekey） |
| QMC2 | `.mflac`, `.mflac0`, `.mflach` | `.flac` | 是（ekey） |

## 关于 QMC 加密

QQ 音乐使用两种加密方案：

### QMC1（旧版）
使用基于固定种子的 XOR 密码。无需外部密钥——解密算法基于字节偏移量是完全确定性的。适用于旧版 `.qmc0`、`.qmc3`、`.qmcflac`、`.qmcogg` 文件。

### QMC2（新版）
使用 ekey（加密密钥），配合 Map XOR 密码（密钥 ≤ 300 字节）或修改版 RC4 流密码（密钥 > 300 字节）。ekey 经过 base64 编码，并使用腾讯的 TC-TEA 算法进一步加密。

ekey 有三种存储方式：

1. **QTag 格式**（旧版 QMC2）：ekey 与歌曲 ID 一起嵌入在文件末尾，后跟 "QTag" 魔术标记。解码器会自动提取。

2. **V1 格式**：文件末尾 4 字节存储原始密钥大小，密钥字节位于大小字段之前。解码器会自动提取。

3. **musicex 格式**（最新版，QQ 音乐 ≥ 19.57）：ekey **未嵌入**文件中。文件尾部仅包含元数据（歌曲 ID、歌曲 mid、文件名）。必须通过 `--ekey` 参数提供 ekey 或使用 `--fetch-ekey` 自动从 QQ 音乐 API 获取。

## 获取 ekey

对于使用 **musicex** 尾部的文件（最新版 QQ 音乐），ekey 并未存储在文件中。可选方案：

### 方案 1：自动从 QQ 音乐 API 获取（推荐）

使用 `--fetch-ekey` 参数自动从 QQ 音乐服务器获取 ekey。**需要本机已登录 QQ 音乐**：

```bash
qmc-decoder --fetch-ekey /path/to/file.mgg
```

解码器会自动：
1. 解析 musicex 尾部，提取歌曲的 media_mid 和文件名
2. 读取本机 QQ 音乐的认证凭据
3. 调用 `music.vkey.GetEVkey` API 获取 ekey
4. 使用获取的 ekey 解密文件

### 方案 2：使用旧版 QQ 音乐客户端

使用 QQ 音乐 ≤ 19.51 版本重新下载文件。旧版本会在文件尾部嵌入 ekey（QTag 格式），解码器可以自动提取。

### 方案 3：手动提供 ekey

使用 `--ekey` 参数提供 base64 编码的 ekey：

```bash
qmc-decoder --ekey "BASE64_EKEY" /path/to/file.mgg
```

> **注意：** 同时提供 `--ekey` 和 `--fetch-ekey` 时，优先使用显式指定的 `--ekey`。

## 使用方法

```bash
# 查看文件信息（检测格式、尾部类型、元数据）
qmc-decoder --info /path/to/file.mgg

# 解密 QMC1 文件（无需密钥）
qmc-decoder /path/to/file.qmcflac /path/to/output.flac

# 解密带有 QTag 尾部的 QMC2 文件（自动提取 ekey）
qmc-decoder /path/to/file.mgg1 /path/to/output.ogg

# 解密 musicex 文件，自动获取 ekey
qmc-decoder --fetch-ekey /path/to/file.mgg

# 解密 musicex 文件，手动提供 ekey
qmc-decoder --ekey "BASE64_EKEY" /path/to/file.mgg /path/to/output.ogg

# 查看文件信息并检查凭据状态（判断 --fetch-ekey 是否可用）
qmc-decoder --info --fetch-ekey /path/to/file.mgg

# 批量解密目录中所有支持的文件
qmc-decoder /path/to/input_dir/ /path/to/output_dir/

# 如果未指定输出路径，输出文件将使用相同文件名并更改扩展名
qmc-decoder /path/to/file.qmc0
# 生成：/path/to/file.mp3
```

## 构建

```bash
cargo build --release
```

编译后的二进制文件位于 `target/release/qmc-decoder`。

## 技术细节

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

完整的解密流程文档请参阅 [MGG 解密流程](MGG_DECRYPTION_FLOW.md)。

## 许可证

[GPL v3](LICENSE)
