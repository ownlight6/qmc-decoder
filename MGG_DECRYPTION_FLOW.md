# QQ Music MGG 加密文件解密流程

本文档记录了 QQ Music 客户端（macOS ≥ 19.57, Windows 22.x）加密音频文件（.mgg / .mflac）的完整解密流程，包括 musicex 文件格式分析、ekey 获取 API 调用、以及认证凭据的提取方法。**Windows 和 macOS 的凭据提取方式有根本差异**，详见[认证凭据提取](#认证凭据提取)章节。

## 目录

- [概述](#概述)
- [musicex 文件格式](#musicex-文件格式)
- [歌曲元数据数据库](#歌曲元数据数据库)
- [认证凭据提取](#认证凭据提取)
- [ekey 获取 API](#ekey-获取-api)
- [完整解密流程](#完整解密流程)
- [API 响应字段说明](#api-响应字段说明)
- [质量等级与文件名前缀](#质量等级与文件名前缀)
- [常见错误码](#常见错误码)
- [附录：二进制中的关键符号](#附录二进制中的关键符号)

---

## 概述

QQ 音乐 macOS 客户端从 19.57 版本开始采用 **musicex** 格式存储加密音频文件。与旧版 QTag/V1 格式不同，musicex 格式**不再将 ekey 嵌入文件尾部**，而是仅在文件末尾存储元数据（歌曲 ID、media_mid、文件名），ekey 需要在播放/下载时通过 API 实时获取。

### 加密体系对比

| 格式 | ekey 存储位置 | 是否需要 API | 解密难度 |
|------|-------------|-------------|---------|
| QTag (旧版 QMC2) | 文件尾部（QTag 标记前） | 否 | 低（自动提取） |
| V1 | 文件尾部（key_size 字段前） | 否 | 低（自动提取） |
| **musicex** (≥ 19.57) | **不嵌入文件** | **是** | 中（需 API 调用） |

---

## musicex 文件格式

### 文件结构

```
┌─────────────────────────────────────────┐
│          QMC2 加密音频数据               │  ← 以 "#!Qk" 魔术头开始
│          (filesize - footer_size)       │
├─────────────────────────────────────────┤
│          musicex Footer (192 字节)       │
│                                         │
│  +0x00: [4B] song_id (uint32 LE)       │
│  +0x04: [4B] quality_type1 (uint32 LE) │
│  +0x08: [4B] quality_type2 (uint32 LE) │
│  +0x0C: [60B] media_mid (UTF-16LE)     │
│  +0x48: [68B] filename (UTF-16LE)      │
│  +0x8C: [48B] padding (零填充)          │
│  +0xBC: reserved                        │
│  +0xC0: [4B] footer_size = 0xC0 (192) │
│  +0xC4: [4B] version = 1               │
│  +0xC8: [8B] magic = "musicex\0"       │
└─────────────────────────────────────────┘
```

### 字段说明

| 偏移 | 长度 | 类型 | 说明 | 示例 |
|------|------|------|------|------|
| 0x00 | 4 | uint32 LE | 歌曲ID | `0x0C27XXXX` |
| 0x04 | 4 | uint32 LE | 质量类型1 | `2` |
| 0x08 | 4 | uint32 LE | 质量类型2 | `2` |
| 0x0C | 60 | UTF-16LE | 歌曲 mid | `"00XXXXXXXXXXXX"` |
| 0x48 | 68 | UTF-16LE | 资源文件名 | `"O4M0XXXXXXXXXXXXXX.mgg"` |
| 0xC0 | 4 | uint32 LE | footer 大小 | `0xC0` (192) |
| 0xC4 | 4 | uint32 LE | 版本号 | `1` |
| 0xC8 | 8 | ASCII | 魔术标记 | `"musicex\0"` |

### 文件名格式

musicex footer 中的 filename 由质量前缀 + 资源 ID + 扩展名组成：

```
O4M0 + <K_SONG_RESERVE8> + .mgg  →  O4M0XXXXXXXXXXXXXX.mgg
```

其中 `<K_SONG_RESERVE8>` 对应数据库中的 `K_SONG_RESERVE8` 字段。

### 检测方法

读取文件末尾 8 字节，若为 `"musicex\0"` 则为 musicex 格式。然后向前读取 footer_size + 8 字节即可解析完整 footer。

---

## 歌曲元数据数据库

### 数据库路径

```
~/Library/Containers/com.tencent.QQMusicMac/Data/Library/Application Support/QQMusicMac/qqmusic.sqlite
```

### SONGS 表关键字段

| 字段 | 类型 | 说明 | 示例 |
|------|------|------|------|
| `id` | BIGINT | 歌曲ID | `<song_id>` |
| `name` | TEXT | 歌曲名 | `<歌曲名>` |
| `singer` | TEXT | 歌手 | `<歌手名>` |
| `album` | TEXT | 专辑 | `<专辑名>` |
| `file` | TEXT | 文件相对路径 | `/iQmc/<歌手>-<歌名>.mgg` |
| `filesize` | INTEGER | 文件大小 | `<字节数>` |
| `K_SONG_RESERVE1` | TEXT | media_mid | `<media_mid>` |
| `K_SONG_RESERVE3` | TEXT | album_mid | `<album_mid>` |
| `K_SONG_RESERVE8` | TEXT | 资源文件ID | `<file_id>` |
| `K_SONG_RESERVE9` | TEXT | 备用mid | `<备用mid>` |

### 查询示例

```sql
SELECT id, name, singer, K_SONG_RESERVE1, K_SONG_RESERVE8, file
FROM SONGS
WHERE file LIKE '%.mgg' OR file LIKE '%.mflac%';
```

---

## 认证凭据提取

**macOS 与 Windows 的凭据存储机制完全不同**，但最终获取的凭据（`uin` + `authst`）格式一致，均可用于调用 GetEVkey API。

---

### macOS 凭据提取

#### 凭据存储位置

```
~/Library/Containers/com.tencent.QQMusicMac/Data/Library/Preferences/com.tencent.QQMusicMac.plist
```

`AutoLoginUserInfo` 字段是一个 NSKeyedArchiver 编码的 plist 二进制数据，解码后包含以下关键信息：

#### 凭据字段

| 字段 | 说明 | 示例 |
|------|------|------|
| `nUserId` / `strUserAccount` | QQ UIN（用户ID） | `<数字ID>` |
| `loginType` | 登录类型 | `3`（微信登录）, `1`（QQ登录） |
| `strOpenId` | 微信 OpenID | `<32位十六进制字符串>` |
| `strAccessToken` | 访问令牌 | `<32位十六进制字符串>` |
| `strRefreshToken` | 刷新令牌 | `<32位十六进制字符串>` |
| `strRefreshKey` | 刷新密钥 | `<Base64编码字符串>` |
| **`strAuthst`** | **API 认证密钥** | `<长Base64编码字符串>` |

#### Python 提取脚本

```python
import plistlib
import struct

def extract_auth_credentials():
    plist_path = "~/Library/Containers/com.tencent.QQMusicMac/Data/Library/Preferences/com.tencent.QQMusicMac.plist"
    with open(plist_path, 'rb') as f:
        plist = plistlib.load(f)

    auto_login_data = plist['AutoLoginUserInfo']
    # NSKeyedArchiver format - parse the objects
    inner = plistlib.loads(auto_login_data)

    # Traverse NSKeyedArchiver structure to extract credentials
    objects = inner['$objects']
    # ... (需要遍历 UID 引用来提取具体字段)

    return {
        'uin': uin,
        'authst': auth_key,
        'openid': openid,
        'access_token': access_token,
    }
```

> **注意**：`strAuthst` 是调用 API 时的核心认证凭据（对应 API 参数 `authst`）。

---

### Windows 凭据提取

Windows 版 QQ Music（≥ 22.x）**没有持久化的 authst 文件**。与 macOS 不同，authst 仅存在于运行中的 QQMusic.exe 进程内存中，需要借助 Win32 API 从中提取。

#### 凭据来源

| 凭据 | 来源 | 格式 |
|------|------|------|
| `uin` | `%APPDATA%\Tencent\QQMusic\QQMusicServiceConfig.ini` | `[Account]\nUin=<数字QQ号>` |
| `authst` | QQMusic.exe 进程内存中的 JSON 字符串 | `"authst":"<Base64URL编码字符串>"` |

#### 提取原理

1. **读取 UIN**：直接解析 `QQMusicServiceConfig.ini` 中的 `Uin` 字段（纯文本 INI）
2. **定位进程**：通过 `CreateToolhelp32Snapshot` + `Process32FirstW` 枚举进程，匹配 `QQMusic.exe`、`WeChatAppEx.exe`、`qmbrowser.exe`
3. **扫描内存**：
   - 用 `OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)` 打开目标进程
   - 用 `VirtualQueryEx` 遍历进程地址空间，筛选 `MEM_COMMIT | MEM_PRIVATE | MEM_IMAGE` 可读内存区域
   - 用 `ReadProcessMemory` 以 1MB 分块读取内存
   - 搜索 `"authst":"` JSON key pattern，提取紧跟其后的 Base64URL 字符串

#### Rust 实现要点

```rust
// MEMORY_BASIC_INFORMATION 结构（Windows 10 1803+ 包含 PartitionId）
#[repr(C)]
struct MEMORY_BASIC_INFORMATION {
    base_address: *mut c_void,    // 8 bytes
    allocation_base: *mut c_void,  // 8 bytes
    allocation_protect: u32,       // 4 bytes
    partition_id: u16,            // Win 10 1803+，必须包含！
    region_size: usize,           // 8 bytes（#[repr(C)] 自动添加 2 字节 padding）
    state: u32,                   // 4 bytes
    protect: u32,                 // 4 bytes
    _type: u32,                   // 4 bytes
}
```

> ⚠️ **`partition_id` 是关键**：Windows 10 1803+ 在 `AllocationProtect` 和 `RegionSize` 之间新增了 2 字节的 `PartitionId`。缺少该字段将导致 `region_size`、`state`、`protect` 从错误的偏移量读取，使 `VirtualQueryEx` 返回的 `state` 值变成 `protect`（如 `PAGE_READWRITE=0x04` 而非 `MEM_COMMIT=0x1000`），导致所有内存区域都被跳过。

#### 性能优化

- **只扫描 `MEM_PRIVATE`（堆/栈）和 `MEM_IMAGE`（DLL 数据段）**类型，跳过 `MEM_MAPPED`（文件映射）
- **仅搜索 JSON key pattern**：不扫描所有 base64 字符串，找到 `"authst":"<token>"` 立即返回
- 实测 QQMusic.exe 进程扫描约 0.5 秒完成

---

### 平台对比汇总

| 维度 | macOS | Windows |
|------|-------|---------|
| **UIN 来源** | plist 中 NSKeyedArchiver 解码 | `QQMusicServiceConfig.ini` 纯文本 |
| **authst 来源** | plist 文件 `AutoLoginUserInfo` | QQMusic.exe 进程内存 |
| **存储机制** | NSKeyedArchiver（二进制 plist） | 运行时内存中的 JSON 字符串 |
| **提取方式** | 文件读取 + plist 解析 | Win32 API 跨进程内存读取 |
| **权限要求** | 无（同用户文件读取） | 无（同用户 OpenProcess） |
| **QQ Music 是否必须运行** | ❌ 否 | ✅ 是（authst 在内存中） |
| **authst 持久性** | 持久化到 plist 文件 | 仅运行时存在 |
| **实现复杂度** | 低 | 中 |
| **API platform 参数** | `"20"` | `"27"` |

---

## ekey 获取 API

### 端点

```
POST https://u.y.qq.com/cgi-bin/musicu.fcg
```

### 请求体

```json
{
    "comm": {
        "authst": "<strAuthst>",
        "ct": "19",
        "cv": "1859",
        "uin": "<QQ_UIN>",
        "tmeLoginType": "3"
    },
    "req_1": {
        "module": "music.vkey.GetEVkey",
        "method": "CgiGetEVkey",
        "param": {
            "filename": ["<filename_from_musicex>"],
            "guid": "10000",
            "songmid": ["<media_mid>"],
            "songtype": [1],
            "uin": "<QQ_UIN>",
            "loginflag": 1,
            "platform": "20",
            "ctx": 1
        }
    }
}
```

### 关键参数说明

| 参数 | 说明 | 来源 |
|------|------|------|
| `comm.authst` | 认证令牌 | `AutoLoginUserInfo.strAuthst` |
| `comm.uin` | 用户ID | `AutoLoginUserInfo.nUserId` |
| `comm.tmeLoginType` | 登录类型 | `3` = 微信, `1` = QQ |
| `param.filename` | 资源文件名 | musicex footer 的 filename 字段（**必须含 .mgg 后缀**） |
| `param.songmid` | 歌曲 mid | musicex footer 的 media_mid 字段 |
| `param.songtype` | 歌曲类型 | `1` = 加密文件（**必须为 1**） |
| `param.platform` | 平台标识 | `"20"`（macOS）/ `"27"`（Windows）— 自动根据编译目标选择 |

### ⚠️ 重要注意事项

1. **filename 必须包含 `.mgg` / `.mflac` 扩展名**：不带扩展名的文件名会返回 `result: 104005` 且 ekey 为空
2. **songtype 必须为 1**：表示请求加密资源的 ekey，为 0 则只返回 vkey 不返回 ekey
3. **使用 `GetEVkey` 而非 `GetVkey`**：`GetVkey` 只返回播放地址和 vkey，不返回 ekey
4. **authst 有效期**：令牌会过期，需通过 `strRefreshKey` 刷新

### 成功响应示例

```json
{
    "code": 0,
    "req_1": {
        "code": 0,
        "data": {
            "midurlinfo": [
                {
                    "songmid": "<media_mid>",
                    "filename": "<filename>.mgg",
                    "purl": "<filename>.mgg?guid=10000&vkey=...&uin=...&fromtag=120522",
                    "vkey": "<vkey_string>",
                    "ekey": "<base64编码的ekey字符串>",
                    "result": 0,
                    "auth_switch": 24246031,
                    "subcode": 0
                }
            ],
            "sip": [
                "http://aqqmusic.tc.qq.com/",
                "http://sjy6.stream.qqmusic.qq.com/"
            ],
            "expiration": 80400
        }
    }
}
```

### ekey 有效期

API 返回的 `expiration` 字段为 **80400 秒**（约 22.3 小时），ekey 过期后需要重新调用 API 获取。

### 可替换的 API 方法

| 方法 | module | 返回 ekey | 用途 |
|------|--------|----------|------|
| `CgiGetVkey` | `vkey.GetVkeyServer` | ❌ | 获取播放 URL 和 vkey（非加密） |
| `CgiGetEVkey` | `music.vkey.GetEVkey` | ✅ | 获取加密文件的 ekey 和 vkey |
| `CgiGetEDownUrl` | `music.vkey.GetEDownUrl` | ❌ | 获取加密下载 URL（result: 104005） |
| `CgiGetDownUrl` | `music.vkey.GetDownUrl` | ❌ | 获取普通下载 URL（仅 vkey） |

---

## 完整解密流程

```
┌─────────────────────────────────┐
│  1. 读取 .mgg/.mflac 文件       │
│     检测 "musicex\0" 魔术标记    │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  2. 解析 musicex Footer         │
│     - song_id (bytes 0-3)       │
│     - media_mid (bytes 0x0C+)   │
│     - filename (bytes 0x48+)    │
│     - footer_size (last -16:-12)│
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  3. 提取认证凭据                │
│                                 │
│  ┌─ macOS ──────────────────┐   │
│  │ 读取 plist 文件           │   │
│  │ 解码 AutoLoginUserInfo   │   │
│  │ 获取 uin + authst        │   │
│  └──────────────────────────┘   │
│                                 │
│  ┌─ Windows ────────────────┐   │
│  │ 读取 QQMusicServiceConfig│   │
│  │ .ini → UIN               │   │
│  │ 扫描 QQMusic.exe 进程内存│   │
│  │ 搜索 "authst":"..." JSON │   │
│  │ 提取 authst + uin        │   │
│  └──────────────────────────┘   │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  4. 调用 GetEVkey API           │
│     POST musicu.fcg             │
│     module: music.vkey.GetEVkey │
│     filename + songmid + songtype=1 │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  5. 从响应中提取 ekey            │
│     midurlinfo[0].ekey          │
│     (base64 编码的加密密钥)       │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  6. 使用 QMC2 算法解密          │
│     - Base64 解码 ekey          │
│     - TC-TEA 解密 (EncV2 → V1)  │
│     - 密钥推导 (header + body)  │
│     - Map XOR (key≤300) 或      │
│       RC4 流密码 (key>300)      │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  7. 输出解密后的音频文件         │
│     .mgg → .ogg                 │
│     .mflac → .flac              │
└─────────────────────────────────┘
```

---

## API 响应字段说明

### midurlinfo 各字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `songmid` | string | 歌曲 mid |
| `filename` | string | 资源文件名 |
| `purl` | string | 播放/下载相对路径（含 vkey 参数） |
| `vkey` | string | 验证密钥（用于 URL 鉴权） |
| `ekey` | string | 加密密钥（base64，用于 QMC2 解密） |
| `result` | int | 结果码，0 = 成功 |
| `subcode` | int | 子结果码 |
| `auth_switch` | int | 鉴权开关标志位 |
| `auth_switch2` | int | 鉴权开关标志位2 |
| `isbuy` | int | 是否已购买 |
| `pneedbuy` | int | 是否需要购买 |
| `isonly` | int | 是否为独家 |
| `sip` | string[] | CDN 前缀列表 |
| `expiration` | int | 过期时间（秒） |

---

## 质量等级与文件名前缀

| 前缀 | 音质 | 格式 | 加密扩展名 | 说明 |
|------|------|------|-----------|------|
| `C400` | 标准音质 | M4A | - | 非加密 |
| `M500` | 高品质 | MP3 | - | 非加密 |
| `M800` | 超高品质 | MP3 | `.mgg` | QMC2 加密 |
| `O400` | 高品质 | OGG | `.mgg` | QMC2 加密 |
| `O4M0` | 超高品质 | OGG | `.mgg` | QMC2 加密（≥320kbps） |
| `F000` | 无损 | FLAC | `.mflac` | QMC2 加密 |

> 文件名 = 前缀 + K_SONG_RESERVE8 + 扩展名
> 示例：`O4M0` + `<file_id>` + `.mgg` = `O4M0<file_id>.mgg`

---

## 常见错误码

| result | 含义 | 解决方案 |
|--------|------|---------|
| `0` | 成功 | - |
| `104005` | 需要VIP/加密资源请求方式错误 | 确认 filename 包含 `.mgg` 扩展名且 songtype=1；检查 authst 是否过期 |
| 空响应 | 无权限 | 确认用户已登录且为 VIP（加密资源需要付费） |

---

## 附录：二进制中的关键符号

通过分析 QQ Music macOS 二进制文件发现的关键类和方法：

| 符号 | 说明 |
|------|------|
| `-[SongInfo getEkeyWithSongRateType:]` | 按音质类型获取 ekey |
| `-[SongInfo setEkeyWithSongRateType:ekey:]` | 设置歌曲的 ekey |
| `+[QMStreamEncrypt initWithEKey:]` | 使用 ekey 初始化加密解密器 |
| `+[QMStreamEncrypt convertEncFile:toDecFile:Ekey:]` | 使用 ekey 解密文件 |
| `+[QMStreamEncrypt writeEKeyToEndOfFile:ekey:]` | 将 ekey 写入文件尾部 |
| `+[QMStreamEncrypt readEKeyFromFile:]` | 从文件读取 ekey |
| `-[QueryvkeyCgi setQueryVKeyInfo:songRate:useEncStream:downloadFrom:isDownloadTask:]` | 设置 vkey 查询参数 |
| `-[QueryvkeyCgi parseResponseData:]` | 解析 API 响应 |
| `[QMStreamEncrypt isNotEncryptData:fileExtension:]` | 判断文件是否加密 |
| `CgiGetEVkey` / `music.vkey.GetEVkey` | 获取加密资源的 ekey |
| `CgiGetVkey` / `vkey.GetVkeyServer` | 获取播放 vkey（不含 ekey） |
| `CgiGetEDownUrl` / `music.vkey.GetEDownUrl` | 获取加密下载 URL |
| `CgiGetDownUrl` / `music.vkey.GetDownUrl` | 获取普通下载 URL |

### API 通信格式

QQ Music 客户端与服务器使用 protobuf 编码通信（可见日志格式 `{"enc":%d;"proto_len":%d;"data_len":%d;"cmd":%d}`），但也支持 JSON 格式（如本文档中的 API 调用示例）。

### 凭据刷新机制

- 客户端通过 `onTimerRefreshMusicKey` 定时器自动刷新认证密钥
- `QQMusicLastRenewTicketTime` 记录上次续签时间
- `strRefreshKey` 和 `strRefreshToken` 用于令牌刷新

