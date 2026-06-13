// On Windows, hide the console window when launching as a GUI app.
// This prevents a terminal window from briefly appearing when double-clicking the exe.
// CLI output still works when launched from a terminal.
#![cfg_attr(all(feature = "gui", target_os = "windows"), windows_subsystem = "windows")]

#[cfg(feature = "gui")]
mod gui;

use clap::Parser;
use qmc_decoder::{determine_output_path, decrypt_file, info_file, process_directory, Format};

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
    input: Option<PathBuf>,

    /// Output file or directory (optional, defaults to same location with changed extension)
    #[arg(value_name = "OUTPUT")]
    output: Option<PathBuf>,

    /// EKey for QMC2 decryption (base64 encoded)
    /// Required for .mgg/.mflac files with musicex footer (unless --fetch-ekey is used)
    #[arg(long)]
    ekey: Option<String>,

    /// Automatically fetch the ekey from QQ Music API for musicex files
    /// Requires QQ Music to be logged in on this computer
    #[arg(long)]
    fetch_ekey: bool,

    /// Extract and display file metadata without decrypting
    #[arg(long)]
    info: bool,

    /// Launch the graphical interface
    #[arg(long)]
    gui: bool,
}

use std::path::PathBuf;

fn main() {
    let args = Args::parse();

    // Launch GUI if --gui flag is set or no positional arguments are provided
    if args.gui || args.input.is_none() {
        #[cfg(feature = "gui")]
        {
            gui::run();
            return;
        }
        #[cfg(not(feature = "gui"))]
        {
            eprintln!("GUI support is not compiled in. Build with --features gui to enable it.");
            eprintln!("Usage: qmc-decoder <INPUT> [OUTPUT] [OPTIONS]");
            std::process::exit(1);
        }
    }

    // CLI mode
    let input = args.input.as_ref().unwrap();

    if !input.exists() {
        eprintln!("Error: input path does not exist: {}", input.display());
        std::process::exit(1);
    }

    if input.is_dir() {
        // Batch mode: decrypt all supported files in directory
        let results = process_directory(
            input,
            args.output.as_deref(),
            args.ekey.as_deref(),
            args.fetch_ekey,
        );
        let count = results.iter().filter(|r| r.is_ok()).count();
        let errors = results.iter().filter(|r| r.is_err()).count();
        for result in &results {
            match result {
                Ok(r) => println!(
                    "Decrypted: {} -> {} ({:?}, {} bytes)",
                    r.input_path.display(),
                    r.output_path.display(),
                    r.format,
                    r.decrypted_bytes
                ),
                Err(e) => eprintln!("Error: {}", e),
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

        if args.info {
            match info_file(input, format, args.fetch_ekey) {
                Ok(info) => {
                    println!("File: {}", info.input_path.display());
                    println!("Size: {} bytes", info.file_size);
                    println!("Format: {:?}", info.format);
                    match &info.footer_info {
                        qmc_decoder::FooterInfo::Musicex {
                            song_id,
                            mid,
                            filename,
                        } => {
                            println!("Footer: musicex (v1)");
                            println!("  Song ID: {}", song_id);
                            println!("  Media MID: {}", mid);
                            println!("  Filename: {}", filename);
                            if args.fetch_ekey {
                                match qmc_decoder::get_qqmusic_credentials() {
                                    Ok(creds) => {
                                        println!("  QQ Music credentials: found (uin={})", creds.uin);
                                    }
                                    Err(e) => {
                                        println!("  QQ Music credentials: not found ({})", e);
                                    }
                                }
                            }
                        }
                        qmc_decoder::FooterInfo::QTag { ekey, song_id } => {
                            println!("Footer: QTag");
                            println!("  Song ID: {}", song_id);
                            println!(
                                "  EKey: {} ({} chars)",
                                &ekey[..ekey.len().min(40)],
                                ekey.len()
                            );
                        }
                        qmc_decoder::FooterInfo::V1 { key_size } => {
                            println!("Footer: V1 (key_size={})", key_size);
                        }
                        qmc_decoder::FooterInfo::Unknown => {
                            println!("Footer: unknown (no footer detected)");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        } else {
            let output = determine_output_path(input, args.output.as_deref(), format);
            match decrypt_file(input, &output, format, args.ekey.as_deref(), args.fetch_ekey) {
                Ok(r) => println!(
                    "Decrypted: {} -> {} ({:?}, {} bytes)",
                    r.input_path.display(),
                    r.output_path.display(),
                    r.format,
                    r.decrypted_bytes
                ),
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}