//! Ekey auto-fetch module
//!
//! Automatically fetches the ekey from QQ Music's API for musicex-format files.
//! This requires:
//! 1. The file to have a musicex footer (so we can extract media_mid and filename)
//! 2. QQ Music to be logged in on this computer (so we can read auth credentials)
//! 3. Network access to u.y.qq.com

use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use std::io::Cursor;
#[cfg(target_os = "macos")]
use plist::Value;
use std::path::Path;

use crate::qmc2::Qmc2Crypto;

/// HTTP User-Agent header sent to QQ Music API. Must match a real browser
/// so the API doesn't reject the request. Platform-specific to avoid
/// being flagged as a mismatched client.
#[cfg(target_os = "windows")]
const QQ_API_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";
#[cfg(not(target_os = "windows"))]
const QQ_API_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)";

/// QQ Music platform code.  20 = macOS, 27 = Windows PC.
#[cfg(target_os = "windows")]
const QQ_API_PLATFORM: &str = "27";
#[cfg(not(target_os = "windows"))]
const QQ_API_PLATFORM: &str = "20";

/// Errors that can occur during ekey fetching
#[derive(Debug)]
pub enum EkeyFetchError {
    /// The file doesn't have a musicex footer
    NoMusicexFooter,
    /// QQ Music credentials not found (app not installed or not logged in)
    NoCredentials(String),
    /// The API returned an error response
    ApiError { code: i64, message: String },
    /// The API returned successfully but with an empty ekey (VIP required or auth expired)
    EmptyEkey { result_code: i64 },
    /// Network/HTTP error
    NetworkError(String),
    /// Failed to parse plist or JSON
    ParseError(String),
    /// STag file whose ekey could not be resolved from the local library
    StagUnresolved(String),
}

impl std::fmt::Display for EkeyFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EkeyFetchError::NoMusicexFooter => {
                write!(f, "File does not have a musicex footer; auto-fetch is only supported for musicex-format files")
            }
            EkeyFetchError::NoCredentials(msg) => {
                write!(f, "QQ Music credentials not found: {}", msg)
            }
            EkeyFetchError::ApiError { code, message } => {
                write!(f, "QQ Music API error (code {}): {}", code, message)
            }
            EkeyFetchError::EmptyEkey { result_code } => {
                match result_code {
                    104003 => write!(f, "API returned empty ekey (result=104003). This usually means: \
                        (1) the song requires VIP access — your account may not have permission, or \
                        (2) the song is not available in this region. Try a different song."),
                    104005 => write!(f, "API returned empty ekey (result=104005). This usually means: \
                        (1) your QQ Music login has expired — try re-opening the app, or \
                        (2) the song requires VIP access — ensure your account has an active subscription"),
                    _ => write!(f, "API returned empty ekey (result={}). The song may require VIP access or the auth token may have expired", result_code),
                }
            }
            EkeyFetchError::NetworkError(msg) => {
                write!(f, "Network error: {}", msg)
            }
            EkeyFetchError::ParseError(msg) => {
                write!(f, "Parse error: {}", msg)
            }
            EkeyFetchError::StagUnresolved(msg) => {
                write!(f, "STag file: {}", msg)
            }
        }
    }
}

impl std::error::Error for EkeyFetchError {}

/// Musicex footer metadata extracted from a .mgg/.mflac file
#[derive(Debug, Clone)]
pub struct MusicexInfo {
    pub song_id: u32,
    pub media_mid: String,
    pub filename: String,
}

/// QQ Music authentication credentials
#[derive(Debug, Clone)]
pub struct QQMusicCredentials {
    pub uin: String,
    pub authst: String,
}

// ============================================================
// API request/response types
// ============================================================

#[derive(Serialize)]
struct MusicuRequest {
    comm: MusicuComm,
    #[serde(rename = "req_1")]
    req: MusicuReq1,
}

#[derive(Serialize)]
struct MusicuComm {
    authst: String,
    ct: &'static str,
    cv: &'static str,
    uin: String,
    #[serde(rename = "tmeLoginType")]
    tme_login_type: &'static str,
}

#[derive(Serialize)]
struct MusicuReq1 {
    module: &'static str,
    method: &'static str,
    param: MusicuParam,
}

#[derive(Serialize)]
struct MusicuParam {
    filename: Vec<String>,
    guid: &'static str,
    songmid: Vec<String>,
    songtype: Vec<i32>,
    uin: String,
    loginflag: i32,
    platform: &'static str,
    ctx: i32,
}

#[derive(Deserialize)]
struct MusicuResponse {
    #[serde(rename = "req_1")]
    req: Option<MusicuReq1Response>,
}

#[derive(Deserialize)]
struct MusicuReq1Response {
    code: Option<i64>,
    data: Option<MusicuData>,
}

#[derive(Deserialize)]
struct MusicuData {
    midurlinfo: Option<Vec<MidUrlInfo>>,
    #[allow(dead_code)]
    msg: Option<String>,
}

#[derive(Deserialize)]
struct MidUrlInfo {
    ekey: Option<String>,
    result: Option<i64>,
    #[allow(dead_code)]
    purl: Option<String>,
    #[allow(dead_code)]
    filename: Option<String>,
}

// ============================================================
// NSKeyedArchiver parsing (macOS only)
// ============================================================

