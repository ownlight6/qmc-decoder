//! QMC Decoder library
//!
//! Provides core decryption logic for QQ Music encrypted audio files
//! (QMC1 and QMC2 formats), consumed by the Tauri frontend via commands.

mod ekey_fetch;
mod qmc1;
mod qmc2;

use base64::Engine;
use std::fs;
use std::path::{Path, PathBuf};

// Re-export public API from sub-modules
pub use ekey_fetch::{
    call_get_evkey_api, fetch_ekey, get_qqmusic_credentials, parse_musicex_footer,
    EkeyFetchError, MusicexInfo, QQMusicCredentials,
};
pub use qmc2::{Qmc2Crypto, Qmc2Error};

/// Supported encrypted formats and their decrypted output format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    // QMC1 formats
    Qmc0,    // -> mp3
    Qmc2,    // -> ogg
    Qmc3,    // -> mp3
    QmcFlac, // -> flac
    QmcOgg,  // -> ogg
    // QMC2 formats
    Mgg,    // -> ogg
    Mgg0,   // -> ogg
    Mgg1,   // -> ogg
    Mggl,   // -> ogg
    Mflac,  // -> flac
    Mflac0, // -> flac
    MflacH, // -> flac
}

impl Format {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "qmc0" => Some(Format::Qmc0),
            "qmc2" => Some(Format::Qmc2),
            "qmc3" => Some(Format::Qmc3),
            "qmcflac" => Some(Format::QmcFlac),
            "qmcogg" => Some(Format::QmcOgg),
            "mgg" => Some(Format::Mgg),
            "mgg0" => Some(Format::Mgg0),
            "mgg1" => Some(Format::Mgg1),
            "mggl" => Some(Format::Mggl),
            "mflac" => Some(Format::Mflac),
            "mflac0" => Some(Format::Mflac0),
            "mflach" => Some(Format::MflacH),
            _ => None,
        }
    }

    pub fn is_qmc1(&self) -> bool {
        matches!(
            self,
            Format::Qmc0 | Format::Qmc2 | Format::Qmc3 | Format::QmcFlac | Format::QmcOgg
        )
    }

    pub fn decrypted_extension(&self) -> &'static str {
        match self {
            Format::Qmc0 | Format::Qmc3 => "mp3",
            Format::Qmc2
            | Format::QmcOgg
            | Format::Mgg
            | Format::Mgg0
            | Format::Mgg1
            | Format::Mggl => "ogg",
            Format::QmcFlac | Format::Mflac | Format::Mflac0 | Format::MflacH => "flac",
        }
    }
}

/// Metadata extracted from a QMC2 file's footer
#[derive(Debug, Clone)]
pub enum FooterInfo {
    /// QMC2 v1: key size stored as last 4 bytes (little-endian u32)
    V1 { key_size: u32 },
    /// QMC2 v2 (QTag): ekey and song_id embedded at end of file
    QTag { ekey: String, song_id: String },
    /// Newer musicex footer: ekey not embedded in file
    Musicex {
        song_id: u32,
        mid: String,
        filename: String,
    },
    /// No recognized footer (might be QMC1 or a raw encrypted file)
    Unknown,
}

/// Result of a successful decryption
#[derive(Debug)]
pub struct DecryptResult {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub format: Format,
    pub decrypted_bytes: usize,
    pub footer_info: FooterInfo,
}

/// Result of an info-only operation
#[derive(Debug)]
pub struct InfoResult {
    pub input_path: PathBuf,
    pub file_size: usize,
    pub format: Format,
    pub footer_info: FooterInfo,
}

