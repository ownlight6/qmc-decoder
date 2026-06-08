mod ekey_fetch;
mod qmc1;
mod qmc2;

use base64::Engine;
use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};

/// QQ Music encrypted file decoder
///
/// Supports QMC1 formats: .qmc0, .qmc2, .qmc3, .qmcflac, .qmcogg
/// Supports QMC2 formats: .mflac, .mflac0, .mgg, .mgg1, .mggl
///
/// For QMC2 (.mgg/.mflac) files:
/// - If the file has QTag/STag footer, the ekey is extracted automatically
/// - If the file has musicex footer (newer clients), use --ekey or --fetch-ekey
#[derive(Parser, Debug)]
#[command(
    name = "qmc-decoder",
    version,
    about = "Decrypt QQ Music encrypted audio files"
)]
struct Args {
    /// Input file or directory
    #[arg(value_name = "INPUT")]
    input: PathBuf,

    /// Output file or directory (optional, defaults to same location with changed extension)
    #[arg(value_name = "OUTPUT")]
    output: Option<PathBuf>,

    /// EKey for QMC2 decryption (base64 encoded)
    /// Required for .mgg/.mflac files with musicex footer (unless --fetch-ekey is used)
    #[arg(long)]
    ekey: Option<String>,

    /// Automatically fetch the ekey from QQ Music API for musicex files
    /// Requires QQ Music to be logged in on this Mac
    #[arg(long)]
    fetch_ekey: bool,

    /// Extract and display file metadata without decrypting
    #[arg(long)]
    info: bool,
}