/// Minimal NSKeyedArchiver parser for extracting QQ Music credentials.
///
/// The `AutoLoginUserInfo` in the QQ Music plist is encoded as an NSKeyedArchiver
/// binary plist. We need to resolve UID references in the `$objects` array to
/// extract `strUserAccount` (UIN) and `strAuthst` (auth key).
#[cfg(target_os = "macos")]
fn parse_nskeyed_archiver_credentials(data: &[u8]) -> Result<QQMusicCredentials, EkeyFetchError> {
    let cursor = Cursor::new(data);
    let plist_value = Value::from_reader(cursor).map_err(|e| {
        EkeyFetchError::ParseError(format!("Failed to parse AutoLoginUserInfo plist: {}", e))
    })?;

    let dict = plist_value.as_dictionary().ok_or_else(|| {
        EkeyFetchError::ParseError("AutoLoginUserInfo is not a dictionary".to_string())
    })?;

    // Verify it's an NSKeyedArchiver plist
    let _archiver = dict
        .get("$archiver")
        .and_then(|v: &Value| v.as_string())
        .unwrap_or("");
    // We don't strictly require the archiver check — just try to parse the objects

    // Get the $objects array
    let objects = dict
        .get("$objects")
        .and_then(|v: &Value| v.as_array())
        .ok_or_else(|| {
            EkeyFetchError::ParseError("Missing $objects in NSKeyedArchiver".to_string())
        })?;

    // Helper to get a string from a UID reference
    // In NSKeyedArchiver, UID references are plist::Value::Uid values
    fn resolve_uid_string(objects: &[Value], uid_val: &Value) -> Option<String> {
        let idx = match uid_val {
            Value::Uid(uid) => uid.get() as usize,
            _ => return None,
        };
        objects
            .get(idx)
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
    }

    // Find the user info dictionary (the object with strAuthst key)
    let mut uin = None;
    let mut authst = None;

    for obj in objects.iter() {
        if let Some(d) = obj.as_dictionary() {
            if d.contains_key("strAuthst") {
                // Found the user info dict
                if let Some(v) = d.get("strAuthst") {
                    authst = resolve_uid_string(objects, v);
                }
                if let Some(v) = d.get("strUserAccount") {
                    uin = resolve_uid_string(objects, v);
                }
                // Also check for nCurrUseId as fallback (it's a direct integer, not UID)
                if uin.is_none() {
                    if let Some(v) = d.get("nCurrUseId") {
                        // nCurrUseId is typically a direct unsigned integer value
                        if let Some(int_val) = v.as_unsigned_integer() {
                            uin = Some(int_val.to_string());
                        } else if let Some(int_val) = v.as_signed_integer() {
                            uin = Some(int_val.to_string());
                        }
                    }
                }
                break;
            }
        }
    }

    let uin = uin.ok_or_else(|| {
        EkeyFetchError::ParseError(
            "Could not find UIN (strUserAccount/nCurrUseId) in AutoLoginUserInfo".to_string(),
        )
    })?;
    let authst = authst.ok_or_else(|| {
        EkeyFetchError::ParseError(
            "Could not find auth key (strAuthst) in AutoLoginUserInfo".to_string(),
        )
    })?;

    Ok(QQMusicCredentials { uin, authst })
}

// ============================================================
// STag footer support (newer QMC2: ekey is not embedded)
// ============================================================

/// Metadata extracted from an STag footer.
#[derive(Debug, Clone)]
pub struct StagInfo {
    pub song_id: u32,
    pub media_mid: String,
}

/// Parse the STag footer: `<meta><u32 BE meta_size><"STag">`.
/// meta is CSV, typically `<song_id>,<n>,<media_mid>`.
pub fn parse_stag_footer(data: &[u8]) -> Result<StagInfo, EkeyFetchError> {
    if data.len() < 12 || &data[data.len() - 4..] != b"STag" {
        return Err(EkeyFetchError::ParseError(
            "File has neither a musicex nor an STag footer; auto-fetch needs one of them \
             (provide the ekey manually instead)"
                .to_string(),
        ));
    }
    let meta_size = u32::from_be_bytes(data[data.len() - 8..data.len() - 4].try_into().unwrap())
        as usize;
    if meta_size == 0 || meta_size + 8 > data.len() {
        return Err(EkeyFetchError::ParseError(
            "Invalid STag footer: bad meta size".to_string(),
        ));
    }
    let meta = String::from_utf8_lossy(&data[data.len() - 8 - meta_size..data.len() - 8])
        .to_string();
    let fields: Vec<&str> = meta.split(',').collect();
    let media_mid = fields
        .last()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if media_mid.is_empty() {
        return Err(EkeyFetchError::ParseError(
            "Invalid STag footer: media_mid is missing".to_string(),
        ));
    }
    let song_id = fields
        .first()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    Ok(StagInfo { song_id, media_mid })
}

/// Directory where the QQ Music macOS client keeps its own downloaded copies.
#[cfg(target_os = "macos")]
fn qqmusic_local_library_dir() -> std::path::PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join("Library/Containers/com.tencent.QQMusicMac/Data/Library/Application Support/QQMusicMac/iQmc")
}

/// STag files carry no songmid, but the GetEVkey API needs one.
/// QQ Music's own local copies use the musicex footer which *does* carry the
/// songmid, so resolve it by finding the same song in the local library.
#[cfg(target_os = "macos")]
fn resolve_stag_via_local_library(stag: &StagInfo) -> Result<MusicexInfo, EkeyFetchError> {
    use std::io::{Read, Seek, SeekFrom};

    let dir = qqmusic_local_library_dir();
    let entries = std::fs::read_dir(&dir).map_err(|e| {
        EkeyFetchError::StagUnresolved(format!(
            "cannot read the local QQ Music library ({}): {}",
            dir.display(),
            e
        ))
    })?;

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(
            ext.as_str(),
            "mflac" | "mflac0" | "mflach" | "mgg" | "mgg0" | "mgg1" | "mggl"
        ) {
            continue;
        }
        let mut file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len < 256 {
            continue;
        }
        let mut buf = vec![0u8; 256];
        if file.seek(SeekFrom::End(-256)).is_err() || file.read_exact(&mut buf).is_err() {
            continue;
        }
        if let Ok(info) = parse_musicex_footer(&buf) {
            if info.song_id == stag.song_id || info.filename.contains(&stag.media_mid) {
                eprintln!(
                    "Resolved STag via local library copy: {} (song_id={}, mid={})",
                    path.display(),
                    info.song_id,
                    info.media_mid
                );
                return Ok(info);
            }
        }
    }

    Err(EkeyFetchError::StagUnresolved(format!(
        "no musicex copy of song_id={} was found in the local QQ Music library ({}). \
         Re-download the song in QQ Music, or provide the ekey manually.",
        stag.song_id,
        dir.display()
    )))
}

