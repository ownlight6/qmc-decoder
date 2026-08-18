//! Tauri commands wrapping the `qmc_decoder` core library.
//!
//! The user interface lives in the web frontend; everything heavy (file IO,
//! decryption, ekey fetching, native dialogs) runs on Rust side in blocking
//! threads so the UI never freezes.

use std::path::{Path, PathBuf};

use qmc_decoder::{decrypt_file, fetch_ekey, get_qqmusic_credentials, info_file};
use qmc_decoder::{determine_output_path, Format, FooterInfo};
use serde::Serialize;
use tauri::Emitter;

/// Result of processing one file, shown in the frontend results list.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    pub input_path: String,
    pub output_path: Option<String>,
    pub success: bool,
    pub message: String,
    pub format: Option<String>,
    pub decrypted_bytes: Option<usize>,
}

impl FileResult {
    fn ok(input: &Path, output: &Path, format: Format, bytes: usize) -> Self {
        FileResult {
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            success: true,
            message: format!("已解密，输出 {} 字节", bytes),
            format: Some(format!("{:?}", format)),
            decrypted_bytes: Some(bytes),
        }
    }

    fn err(input: &Path, message: impl Into<String>) -> Self {
        FileResult {
            input_path: input.display().to_string(),
            output_path: None,
            success: false,
            message: message.into(),
            format: None,
            decrypted_bytes: None,
        }
    }
}

/// Decrypt a list of files and/or directories, returning one result per file.
#[tauri::command]
pub async fn decrypt_paths(
    app: tauri::AppHandle,
    paths: Vec<String>,
    output_dir: Option<String>,
    ekey: Option<String>,
    fetch_ekey: bool,
) -> Result<Vec<FileResult>, String> {
    let ekey = ekey.filter(|s| !s.trim().is_empty());
    let output_dir = output_dir.filter(|s| !s.trim().is_empty());

    tauri::async_runtime::spawn_blocking(move || {
        // Count how many files will be processed so the frontend can show a
        // determinate progress bar (directories expand into child files).
        let mut total: usize = 0;
        let mut count_ok = true;
        for p in &paths {
            let input = PathBuf::from(p);
            if input.is_dir() {
                total += count_supported(&input);
            } else if input.exists() {
                total += 1;
            } else {
                count_ok = false;
            }
        }
        if !count_ok {
            // Fall back to the number of explicit inputs.
            total = paths.len();
        }
        let _ = app.emit("decrypt-started", total);
        // Ensure the output directory exists up-front so it doesn't fail per file.
        if let Some(dir) = output_dir.as_deref() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("无法创建输出目录 {}: {}", dir, e))?;
        }
        let output_dir = output_dir.as_deref().map(Path::new);

        let mut results = Vec::new();
        let mut done: usize = 0;
        for p in paths {
            let input = PathBuf::from(&p);
            if !input.exists() {
                done += 1;
                emit_progress(&app, done);
                results.push(FileResult::err(&input, "路径不存在"));
                continue;
            }
            if input.is_dir() {
                match process_dir(&input, output_dir, ekey.as_deref(), fetch_ekey, &app, &mut done) {
                    Ok(mut rows) => results.append(&mut rows),
                    Err(msg) => {
                        done += 1;
                        emit_progress(&app, done);
                        results.push(FileResult::err(&input, msg));
                    }
                }
                continue;
            }
            match decrypt_single(&input, output_dir, ekey.as_deref(), fetch_ekey) {
                Ok(r) => results.push(r),
                Err((input, msg)) => results.push(FileResult::err(&input, msg)),
            }
            done += 1;
            emit_progress(&app, done);
        }
        Ok(results)
    })
    .await
    .map_err(|e| format!("后台任务错误: {}", e))?
}