/// Read a null-terminated UTF-16LE string from a byte slice at the given offset
pub fn read_utf16_le_string(data: &[u8], offset: usize, max_len: usize) -> String {
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

/// Detect the footer type of a QMC2 file
pub fn detect_footer(data: &[u8]) -> FooterInfo {
    if data.len() < 8 {
        return FooterInfo::Unknown;
    }

    let last4 = &data[data.len() - 4..];

    // Check for "musicex\0" magic at end
    if data.len() >= 16 {
        if &data[data.len() - 8..] == b"musicex\x00" {
            let magic_start = data.len() - 8;
            let version_start = magic_start - 4;
            let meta_size_start = version_start - 4;

            if meta_size_start >= 4 {
                let version =
                    u32::from_le_bytes(data[version_start..magic_start].try_into().unwrap());
                let footer_size =
                    u32::from_le_bytes(data[meta_size_start..version_start].try_into().unwrap());

                let metadata_size = (footer_size as usize).saturating_sub(16);

                if version == 1 && metadata_size > 0 && metadata_size <= meta_size_start {
                    let footer_start = data.len() - (footer_size as usize);
                    let meta = &data[footer_start..meta_size_start];

                    let song_id = if meta.len() > 0x04 {
                        u32::from_le_bytes(meta[0x00..0x04].try_into().unwrap_or([0u8; 4]))
                    } else {
                        0
                    };

                    let mid = read_utf16_le_string(meta, 0x0C, 60);
                    let filename = read_utf16_le_string(meta, 0x48, 68);

                    return FooterInfo::Musicex {
                        song_id,
                        mid,
                        filename,
                    };
                }
            }
        }
    }

    // Check for QTag marker (last 4 bytes = "QTag" in little-endian = 0x67615451)
    let eof_magic = u32::from_le_bytes(last4.try_into().unwrap());
    if eof_magic == 0x6761_5451 {
        if data.len() >= 12 {
            let meta_size_be =
                u32::from_be_bytes(data[data.len() - 8..data.len() - 4].try_into().unwrap());
            let meta_end = data.len() - 8;
            let meta_start = meta_end.saturating_sub(meta_size_be as usize);

            let meta = &data[meta_start..meta_end];
            if let Some(comma_pos) = meta.iter().position(|&b| b == b',') {
                let ekey = String::from_utf8_lossy(&meta[..comma_pos]).to_string();
                let rest = &meta[comma_pos + 1..];
                if let Some(comma2_pos) = rest.iter().position(|&b| b == b',') {
                    let song_id = String::from_utf8_lossy(&rest[..comma2_pos]).to_string();
                    return FooterInfo::QTag { ekey, song_id };
                }
            }
        }
    }

    // Check for QMC2 v1: last 4 bytes as key size (1..=1024)
    let potential_key_size = u32::from_le_bytes(last4.try_into().unwrap());
    if potential_key_size > 0 && potential_key_size <= 0x400 {
        return FooterInfo::V1 {
            key_size: potential_key_size,
        };
    }

    FooterInfo::Unknown
}

/// Determine the output path based on input path, optional output override, and format
pub fn determine_output_path(input: &Path, output: Option<&Path>, format: Format) -> PathBuf {
    if let Some(out) = output {
        if out.is_dir() {
            let stem = input
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let ext = format.decrypted_extension();
            out.join(format!("{}.{}", stem, ext))
        } else {
            out.to_path_buf()
        }
    } else {
        let stem = input
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let ext = format.decrypted_extension();
        input
            .parent()
            .unwrap_or(Path::new("."))
            .join(format!("{}.{}", stem, ext))
    }
}

/// Decrypt a single file, writing the output to disk.
///
/// This is the main decryption entry point. It:
/// 1. Reads the input file
/// 2. Detects the footer format
/// 3. Decrypts using QMC1 or QMC2 cipher as appropriate
/// 4. Writes the decrypted data to the output path
///
/// For QMC2 files with musicex footer, either `ekey` or `fetch_ekey` must be provided.
pub fn decrypt_file(
    input_path: &Path,
    output_path: &Path,
    format: Format,
    ekey: Option<&str>,
    fetch_ekey: bool,
) -> Result<DecryptResult, String> {
    let data = fs::read(input_path)
        .map_err(|e| format!("Failed to read {}: {}", input_path.display(), e))?;

    let footer = detect_footer(&data);

    match format {
        Format::Qmc0 | Format::Qmc2 | Format::Qmc3 | Format::QmcFlac | Format::QmcOgg => {
            // QMC1: use seed-based XOR cipher
            let decrypted_data_len = match &footer {
                FooterInfo::V1 { key_size } => data.len() - 4 - (*key_size as usize),
                _ => data.len(),
            };
            let encrypted = &data[..decrypted_data_len];
            let mut decrypted = encrypted.to_vec();
            qmc1::decrypt(&mut decrypted);
            let decrypted_bytes = decrypted.len();
            fs::write(output_path, &decrypted)
                .map_err(|e| format!("Failed to write {}: {}", output_path.display(), e))?;
            Ok(DecryptResult {
                input_path: input_path.to_path_buf(),
                output_path: output_path.to_path_buf(),
                format,
                decrypted_bytes,
                footer_info: footer,
            })
        }
        _fmt @ (Format::Mgg
        | Format::Mgg0
        | Format::Mgg1
        | Format::Mggl
        | Format::Mflac
        | Format::Mflac0
        | Format::MflacH) => {
            // QMC2: need ekey
            let (ekey_str, audio_len) = match &footer {
                FooterInfo::QTag { ekey, .. } => {
                    let meta_size_be = u32::from_be_bytes(
                        data[data.len() - 8..data.len() - 4].try_into().unwrap(),
                    );
                    let audio_len = data.len() - 8 - (meta_size_be as usize);
                    (ekey.clone(), audio_len)
                }
                FooterInfo::V1 { key_size } => {
                    let key_start = data.len() - 4 - (*key_size as usize);
                    let key_bytes = &data[key_start..data.len() - 4];
                    let ekey_b64 = base64::engine::general_purpose::STANDARD.encode(key_bytes);
                    (ekey_b64, key_start)
                }
                FooterInfo::Musicex {
                    song_id, mid, ..
                } => {
                    let audio_len = if data.len() >= 16 && &data[data.len() - 8..] == b"musicex\x00"
                    {
                        let footer_size = u32::from_le_bytes(
                            data[data.len() - 16..data.len() - 12]
                                .try_into()
                                .unwrap_or([0; 4]),
                        );
                        data.len().saturating_sub(footer_size as usize)
                    } else {
                        data.len()
                    };

                    if let Some(key) = ekey {
                        (key.to_string(), audio_len)
                    } else if fetch_ekey {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| format!("Failed to create async runtime: {}", e))?;
                        let fetched_ekey = rt.block_on(async { ekey_fetch::fetch_ekey(input_path).await })
                            .map_err(|e| format!("{}", e))?;
                        (fetched_ekey, audio_len)
                    } else {
                        return Err(format!(
                            "This file uses the newer 'musicex' format (song_id={}, mid={}).\n\
                             The encryption key (ekey) is NOT embedded in the file.\n\n\
                             You can either:\n\
                             1. Provide the ekey via --ekey argument\n\
                             2. Use --fetch-ekey to automatically fetch the ekey from QQ Music API\n\
                                (requires QQ Music to be logged in on this computer)\n\n\
                             Run with --info to see file metadata.",
                            song_id, mid
                        ));
                    }
                }
                FooterInfo::Unknown => {
                    if let Some(key) = ekey {
                        (key.to_string(), data.len())
                    } else if fetch_ekey {
                        return Err(
                            "Cannot auto-fetch ekey: file does not have a musicex footer.\n\
                             The --fetch-ekey option only works with musicex-format files.\n\
                             Please provide the ekey via --ekey argument instead."
                                .to_string(),
                        );
                    } else {
                        return Err(
                            "Could not detect file footer format and no ekey provided.\n\
                             Please provide the ekey via --ekey argument, or use --fetch-ekey \
                             for musicex-format files."
                                .to_string(),
                        );
                    }
                }
            };

            let encrypted = &data[..audio_len];
            let mut decrypted = encrypted.to_vec();

            let crypto = qmc2::Qmc2Crypto::from_ekey(&ekey_str)
                .map_err(|e| format!("Failed to initialize QMC2 crypto: {}", e))?;
            crypto.decrypt(0, &mut decrypted);

            let decrypted_bytes = decrypted.len();
            fs::write(output_path, &decrypted)
                .map_err(|e| format!("Failed to write {}: {}", output_path.display(), e))?;
            Ok(DecryptResult {
                input_path: input_path.to_path_buf(),
                output_path: output_path.to_path_buf(),
                format,
                decrypted_bytes,
                footer_info: footer,
            })
        }
    }
}