#[cfg(not(target_os = "macos"))]
fn resolve_stag_via_local_library(stag: &StagInfo) -> Result<MusicexInfo, EkeyFetchError> {
    Err(EkeyFetchError::StagUnresolved(format!(
        "automatic resolution is only implemented on macOS (song_id={}, media_mid={}); \
         provide the ekey manually",
        stag.song_id, stag.media_mid
    )))
}

// ============================================================
// Public API
// ============================================================

/// Parse the musicex footer from file data to extract metadata needed for ekey fetching.
pub fn parse_musicex_footer(data: &[u8]) -> Result<MusicexInfo, EkeyFetchError> {
    // Check for "musicex\0" magic at end
    if data.len() < 16 || &data[data.len() - 8..] != b"musicex\x00" {
        return Err(EkeyFetchError::NoMusicexFooter);
    }

    let magic_start = data.len() - 8;
    let version_start = magic_start - 4;
    let footer_size_start = version_start - 4;

    if footer_size_start < 4 {
        return Err(EkeyFetchError::ParseError(
            "musicex footer is too short".to_string(),
        ));
    }

    let version = u32::from_le_bytes(data[version_start..magic_start].try_into().unwrap());
    let footer_size =
        u32::from_le_bytes(data[footer_size_start..version_start].try_into().unwrap());

    // footer_size is the total footer size including the 16-byte trailer
    let metadata_size = (footer_size as usize).saturating_sub(16);

    if version != 1 || metadata_size == 0 || metadata_size > footer_size_start {
        return Err(EkeyFetchError::ParseError(format!(
            "Invalid musicex footer: version={}, footer_size={}",
            version, footer_size
        )));
    }

    let footer_start = data.len() - (footer_size as usize);
    let meta = &data[footer_start..footer_size_start];

    // Parse the musicex metadata structure:
    // +0x00: 4 bytes song_id (uint32 LE)
    // +0x04: 4 bytes quality_type1
    // +0x08: 4 bytes quality_type2
    // +0x0C: 60 bytes media_mid (UTF-16LE, null-terminated)
    // +0x48: 68 bytes filename (UTF-16LE, null-terminated)
    let song_id = if meta.len() > 0x04 {
        u32::from_le_bytes(meta[0x00..0x04].try_into().unwrap_or([0u8; 4]))
    } else {
        0
    };

    let media_mid = read_utf16_le_string(meta, 0x0C, 60);
    let filename = read_utf16_le_string(meta, 0x48, 68);

    if media_mid.is_empty() || filename.is_empty() {
        return Err(EkeyFetchError::ParseError(
            "Could not extract media_mid or filename from musicex footer".to_string(),
        ));
    }

    Ok(MusicexInfo {
        song_id,
        media_mid,
        filename,
    })
}

/// Read a null-terminated UTF-16LE string from a byte slice at the given offset.
fn read_utf16_le_string(data: &[u8], offset: usize, max_len: usize) -> String {
    let mut chars = Vec::new();
    let end = std::cmp::min(offset + max_len, data.len());
    let mut i = offset;
    while i + 1 < end {
        let code = u16::from_le_bytes([data[i], data[i + 1]]);
        if code == 0 {
            break;
        }
        chars.push(code);
        i += 2;
    }
    String::from_utf16_lossy(&chars)
}

/// Get QQ Music credentials from the macOS plist file.
///
/// Reads the `AutoLoginUserInfo` from the QQ Music preferences plist,
/// parses the NSKeyedArchiver format, and extracts the UIN and auth key.
#[cfg(target_os = "macos")]
pub fn get_qqmusic_credentials() -> Result<QQMusicCredentials, EkeyFetchError> {
    let plist_path = dirs::home_dir()
        .ok_or_else(|| {
            EkeyFetchError::NoCredentials("Cannot determine home directory".to_string())
        })?
        .join("Library/Containers/com.tencent.QQMusicMac/Data/Library/Preferences/com.tencent.QQMusicMac.plist");

    if !plist_path.exists() {
        return Err(EkeyFetchError::NoCredentials(format!(
            "QQ Music preferences not found at {}",
            plist_path.display()
        )));
    }

    let plist_value = Value::from_file(&plist_path).map_err(|e| {
        EkeyFetchError::ParseError(format!("Failed to read QQ Music plist: {}", e))
    })?;

    let plist_dict = plist_value.as_dictionary().ok_or_else(|| {
        EkeyFetchError::ParseError("QQ Music plist is not a dictionary".to_string())
    })?;

    let auto_login_data = plist_dict.get("AutoLoginUserInfo").ok_or_else(|| {
        EkeyFetchError::NoCredentials(
            "AutoLoginUserInfo not found in QQ Music plist — is QQ Music logged in?".to_string(),
        )
    })?;

    // AutoLoginUserInfo is stored as a Data (bytes) blob containing an NSKeyedArchiver plist
    let data = auto_login_data.as_data().ok_or_else(|| {
        EkeyFetchError::ParseError("AutoLoginUserInfo is not a Data value".to_string())
    })?;

    parse_nskeyed_archiver_credentials(data)
}

// ============================================================
// Windows: read credentials from QQ Music's process memory
// ============================================================

