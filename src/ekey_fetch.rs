//! Ekey auto-fetch module
//!
//! Automatically fetches the ekey from QQ Music's API for musicex-format files.
//! This requires:
//! 1. The file to have a musicex footer (so we can extract media_mid and filename)
//! 2. QQ Music to be logged in on this Mac (so we can read auth credentials)
//! 3. Network access to u.y.qq.com

use plist::Value;
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::Path;

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
                if *result_code == 104005 {
                    write!(f, "API returned empty ekey (result=104005). This usually means: \
                        (1) your QQ Music login has expired — try re-opening the app, or \
                        (2) the song requires VIP access — ensure your account has an active subscription")
                } else {
                    write!(f, "API returned empty ekey (result={}). The song may require VIP access or the auth token may have expired", result_code)
                }
            }
            EkeyFetchError::NetworkError(msg) => {
                write!(f, "Network error: {}", msg)
            }
            EkeyFetchError::ParseError(msg) => {
                write!(f, "Parse error: {}", msg)
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
// NSKeyedArchiver parsing
// ============================================================

/// Minimal NSKeyedArchiver parser for extracting QQ Music credentials.
///
/// The `AutoLoginUserInfo` in the QQ Music plist is encoded as an NSKeyedArchiver
/// binary plist. We need to resolve UID references in the `$objects` array to
/// extract `strUserAccount` (UIN) and `strAuthst` (auth key).
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
                platform: "20",
                ctx: 1,
            },
        },
    };

    let client = reqwest::Client::new();
    let resp = client
        .post("https://u.y.qq.com/cgi-bin/musicu.fcg")
        .header("Content-Type", "application/json")
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")
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

    let response: MusicuResponse = resp
        .json()
        .await
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
/// 2. Finds QQ Music credentials on this Mac
/// 3. Calls the GetEVkey API
/// 4. Returns the ekey string
pub async fn fetch_ekey(input_path: &Path) -> Result<String, EkeyFetchError> {
    let data = std::fs::read(input_path)
        .map_err(|e| EkeyFetchError::ParseError(format!("Failed to read file: {}", e)))?;

    let info = parse_musicex_footer(&data)?;

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

    eprintln!("Fetching ekey from QQ Music API for {}...", api_filename);

    let creds = get_qqmusic_credentials()?;
    eprintln!("Found QQ Music credentials (uin={})", creds.uin);

    let ekey = call_get_evkey_api(&creds, &api_filename, &info.media_mid).await?;

    eprintln!("Successfully obtained ekey ({} chars)", ekey.len());

    Ok(ekey)
}

#[cfg(test)]
mod tests {
    use super::*;

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