/// Get file metadata without decrypting
pub fn info_file(input_path: &Path, format: Format, check_credentials: bool) -> Result<InfoResult, String> {
    let data = fs::read(input_path)
        .map_err(|e| format!("Failed to read {}: {}", input_path.display(), e))?;

    let file_size = data.len();
    let footer = detect_footer(&data);
    let footer_info = if check_credentials {
        // For musicex footers with credential check, we don't modify the FooterInfo
        // — the caller can check credentials separately via get_qqmusic_credentials()
        footer
    } else {
        footer
    };

    Ok(InfoResult {
        input_path: input_path.to_path_buf(),
        file_size,
        format,
        footer_info,
    })
}

/// Process all supported files in a directory (batch mode).
///
/// Returns a list of results, one per file. Errors are captured per-file
/// rather than aborting the entire batch.
pub fn process_directory(
    input_dir: &Path,
    output_dir: Option<&Path>,
    ekey: Option<&str>,
    fetch_ekey: bool,
) -> Vec<Result<DecryptResult, String>> {
    let out = output_dir.unwrap_or(input_dir);
    if !out.exists() {
        if let Err(e) = fs::create_dir_all(out) {
            return vec![Err(format!("Error creating output directory: {}", e))];
        }
    }

    let mut results = Vec::new();

    if let Ok(entries) = fs::read_dir(input_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if let Some(format) = Format::from_extension(ext) {
                let out_path = determine_output_path(&path, Some(out), format);
                let result = decrypt_file(&path, &out_path, format, ekey, fetch_ekey);
                results.push(result);
            }
        }
    }

    results
}