/// Decrypt every supported file inside a directory.
/// Reports per-file progress through the same `decrypt-*` events.
fn process_dir(
    input: &Path,
    output_dir: Option<&Path>,
    ekey: Option<&str>,
    fetch_ekey: bool,
    app: &tauri::AppHandle,
    done: &mut usize,
) -> Result<Vec<FileResult>, String> {
    // Recursively collect the supported files (QQ 下载目录里歌曲常按歌手/专辑
    // 分层存放，只取一层会漏歌）。不可读的目录跳过，深度有上限防邪树/链接环。
    let mut files = Vec::new();
    collect_supported(input, &mut files, 0);
    if files.is_empty() {
        return Err("目录中没有找到支持的加密文件".to_string());
    }

    let mut results = Vec::new();
    for path in files {
        match decrypt_single(&path, output_dir, ekey, fetch_ekey) {
            Ok(r) => results.push(r),
            Err((input, msg)) => results.push(FileResult::err(&input, msg)),
        }
        *done += 1;
        emit_progress(app, *done);
    }
    Ok(results)
}

/// Recursively gather supported (decryptable) file paths under `input`.
/// `depth` guards against runaway trees / symlink cycles.
fn collect_supported(input: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 10 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(input) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_supported(&path, out, depth + 1);
        } else if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if Format::from_extension(ext).is_some() {
                out.push(path);
            }
        }
    }
}

/// Number of supported files inside a directory (recursively).
fn count_supported(input: &Path) -> usize {
    let mut files = Vec::new();
    collect_supported(input, &mut files, 0);
    files.len()
}

fn emit_progress(app: &tauri::AppHandle, done: usize) {
    let _ = app.emit("decrypt-progress", done);
}

/// Decrypt a single file, returning the path and error message on failure.
fn decrypt_single(
    input: &Path,
    output_dir: Option<&Path>,
    ekey: Option<&str>,
    fetch_ekey: bool,
) -> Result<FileResult, (PathBuf, String)> {
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    let format = Format::from_extension(ext)
        .ok_or_else(|| (input.to_path_buf(), format!("不支持的文件格式 '{}'", ext)))?;

    let output = match output_dir {
        Some(dir) => determine_output_path(input, Some(dir), format),
        None => determine_output_path(input, None, format),
    };

    match decrypt_file(input, &output, format, ekey, fetch_ekey) {
        Ok(dec) => Ok(FileResult::ok(
            &dec.input_path,
            &dec.output_path,
            dec.format,
            dec.decrypted_bytes,
        )),
        Err(e) => Err((input.to_path_buf(), e)),
    }
}

/// Metadata for a single file (format, footer type, embedded metadata), without decrypting.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub path: String,
    pub size: usize,
    pub format: String,
    pub footer: String,
    pub details: Vec<(String, String)>,
}

#[tauri::command]
pub async fn get_file_info(path: String) -> Result<FileInfo, String> {
    tauri::async_runtime::spawn_blocking(move || build_file_info(Path::new(&path)))
        .await
        .map_err(|e| format!("后台任务错误: {}", e))?
}

fn build_file_info(input: &Path) -> Result<FileInfo, String> {
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    let format = Format::from_extension(ext)
        .ok_or_else(|| format!("不支持的文件格式 '{}'", ext))?;

    let info = info_file(input, format, false).map_err(|e| e.to_string())?;

    let mut details: Vec<(String, String)> = vec![
        ("文件大小".to_string(), format!("{} 字节", info.file_size)),
        ("主格式".to_string(), format!("{:?}", info.format)),
    ];

    let footer = match &info.footer_info {
        FooterInfo::QTag { ekey, song_id } => {
            details.push(("尾部类型".to_string(), "QTag".to_string()));
            details.push(("歌曲 ID".to_string(), song_id.clone()));
            let preview: String = ekey.chars().take(40).collect();
            details.push((
                "EKey".to_string(),
                format!("{}…（共 {} 字符）", preview, ekey.chars().count()),
            ));
            "QTag".to_string()
        }
        FooterInfo::V1 { key_size } => {
            details.push(("尾部类型".to_string(), "V1".to_string()));
            details.push(("密钥大小".to_string(), key_size.to_string()));
            "V1".to_string()
        }
        FooterInfo::Musicex {
            song_id,
            mid,
            filename,
        } => {
            details.push(("尾部类型".to_string(), "musicex".to_string()));
            details.push(("歌曲 ID".to_string(), song_id.to_string()));
            details.push(("媒体 MID".to_string(), mid.clone()));
            details.push(("文件名".to_string(), filename.clone()));
            "musicex".to_string()
        }
        FooterInfo::Unknown => {
            details.push(("尾部类型".to_string(), "未知（未检测到尾部或为 QMC1）".to_string()));
            "未知".to_string()
        }
    };

    Ok(FileInfo {
        path: input.display().to_string(),
        size: info.file_size,
        format: ext.to_string(),
        footer,
        details,
    })
}