/// Supported encrypted formats and their decrypted output format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
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
    fn from_extension(ext: &str) -> Option<Self> {
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

    fn _is_qmc1(&self) -> bool {
        matches!(
            self,
            Format::Qmc0 | Format::Qmc2 | Format::Qmc3 | Format::QmcFlac | Format::QmcOgg
        )
    }

    fn decrypted_extension(&self) -> &'static str {
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
#[derive(Debug)]
enum FooterInfo {
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

fn detect_footer(data: &[u8]) -> FooterInfo {
    if data.len() < 8 {
        return FooterInfo::Unknown;
    }

    let last4 = &data[data.len() - 4..];

    // Check for "musicex\0" magic at end
    if data.len() >= 16 {
        if &data[data.len() - 8..] == b"musicex\x00" {
            // Parse musicex footer structure:
            // The footer is a contiguous block at the end of the file:
            // [metadata content] [footer_size (4B LE)] [version (4B LE)] [magic (8B "musicex\0")]
            // footer_size includes the entire footer (metadata + trailer fields).
            // Layout within the footer_size-byte block:
            //   +0x00: song_id (4B), quality_type1 (4B), quality_type2 (4B)
            //   +0x0C: media_mid (60B UTF-16LE), filename (68B UTF-16LE), padding
            //   +footer_size-16: footer_size (4B), version (4B), magic (8B)
            let magic_start = data.len() - 8;
            let version_start = magic_start - 4; // data[len-12..len-8]
            let meta_size_start = version_start - 4; // data[len-16..len-12]

            if meta_size_start >= 4 {
                let version =
                    u32::from_le_bytes(data[version_start..magic_start].try_into().unwrap());
                let footer_size =
                    u32::from_le_bytes(data[meta_size_start..version_start].try_into().unwrap());

                // footer_size is the total footer size including the 16-byte trailer
                let metadata_size = (footer_size as usize).saturating_sub(16);

                if version == 1 && metadata_size > 0 && metadata_size <= meta_size_start {
                    let footer_start = data.len() - (footer_size as usize);
                    let meta = &data[footer_start..meta_size_start];

                    // Parse musicex metadata:
                    // +0x00: 4B song_id (uint32 LE)
                    // +0x04: 4B quality_type1
                    // +0x08: 4B quality_type2
                    // +0x0C: 60B media_mid (UTF-16LE)
                    // +0x48: 68B filename (UTF-16LE)
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
        // QTag format: [ekey],[song_id],2,[meta_size_be],"QTag"
        if data.len() >= 12 {
            let meta_size_be =
                u32::from_be_bytes(data[data.len() - 8..data.len() - 4].try_into().unwrap());
            let meta_end = data.len() - 8;
            let meta_start = meta_end.saturating_sub(meta_size_be as usize);

            let meta = &data[meta_start..meta_end];
            // Find the ekey (everything before the first comma)
            if let Some(comma_pos) = meta.iter().position(|&b| b == b',') {
                let ekey = String::from_utf8_lossy(&meta[..comma_pos]).to_string();
                // Find song_id (between first and second comma)
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

/// Read a null-terminated UTF-16LE string from a byte slice at the given offset
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

fn determine_output_path(input: &Path, output: Option<&Path>, format: Format) -> PathBuf {
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

fn decrypt_file(
    input_path: &Path,
    output_path: &Path,
    format: Format,
    ekey: Option<&str>,
    fetch_ekey: bool,
    info_only: bool,
) -> Result<(), String> {
    let data = fs::read(input_path)
        .map_err(|e| format!("Failed to read {}: {}", input_path.display(), e))?;

    let footer = detect_footer(&data);

    if info_only {
        println!("File: {}", input_path.display());
        println!("Size: {} bytes", data.len());
        println!("Format: {:?}", format);
        match &footer {
            FooterInfo::Musicex {
                song_id,
                mid,
                filename,
            } => {
                println!("Footer: musicex (v1)");
                println!("  Song ID: {}", song_id);
                println!("  Media MID: {}", mid);
                println!("  Filename: {}", filename);
                if fetch_ekey {
                    match ekey_fetch::get_qqmusic_credentials() {
                        Ok(creds) => {
                            println!("  QQ Music credentials: found (uin={})", creds.uin);
                        }
                        Err(e) => {
                            println!("  QQ Music credentials: not found ({})", e);
                        }
                    }
                }
            }
            FooterInfo::QTag { ekey, song_id } => {
                println!("Footer: QTag");
                println!("  Song ID: {}", song_id);
                println!("  EKey: {} ({} chars)", &ekey[..ekey.len().min(40)], ekey.len());
            }
            FooterInfo::V1 { key_size } => {
                println!("Footer: V1 (key_size={})", key_size);
            }
            FooterInfo::Unknown => {
                println!("Footer: unknown (no footer detected)");
            }
        }
        return Ok(());
    }

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
            fs::write(output_path, &decrypted)
                .map_err(|e| format!("Failed to write {}: {}", output_path.display(), e))?;
            println!(
                "Decrypted: {} -> {} (QMC1, {} bytes)",
                input_path.display(),
                output_path.display(),
                decrypted.len()
            );
            Ok(())
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
                    println!("Found QTag footer, extracting ekey from file");
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
                    println!("Found V1 footer, extracted {}-byte key", key_size);
                    (ekey_b64, key_start)
                }
                FooterInfo::Musicex {
                    song_id, mid, ..
                } => {
                    // audio_len = total file size minus the entire musicex footer
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
                        // Explicit --ekey takes precedence
                        (key.to_string(), audio_len)
                    } else if fetch_ekey {
                        // Auto-fetch ekey from QQ Music API
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| format!("Failed to create async runtime: {}", e))?;
                        let fetched_ekey = rt.block_on(async {
                            ekey_fetch::fetch_ekey(input_path).await
                        }).map_err(|e| format!("{}", e))?;
                        (fetched_ekey, audio_len)
                    } else {
                        return Err(format!(
                            "This file uses the newer 'musicex' format (song_id={}, mid={}).\n\
                             The encryption key (ekey) is NOT embedded in the file.\n\n\
                             You can either:\n\
                             1. Provide the ekey via --ekey argument\n\
                             2. Use --fetch-ekey to automatically fetch the ekey from QQ Music API\n\
                                (requires QQ Music to be logged in on this Mac)\n\n\
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

            fs::write(output_path, &decrypted)
                .map_err(|e| format!("Failed to write {}: {}", output_path.display(), e))?;
            println!(
                "Decrypted: {} -> {} (QMC2, {} bytes)",
                input_path.display(),
                output_path.display(),
                decrypted.len()
            );
            Ok(())
        }
    }
}

fn main() {
    let args = Args::parse();

    let input = &args.input;
    if !input.exists() {
        eprintln!("Error: input path does not exist: {}", input.display());
        std::process::exit(1);
    }

    if input.is_dir() {
        // Batch mode: decrypt all supported files in directory
        let output_dir = args.output.as_deref().unwrap_or(input);
        if !output_dir.exists() {
            fs::create_dir_all(output_dir).unwrap_or_else(|e| {
                eprintln!("Error creating output directory: {}", e);
                std::process::exit(1);
            });
        }

        let mut count = 0u32;
        let mut errors = 0u32;
        if let Ok(entries) = fs::read_dir(input) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if let Some(format) = Format::from_extension(ext) {
                    let out_path = determine_output_path(&path, Some(output_dir), format);
                    match decrypt_file(
                        &path,
                        &out_path,
                        format,
                        args.ekey.as_deref(),
                        args.fetch_ekey,
                        args.info,
                    ) {
                        Ok(_) => count += 1,
                        Err(e) => {
                            eprintln!("Error processing {}: {}", path.display(), e);
                            errors += 1;
                        }
                    }
                }
            }
        }
        println!("Processed {} files ({} errors)", count, errors);
    } else {
        // Single file mode
        let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
        let format = Format::from_extension(ext).unwrap_or_else(|| {
            eprintln!(
                "Error: unsupported file extension '{}'.\n\
                 Supported: qmc0, qmc2, qmc3, qmcflac, qmcogg, mgg, mgg0, mgg1, mggl, mflac, mflac0, mflach",
                ext
            );
            std::process::exit(1);
        });

        let output = determine_output_path(input, args.output.as_deref(), format);
        if let Err(e) = decrypt_file(
            input,
            &output,
            format,
            args.ekey.as_deref(),
            args.fetch_ekey,
            args.info,
        ) {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