#[cfg(target_os = "windows")]
pub fn get_qqmusic_credentials() -> Result<QQMusicCredentials, EkeyFetchError> {
    use std::path::Path;

    // Windows API types & constants
    type HANDLE = isize;
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const PROCESS_VM_READ: u32 = 0x0010;
    const TH32CS_SNAPPROCESS: u32 = 0x0002;
    const MEM_COMMIT: u32 = 0x1000;
    const MEM_PRIVATE: u32 = 0x20000;
    const MEM_IMAGE: u32 = 0x1000000;
    const PAGE_READWRITE: u32 = 0x04;
    const PAGE_READONLY: u32 = 0x02;
    const PAGE_WRITECOPY: u32 = 0x08;
    const PAGE_EXECUTE_READWRITE: u32 = 0x40;
    const PAGE_EXECUTE_READ: u32 = 0x20;
    const PAGE_GUARD: u32 = 0x100;
    const INVALID_HANDLE_VALUE: isize = -1;

    /// Windows 10 1803+ adds PartitionId between AllocationProtect and RegionSize.
    /// Without it, VirtualQueryEx writes to wrong offsets on Win 10 1803+ / Win 11.
    /// Rust's #[repr(C)] adds the natural C padding automatically (2 bytes after u16
    /// to align usize to 8), so we only list the fields in declaration order.
    #[repr(C)]
    #[allow(non_snake_case, dead_code)]
    struct MEMORY_BASIC_INFORMATION {
        base_address: *mut std::ffi::c_void,
        allocation_base: *mut std::ffi::c_void,
        allocation_protect: u32,
        partition_id: u16, // Windows 10 1803+ — must be present on Win 11
        region_size: usize,
        state: u32,
        protect: u32,
        _type: u32,
    }

    impl MEMORY_BASIC_INFORMATION {
        /// Returns true if this region should be scanned for authst.
        /// We want committed, readable, nonguard memory.
        fn is_scannable(&self) -> bool {
            if self.state != MEM_COMMIT {
                return false;
            }
            if self.protect & PAGE_GUARD != 0 {
                return false;
            }
            let rw = self.protect
                & (PAGE_READONLY | PAGE_READWRITE | PAGE_WRITECOPY
                   | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE);
            rw != 0 && self.region_size > 0
        }
    }

    #[repr(C)]
    struct PROCESSENTRY32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    extern "system" {
        fn OpenProcess(
            dwDesiredAccess: u32,
            bInheritHandle: i32,
            dwProcessId: u32,
        ) -> HANDLE;
        fn CloseHandle(hObject: HANDLE) -> i32;
        fn ReadProcessMemory(
            hProcess: HANDLE,
            lpBaseAddress: *const std::ffi::c_void,
            lpBuffer: *mut std::ffi::c_void,
            nSize: usize,
            lpNumberOfBytesRead: *mut usize,
        ) -> i32;
        fn VirtualQueryEx(
            hProcess: HANDLE,
            lpAddress: *const std::ffi::c_void,
            lpBuffer: *mut MEMORY_BASIC_INFORMATION,
            dwLength: usize,
        ) -> usize;
        fn CreateToolhelp32Snapshot(
            dwFlags: u32,
            th32ProcessID: u32,
        ) -> HANDLE;
        fn Process32FirstW(
            hSnapshot: HANDLE,
            lppe: *mut PROCESSENTRY32W,
        ) -> i32;
        fn Process32NextW(
            hSnapshot: HANDLE,
            lppe: *mut PROCESSENTRY32W,
        ) -> i32;
    }

    const CHUNK_SIZE: usize = 1 * 1024 * 1024; // 1 MB per read

    // ----- read UIN from config -----
    let read_uin = || -> Result<String, EkeyFetchError> {
        let appdata = std::env::var("APPDATA").map_err(|_| {
            EkeyFetchError::NoCredentials("APPDATA environment variable not found".to_string())
        })?;
        let config_path = Path::new(&appdata)
            .join("Tencent")
            .join("QQMusic")
            .join("QQMusicServiceConfig.ini");

        if !config_path.exists() {
            return Err(EkeyFetchError::NoCredentials(format!(
                "QQ Music config not found at {}. Is QQ Music installed?",
                config_path.display()
            )));
        }

        let content = std::fs::read_to_string(&config_path).map_err(|e| {
            EkeyFetchError::ParseError(format!("Failed to read QQ Music config: {}", e))
        })?;

        for line in content.lines() {
            let t = line.trim();
            if let Some(val) = t
                .strip_prefix("Uin=")
                .or_else(|| t.strip_prefix("uin="))
                .or_else(|| t.strip_prefix("UIN="))
            {
                let uin = val.trim().to_string();
                if !uin.is_empty() && uin != "0" {
                    return Ok(uin);
                }
            }
        }
        Err(EkeyFetchError::NoCredentials(
            "UIN not found in QQMusicServiceConfig.ini".to_string(),
        ))
    };

    // ----- find QQMusic.exe processes -----
    let find_qqmusic_pids = || -> Result<Vec<u32>, EkeyFetchError> {
        let mut pids = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return Err(EkeyFetchError::NoCredentials(
                    "Failed to create process snapshot".to_string(),
                ));
            }

            let mut entry = PROCESSENTRY32W {
                dw_size: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                cnt_usage: 0,
                th32_process_id: 0,
                th32_default_heap_id: 0,
                th32_module_id: 0,
                cnt_threads: 0,
                th32_parent_process_id: 0,
                pc_pri_class_base: 0,
                dw_flags: 0,
                sz_exe_file: [0; 260],
            };

            if Process32FirstW(snap, &mut entry) == 0 {
                CloseHandle(snap);
                return Err(EkeyFetchError::NoCredentials(
                    "Failed to enumerate processes".to_string(),
                ));
            }

            loop {
                let exe_path = String::from_utf16_lossy(&entry.sz_exe_file)
                    .trim_end_matches('\0')
                    .to_string();
                if Path::new(&exe_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map_or(false, |n| {
                        n.eq_ignore_ascii_case("QQMusic.exe")
                            || n.eq_ignore_ascii_case("WeChatAppEx.exe")
                            || n.eq_ignore_ascii_case("qmbrowser.exe")
                    }) {
                    pids.push(entry.th32_process_id);
                }
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
            CloseHandle(snap);
        }

        if pids.is_empty() {
            Err(EkeyFetchError::NoCredentials(
                "QQMusic.exe is not running. Please start QQ Music and log in.".to_string(),
            ))
        } else {
            Ok(pids)
        }
    };

    // ----- memory searching helpers -----

    /// Search for the authst value by finding `"authst":"` JSON key then
    /// extracting the string between the quotes.
    fn extract_authst_from_json(data: &[u8]) -> Option<String> {
        let markers: &[&[u8]] = &[b"\"authst\":\"", b"\"authst\": \""];
        for marker in markers {
            for (i, win) in data.windows(marker.len()).enumerate() {
                if win == *marker {
                    let start = i + marker.len();
                    let slice = &data[start..];
                    let mut end = 0;
                    for (j, &b) in slice.iter().enumerate() {
                        if b == b'"' && (j == 0 || slice[j - 1] != b'\\') {
                            end = j;
                            break;
                        }
                    }
                    if end >= 10 {
                        if let Ok(s) = std::str::from_utf8(&slice[..end]) {
                            if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '-' || c == '_' || c == '=') {
                                return Some(s.to_string());
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Find all base64 strings in the data, returning the best authst candidate.
    /// Prioritises strings ending with `=` / `==` (most authst tokens have padding).
    fn is_b64_char(b: u8) -> bool {
        // Standard base64 only — base64url (- and _) matches too many C symbols
        b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='
    }
    fn find_best_authst(data: &[u8]) -> Option<String> {
        let mut start: Option<usize> = None;
        let mut best: Option<(usize, usize, bool)> = None; // (start, len, has_padding)

        let flush = |s: usize, e: usize, best: &mut Option<(usize, usize, bool)>| {
            let len = e - s;
            if len < 20 {
                return;
            }
            // Must be valid UTF-8
            if std::str::from_utf8(&data[s..e]).is_err() {
                return;
            }
            let ends_with_eq = data[e - 1] == b'=';
            let better = match best.as_ref().map(|x| (x.1, x.2)) {
                Some((bl, bp)) => {
                    if ends_with_eq && !bp {
                        true
                    } else if !ends_with_eq && bp {
                        false
                    } else {
                        len > bl
                    }
                }
                None => true,
            };
            if better {
                *best = Some((s, len, ends_with_eq));
            }
        };

        for (i, &b) in data.iter().enumerate() {
            if is_b64_char(b) {
                if start.is_none() {
                    start = Some(i);
                }
            } else if let Some(s) = start.take() {
                flush(s, i, &mut best);
            }
        }
        if let Some(s) = start.take() {
            flush(s, data.len(), &mut best);
        }

        best.map(|(s, len, _)| String::from_utf8_lossy(&data[s..s + len]).to_string())
    }

    /// Windows: find the longest UTF-16LE encoded base64 string (40+ raw bytes → 20+ chars).
    /// On Windows, the authst may be stored as `std::wstring` (wchar_t), where each
    /// base64 character is followed by a `\x00` high byte in little-endian layout.
    #[allow(dead_code)]
    fn find_best_authst_utf16(data: &[u8]) -> Option<String> {
        let mut start: Option<usize> = None;
        let mut best: Option<(usize, Vec<u8>, bool)> = None; // (start, utf8_bytes, has_padding)

        // Helper: non-consuming comparison
        let better = |b: &Option<(usize, Vec<u8>, bool)>, len: usize, padded: bool| -> bool {
            match b {
                None => true,
                Some((_, ref prev, bp)) => {
                    if padded && !bp {
                        true
                    } else if !padded && *bp {
                        false
                    } else {
                        len > prev.len()
                    }
                }
            }
        };

        let n = data.len().saturating_sub(1);
        let mut i = 0;
        while i < n {
            let b = data[i];
            let is_b64 = is_b64_char(b);
            if is_b64 && data[i + 1] == 0x00 {
                if start.is_none() {
                    start = Some(i);
                }
                i += 2;
            } else {
                if let Some(s) = start.take() {
                    let raw_len = i - s;
                    let char_count = raw_len / 2;
                    if char_count >= 10 {
                        let utf8: Vec<u8> = data[s..i].iter().step_by(2).copied().collect();
                        let padded = utf8.last() == Some(&b'=');
                        if better(&best, utf8.len(), padded) {
                            best = Some((s, utf8, padded));
                        }
                    }
                }
                i += 1;
            }
        }
        if let Some(s) = start.take() {
            let raw_len = data.len() - s;
            let char_count = raw_len / 2;
            if char_count >= 10 && (data.len() - s) % 2 == 0 {
                let utf8: Vec<u8> = data[s..].iter().step_by(2).copied().collect();
                let padded = utf8.last() == Some(&b'=');
                if better(&best, utf8.len(), padded) {
                    best = Some((s, utf8, padded));
                }
            }
        }

        best.map(|(_, utf8, _)| String::from_utf8_lossy(&utf8).to_string())
    }

    // ----- scan one process -----
    let scan_process = |pid: u32, label: &str| -> Result<String, EkeyFetchError> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
            if h == 0 {
                return Err(EkeyFetchError::NoCredentials(format!(
                    "Cannot open PID {} ({}). Try running as administrator.",
                    pid, label
                )));
            }

            let mut addr: isize = 0;

            loop {
                let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                let ret = VirtualQueryEx(
                    h,
                    addr as *const std::ffi::c_void,
                    &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                );
                if ret == 0 {
                    break; // past end of address space
                }

                if mbi.is_scannable() {
                    // Skip mapped files (MEM_MAPPED) — file-backed views unlikely
                    // to contain the authst token. Scan MEM_PRIVATE (heap/stack)
                    // and MEM_IMAGE (DLL sections).
                    if mbi._type != MEM_PRIVATE && mbi._type != MEM_IMAGE {
                        addr = (addr as usize).wrapping_add(mbi.region_size) as isize;
                        continue;
                    }

                    let mut offset: usize = 0;
                    while offset < mbi.region_size {
                        let chunk = std::cmp::min(CHUNK_SIZE, mbi.region_size - offset);
                        let mut buf = vec![0u8; chunk];
                        let mut read: usize = 0;

                        let ok = ReadProcessMemory(
                            h,
                            (addr as usize + offset) as *const std::ffi::c_void,
                            buf.as_mut_ptr() as *mut std::ffi::c_void,
                            chunk,
                            &mut read,
                        );

                        if ok != 0 && read > 0 {
                            let region = &buf[..read];

                            // Strategy 1: JSON key pattern — fastest and most reliable
                            if let Some(authst) = extract_authst_from_json(region) {
                                CloseHandle(h);
                                return Ok(authst);
                            }
                        }
                        offset += chunk;
                    }
                }

                // Next region (wrapping_add saturates on overflow)
                addr = (addr as usize).wrapping_add(mbi.region_size) as isize;
            }
            CloseHandle(h);
        }

        Err(EkeyFetchError::NoCredentials(format!(
            "authst not found in {} (PID {})",
            label, pid
        )))
    };

    // ----- secondary: search for authst in SetCookie/_SetCookie data files -----
    let scan_cookie_files = || -> Result<String, EkeyFetchError> {
        let appdata = std::env::var("APPDATA").map_err(|_| {
            EkeyFetchError::NoCredentials("APPDATA env var not found".to_string())
        })?;
        let base = Path::new(&appdata).join("Tencent").join("QQMusic");

        // Try each cookie file
        for filename in &["SetCookie.dat", "_SetCookie.dat"] {
            let path = base.join(filename);
            if !path.exists() {
                continue;
            }
            let data = match std::fs::read(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            // Strategy 1: scan for JSON `"authst":"..."` pattern
            if let Some(authst) = extract_authst_from_json(&data) {
                return Ok(authst);
            }
            // Strategy 2: scan for base64 strings ≥ 30 chars with padding
            if let Some(authst) = find_best_authst(&data) {
                let has_padding = authst.ends_with('=');
                let uniform = authst.bytes().all(|c| c == authst.as_bytes()[0]);
                if has_padding && !uniform && authst.len() >= 30 {
                    return Ok(authst);
                }
            }
        }
        Err(EkeyFetchError::NoCredentials(
            "authst not found in cookie files".to_string(),
        ))
    };

    // ----- main flow: try file-based first, then process memory -----
    let uin = read_uin()?;

    // Try SetCookie.dat / _SetCookie.dat first
    if let Ok(authst) = scan_cookie_files() {
        return Ok(QQMusicCredentials { uin, authst });
    }

    // Fall back: process memory scanning
    let pids = find_qqmusic_pids()?;

    for pid in &pids {
        // Get process name for error messages
        let label = String::from_utf8_lossy(&{
            let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
            let mut name = Vec::new();
            if snap != INVALID_HANDLE_VALUE {
                let mut entry = PROCESSENTRY32W {
                    dw_size: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                    cnt_usage: 0,
                    th32_process_id: 0,
                    th32_default_heap_id: 0,
                    th32_module_id: 0,
                    cnt_threads: 0,
                    th32_parent_process_id: 0,
                    pc_pri_class_base: 0,
                    dw_flags: 0,
                    sz_exe_file: [0; 260],
                };
                if unsafe { Process32FirstW(snap, &mut entry) } != 0 {
                    loop {
                        if entry.th32_process_id == *pid {
                            let s = String::from_utf16_lossy(&entry.sz_exe_file)
                                .trim_end_matches('\0')
                                .to_string();
                            let fname = Path::new(&s).file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(&s)
                                .to_string();
                            name = fname.into_bytes();
                            break;
                        }
                        if unsafe { Process32NextW(snap, &mut entry) } == 0 {
                            break;
                        }
                    }
                }
                unsafe { CloseHandle(snap); }
            }
            name
        }).to_string();

        if let Ok(authst) = scan_process(*pid, &label) {
            return Ok(QQMusicCredentials { uin, authst });
        }
    }

    Err(EkeyFetchError::NoCredentials(
        "Could not find authst in any QQ Music process memory. \
         Make sure QQ Music is running and logged in, \
         then try again. If the issue persists, try running as administrator.".to_string(),
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn get_qqmusic_credentials() -> Result<QQMusicCredentials, EkeyFetchError> {
    Err(EkeyFetchError::NoCredentials(
        "auto-fetch ekey is only supported on macOS and Windows".to_string(),
    ))
}

/// Call the QQ Music GetEVkey API to fetch the ekey for a song.
///
/// Requires valid credentials and the musicex metadata (filename and songmid).
/// The filename should include the `.mgg` or `.mflac` extension.
pub async fn call_get_evkey_api(
    creds: &QQMusicCredentials,
    filename: &str,
    songmid: &str,
) -> Result<String, EkeyFetchError> {
    let request_body = MusicuRequest {
        comm: MusicuComm {
            authst: creds.authst.clone(),
            ct: "19",
            cv: "1859",
            uin: creds.uin.clone(),
            tme_login_type: "3",
        },
        req: MusicuReq1 {
            module: "music.vkey.GetEVkey",
            method: "CgiGetEVkey",
            param: MusicuParam {
                filename: vec![filename.to_string()],
                guid: "10000",
                songmid: vec![songmid.to_string()],
                songtype: vec![1],
                uin: creds.uin.clone(),
                loginflag: 1,
                platform: QQ_API_PLATFORM,
                ctx: 1,
            },
        },
    };

    let client = reqwest::Client::new();
    let resp = client
        .post("https://u.y.qq.com/cgi-bin/musicu.fcg")
        .header("Content-Type", "application/json")
        .header("User-Agent", QQ_API_USER_AGENT)
        .header("Referer", "https://y.qq.com/")
        .json(&request_body)
        .send()
        .await
        .map_err(|e| EkeyFetchError::NetworkError(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(EkeyFetchError::NetworkError(format!(
            "HTTP {} from QQ Music API",
            resp.status()
        )));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| EkeyFetchError::NetworkError(e.to_string()))?;
    if std::env::var("QMC_DEBUG_API").is_ok() {
        eprintln!("[GetEVkey raw response] {}", text);
    }
    let response: MusicuResponse = serde_json::from_str(&text)
        .map_err(|e| EkeyFetchError::ParseError(format!("Failed to parse API response: {}", e)))?;

    let req_data = response.req.ok_or_else(|| {
        EkeyFetchError::ApiError {
            code: -1,
            message: "Missing req_1 in API response".to_string(),
        }
    })?;

    if req_data.code.unwrap_or(-1) != 0 {
        return Err(EkeyFetchError::ApiError {
            code: req_data.code.unwrap_or(-1),
            message: "Top-level API request failed".to_string(),
        });
    }

    let data = req_data.data.ok_or_else(|| EkeyFetchError::ApiError {
        code: -1,
        message: "Missing data in API response".to_string(),
    })?;

    let midurlinfo = data.midurlinfo.ok_or_else(|| EkeyFetchError::ApiError {
        code: -1,
        message: "Missing midurlinfo in API response".to_string(),
    })?;

    if midurlinfo.is_empty() {
        return Err(EkeyFetchError::ApiError {
            code: -1,
            message: "midurlinfo is empty in API response".to_string(),
        });
    }

    let info = &midurlinfo[0];
    let result_code = info.result.unwrap_or(-1);

    // Check for error result codes
    if result_code != 0 {
        return Err(EkeyFetchError::EmptyEkey { result_code });
    }

    let ekey = info
        .ekey
        .as_deref()
        .unwrap_or("")
        .to_string();

    if ekey.is_empty() {
        return Err(EkeyFetchError::EmptyEkey { result_code: 0 });
    }

    Ok(ekey)
}

/// Fetch the ekey for a file by parsing its musicex footer and calling the API.
///
/// This is the main entry point for ekey auto-fetching. It:
/// 1. Reads the file and parses the musicex footer
/// 2. Finds QQ Music credentials from the local QQ Music client
/// 3. Calls the GetEVkey API
/// 4. Returns the ekey string
pub async fn fetch_ekey(input_path: &Path) -> Result<String, EkeyFetchError> {
    let data = std::fs::read(input_path)
        .map_err(|e| EkeyFetchError::ParseError(format!("Failed to read file: {}", e)))?;

    let info = match parse_musicex_footer(&data) {
        Ok(info) => info,
        Err(EkeyFetchError::NoMusicexFooter) => {
            // Not musicex: try the STag footer (no ekey embedded either)
            let stag = parse_stag_footer(&data)?;
            eprintln!(
                "STag footer: song_id={}, media_mid={}",
                stag.song_id, stag.media_mid
            );
            resolve_stag_via_local_library(&stag)?
        }
        Err(e) => return Err(e),
    };

    eprintln!(
        "Musicex footer: song_id={}, media_mid={}, filename={}",
        info.song_id, info.media_mid, info.filename
    );

    // Ensure filename has the proper extension for the API
    let api_filename = if !info.filename.ends_with(".mgg")
        && !info.filename.ends_with(".mgg0")
        && !info.filename.ends_with(".mgg1")
        && !info.filename.ends_with(".mggl")
        && !info.filename.ends_with(".mflac")
        && !info.filename.ends_with(".mflac0")
        && !info.filename.ends_with(".mflach")
    {
        // The filename from the footer might not have an extension;
        // we need to add one based on the file extension
        let ext = input_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mgg");
        format!("{}.{}", info.filename, ext)
    } else {
        info.filename.clone()
    };

    let creds = get_qqmusic_credentials()?;
    eprintln!("Found QQ Music credentials (uin={})", creds.uin);

    // The GetEVkey API returns a *different* ekey for the same media_mid
    // depending on the filename prefix (media quality tier). The local
    // library copy may carry a prefix for a different quality than the
    // file we are decrypting, so try candidate prefixes in turn and keep
    // the first ekey whose decryption yields a plausible audio header.
    let candidates = candidate_filenames(&api_filename);
    let mut first_ok: Option<String> = None;
    let mut last_err: Option<EkeyFetchError> = None;

    for fname in &candidates {
        eprintln!("Fetching ekey from QQ Music API for {}...", fname);
        match call_get_evkey_api(&creds, fname, &info.media_mid).await {
            Ok(ekey) => {
                if first_ok.is_none() {
                    first_ok = Some(ekey.clone());
                }
                if magic_check_ekey(&ekey, &data) {
                    eprintln!(
                        "Successfully obtained ekey ({} chars), verified via {}",
                        ekey.len(),
                        fname
                    );
                    return Ok(ekey);
                }
                eprintln!(
                    "ekey from {} failed audio-header check, trying next prefix...",
                    fname
                );
            }
            Err(e) => {
                eprintln!("API error for {}: {}", fname, e);
                last_err = Some(e);
            }
        }
    }

    // No candidate passed the header check: fall back to the first ekey we
    // managed to obtain (previous behaviour) instead of failing outright.
    if let Some(ekey) = first_ok {
        eprintln!(
            "warning: no candidate ekey passed the audio-header check; \
             falling back to the first obtained ekey ({} chars)",
            ekey.len()
        );
        return Ok(ekey);
    }
    Err(last_err.unwrap_or_else(|| {
        EkeyFetchError::ParseError("No ekey candidates could be fetched".to_string())
    }))
}

/// Quality-tier prefixes used by QQ Music media filenames.
const MEDIA_PREFIXES: [&str; 10] = [
    "O4M0", "O8M0", "O9M0", "O6M0", "O7M0", "M800", "M500", "F0M0", "Q0M0", "F000",
];

/// Build the list of candidate API filenames: the default one first,
/// then the same media_mid/ext with each alternative quality prefix.
fn candidate_filenames(default: &str) -> Vec<String> {
    let mut list = vec![default.to_string()];
    if default.len() > 4 {
        let rest = &default[4..];
        for p in MEDIA_PREFIXES {
            let cand = format!("{}{}", p, rest);
            if !list.contains(&cand) {
                list.push(cand);
            }
        }
    }
    list
}

/// Decrypt the first 4 bytes of the ciphertext with `ekey` and check
/// whether the plaintext looks like a known audio container.
fn magic_check_ekey(ekey: &str, data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    let Ok(crypto) = Qmc2Crypto::from_ekey(ekey) else {
        return false;
    };
    let mut head = data[..4].to_vec();
    crypto.decrypt(0, &mut head);
    crate::looks_like_audio(&head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_filenames_rotates_prefix() {
        let c = candidate_filenames("O4M00010D3os1MQxyZ.mgg");
        assert_eq!(c[0], "O4M00010D3os1MQxyZ.mgg");
        assert!(c.contains(&"O8M00010D3os1MQxyZ.mgg".to_string()));
        assert!(c.contains(&"M5000010D3os1MQxyZ.mgg".to_string()));
        // no duplicates, default first
        let mut dedup = c.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), c.len());
    }

    #[test]
    fn test_candidate_filenames_short_name() {
        // Names shorter than a prefix are returned unchanged (no panic).
        assert_eq!(candidate_filenames("abc"), vec!["abc".to_string()]);
    }

    #[test]
    fn test_magic_check_roundtrip() {
        // QMC2 ciphers are XOR streams: encrypting "fLaC" with a key and
        // checking it back with the same key must pass; a different key
        // must (deterministically) fail.
        let ekey = "MTIzNDU2Nzg5MGFiY2RlZjEyMzQ1Njc4OTA=";
        let crypto = Qmc2Crypto::from_ekey(ekey).unwrap();
        let mut cipher = b"fLaC".to_vec();
        crypto.decrypt(0, &mut cipher); // XOR stream => encrypt
        assert_ne!(&cipher[..], b"fLaC");
        assert!(magic_check_ekey(ekey, &cipher));
        let other = "YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXo=";
        assert!(!magic_check_ekey(other, &cipher));
    }

    #[test]
    fn test_magic_check_rejects_garbage_ciphertext() {
        let ekey = "MTIzNDU2Nzg5MGFiY2RlZjEyMzQ1Njc4OTA=";
        // Random-looking ciphertext must not pass as an audio header.
        assert!(!magic_check_ekey(ekey, b"\x19\x0b\x69\x74rest-of-ciphertext"));
    }

    #[test]
    fn test_read_utf16_le_string() {
        // "Hello" in UTF-16LE with null terminator
        let data: Vec<u8> = vec![
            0x48, 0x00, 0x65, 0x00, 0x6C, 0x00, 0x6C, 0x00, 0x6F, 0x00, 0x00, 0x00,
        ];
        let result = read_utf16_le_string(&data, 0, data.len());
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_read_utf16_le_string_with_padding() {
        // "Hi" in UTF-16LE followed by null padding
        let data: Vec<u8> = vec![
            0x48, 0x00, 0x69, 0x00, // "Hi"
            0x00, 0x00, // null terminator
            0x41, 0x00, // "A" (should not be read)
        ];
        let result = read_utf16_le_string(&data, 0, 4);
        assert_eq!(result, "Hi");
    }

    #[test]
    fn test_parse_musicex_footer_invalid() {
        // Too short
        assert!(matches!(
            parse_musicex_footer(&[0u8; 8]),
            Err(EkeyFetchError::NoMusicexFooter)
        ));

        // Not musicex
        let data = vec![0u8; 256];
        assert!(matches!(
            parse_musicex_footer(&data),
            Err(EkeyFetchError::NoMusicexFooter)
        ));
    }

    #[test]
    fn test_parse_musicex_footer_valid() {
        // Build a valid musicex footer matching the real format.
        // The footer is a contiguous block of footer_size bytes at the end of the file:
        //   [metadata content (footer_size - 16 bytes)] [footer_size (4B)] [version (4B)] [magic (8B)]
        // footer_size = 0xC0 = 192, so metadata content = 176 bytes (0xB0)

        let song_id: u32 = 123456789u32;
        let media_mid = "003aBcDeFgHiJk";
        let filename = "M800003aBcDeFgHiJk.mgg";
        let footer_size: u32 = 0xC0; // 192
        let metadata_content_size = (footer_size as usize) - 16; // 176 = 0xB0

        // Build the metadata content section (176 bytes)
        let mut meta = Vec::new();

        // +0x00: song_id (4 bytes LE)
        meta.extend_from_slice(&song_id.to_le_bytes());
        // +0x04: quality_type1 (4 bytes)
        meta.extend_from_slice(&2u32.to_le_bytes());
        // +0x08: quality_type2 (4 bytes)
        meta.extend_from_slice(&2u32.to_le_bytes());

        // +0x0C: media_mid (60 bytes UTF-16LE, null padded)
        let mut mid_buf = vec![0u8; 60];
        for (i, ch) in media_mid.encode_utf16().enumerate() {
            let offset = i * 2;
            if offset + 1 < 60 {
                mid_buf[offset] = (ch & 0xFF) as u8;
                mid_buf[offset + 1] = (ch >> 8) as u8;
            }
        }
        meta.extend_from_slice(&mid_buf);

        // +0x48: filename (68 bytes UTF-16LE, null padded)
        let mut fname_buf = vec![0u8; 68];
        for (i, ch) in filename.encode_utf16().enumerate() {
            let offset = i * 2;
            if offset + 1 < 68 {
                fname_buf[offset] = (ch & 0xFF) as u8;
                fname_buf[offset + 1] = (ch >> 8) as u8;
            }
        }
        meta.extend_from_slice(&fname_buf);

        // Pad metadata content to 176 bytes (0xB0)
        assert!(meta.len() <= metadata_content_size, "metadata content exceeds expected size");
        meta.extend_from_slice(&vec![0u8; metadata_content_size - meta.len()]);
        assert_eq!(meta.len(), metadata_content_size);

        // Build the complete file: [fake audio data] [metadata content] [footer_size] [version] [magic]
        let mut file_data = Vec::new();
        file_data.extend_from_slice(&[0xAAu8; 100]); // fake audio data
        file_data.extend_from_slice(&meta);
        file_data.extend_from_slice(&footer_size.to_le_bytes()); // footer_size
        file_data.extend_from_slice(&1u32.to_le_bytes()); // version
        file_data.extend_from_slice(b"musicex\x00"); // magic

        let info = parse_musicex_footer(&file_data).unwrap();
        assert_eq!(info.song_id, 123456789);
        assert_eq!(info.media_mid, "003aBcDeFgHiJk");
        assert_eq!(info.filename, "M800003aBcDeFgHiJk.mgg");
    }
}