/// Fetch the ekey for a musicex-format file from the QQ Music API.
/// Requires the local QQ Music client to be logged in.
#[tauri::command]
pub async fn fetch_ekey_musicex(path: String) -> Result<String, String> {
    fetch_ekey(Path::new(&path)).await.map_err(|e| e.to_string())
}

/// Whether the local QQ Music credentials can be read (drives the auto-fetch toggle).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialsStatus {
    pub found: bool,
    pub uin: Option<String>,
    pub reason: Option<String>,
}

#[tauri::command]
pub async fn check_credentials() -> CredentialsStatus {
    // Reads local credentials (plist / process memory); run off the main thread.
    tauri::async_runtime::spawn_blocking(get_qqmusic_credentials)
        .await
        .map(|res| match res {
            Ok(creds) => CredentialsStatus {
                found: true,
                uin: Some(creds.uin),
                reason: None,
            },
            Err(e) => CredentialsStatus {
                found: false,
                uin: None,
                reason: Some(e.to_string()),
            },
        })
        .unwrap_or_else(|_| CredentialsStatus {
            found: false,
            uin: None,
            reason: Some("后台检查任务失败".to_string()),
        })
}

/// Best default location to open file pickers: the QQ Music download folder.
fn qqmusic_download_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    // macOS sandbox container download path
    let container = home.join(
        "Library/Containers/com.tencent.QQMusicMac/Data/Library/Application Support/QQMusicMac/iQmc",
    );
    if container.is_dir() {
        return Some(container);
    }
    // Common non-sandbox download path
    let music = home.join("Music/QQ\u{97f3}\u{4e50}");
    if music.is_dir() {
        return Some(music);
    }
    None
}

#[tauri::command]
pub fn get_default_download_dir() -> Option<String> {
    qqmusic_download_dir().map(|p| p.display().to_string())
}

// ---------------------------------------------------------------------------
// Add-path resolution: expand folders into their contained supported files
// (non-recursive, matching how decrypt walks directories).
// ---------------------------------------------------------------------------
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AddItem {
    pub path: String,
    pub is_dir: bool,
    /// Full paths of the supported files inside a folder (empty for a file).
    pub songs: Vec<String>,
}

fn add_item_one(input: &Path) -> Option<AddItem> {
    if input.is_file() {
        return Some(AddItem {
            path: input.display().to_string(),
            is_dir: false,
            songs: Vec::new(),
        });
    }
    if input.is_dir() {
        let mut files = Vec::new();
        collect_supported(input, &mut files, 0);
        let mut songs: Vec<String> = files.into_iter().map(|p| p.display().to_string()).collect();
        songs.sort();
        return Some(AddItem {
            path: input.display().to_string(),
            is_dir: true,
            songs,
        });
    }
    None
}

