//! 简易命令行解密示例：
//!   cargo run --release --example decrypt -- <input> <output> <ekey|--auto>
use qmc_decoder::{decrypt_file, fetch_ekey, Format};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("用法: decrypt <input> <output> <ekey|--auto>");
        std::process::exit(2);
    }
    let input = Path::new(&args[1]);
    let output = Path::new(&args[2]);
    let ekey_arg = &args[3];

    let ekey = if ekey_arg == "--auto" {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build runtime");
        match rt.block_on(async { fetch_ekey(input).await }) {
            Ok(k) => {
                println!("自动获取 ekey 成功（{} 字符）", k.chars().count());
                k
            }
            Err(e) => {
                eprintln!("自动获取 ekey 失败: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        ekey_arg.clone()
    };

    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("mflac");
    let format = Format::from_extension(ext).unwrap_or(Format::Mflac);
    match decrypt_file(input, output, format, Some(&ekey), false) {
        Ok(r) => println!(
            "解密成功: {} -> {} ({} 字节, 格式 {:?}, footer {:?})",
            r.input_path.display(),
            r.output_path.display(),
            r.decrypted_bytes,
            r.format,
            r.footer_info
        ),
        Err(e) => {
            eprintln!("解密失败: {}", e);
            std::process::exit(1);
        }
    }
}