/// Inspect dropped/selected paths and expand folders into their contained
/// supported files so the UI can list the songs instead of the folder row.
#[tauri::command]
pub async fn inspect_paths(paths: Vec<String>) -> Vec<AddItem> {
    tauri::async_runtime::spawn_blocking(move || {
        paths.iter().filter_map(|p| add_item_one(Path::new(p))).collect()
    })
    .await
    .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Native file/folder pickers (rfd, run off the main thread)
// ---------------------------------------------------------------------------

const QMC_FILTERS: [&str; 12] = [
    "qmc0", "qmc2", "qmc3", "qmcflac", "qmcogg", "mgg", "mgg0", "mgg1", "mggl",
    "mflac", "mflac0", "mflach",
];

fn valid_dir_or_default(default_path: Option<String>) -> Option<String> {
    let from_user = default_path.filter(|d| PathBuf::from(d).is_dir());
    if from_user.is_some() {
        return from_user;
    }
    qqmusic_download_dir().map(|p| p.display().to_string())
}

#[tauri::command]
pub async fn pick_files(default_path: Option<String>) -> Result<Vec<String>, String> {
    let dir = valid_dir_or_default(default_path);
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new()
            .add_filter("QMC 音频", &QMC_FILTERS)
            .add_filter("所有文件", &["*"]);
        if let Some(d) = dir {
            dialog = dialog.set_directory(d);
        }
        Ok::<Vec<String>, String>(
            dialog
                .pick_files()
                .unwrap_or_default()
                .into_iter()
                .map(|f| f.display().to_string())
                .collect(),
        )
    })
    .await
    .map_err(|e| format!("文件选择器错误: {}", e))?
}

#[tauri::command]
pub async fn pick_folder(default_path: Option<String>) -> Result<Option<String>, String> {
    let dir = valid_dir_or_default(default_path);
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new();
        if let Some(d) = dir {
            dialog = dialog.set_directory(d);
        }
        Ok::<Option<String>, String>(dialog.pick_folder().map(|f| f.display().to_string()))
    })
    .await
    .map_err(|e| format!("文件夹选择器错误: {}", e))?
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_tree(root: &Path) -> PathBuf {
        let root = root.to_path_buf();

        let a = root.join("歌手A");
        let b = root.join("歌手B");
        let sub = b.join("子专辑");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(root.join("空文件夹")).unwrap();

        fs::write(a.join("01 晴天.qmcflac"), b"x").unwrap();
        fs::write(a.join("02 星晴.mflac"), b"x").unwrap();
        fs::write(b.join("03 夜曲.qmcogg"), b"x").unwrap();
        fs::write(sub.join("04 七里香.mflac0"), b"x").unwrap();
        fs::write(sub.join("05 曲谱.pdf"), b"x").unwrap();      // 不支持
        fs::write(root.join("说明.txt"), b"x").unwrap();         // 不支持
        root
    }

    #[test]
    fn collect_finds_songs_recursively() {
        let root = make_tree(&std::env::temp_dir().join(format!("qmc_test_{}", std::process::id())));
        let mut files = Vec::new();
        collect_supported(&root, &mut files, 0);
        fs::remove_dir_all(&root).unwrap();
        let names: Vec<String> = files.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert!(
            names == ["01 晴天.qmcflac", "02 星晴.mflac", "03 夜曲.qmcogg", "04 七里香.mflac0"],
            "got {:?}", names
        );
    }

    #[test]
    fn add_item_expands_dir_and_keeps_file() {
        let root = make_tree(&std::env::temp_dir().join(format!("qmc_test_f_{}", std::process::id())));
        let dir_item = add_item_one(&root).unwrap();
        let file_path = root.join("歌手A/01 晴天.qmcflac");
        let file_item = add_item_one(&file_path).unwrap();
        // 深度：向下穿过 root/歌手A 一层
        let deep_dir = root.join("d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12");
        fs::create_dir_all(&deep_dir).unwrap();
        fs::write(deep_dir.join("deep.mflac"), b"x").unwrap();
        let deep_item = add_item_one(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert!(dir_item.is_dir);
        assert_eq!(dir_item.songs.len(), 4);
        assert!(!file_item.is_dir);
        assert!(file_item.songs.is_empty());
        // 深度上限：超过 10 层的文件不应出现
        assert!(deep_item.songs.iter().all(|s| !s.contains("deep.mflac")), "deep file escaped depth cap");
    }
}
