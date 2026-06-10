use std::path::PathBuf;
use std::sync::mpsc;

use qmc_decoder::{decrypt_file, determine_output_path, info_file, Format, FooterInfo};

// ---------------------------------------------------------------------------
// i18n
// ---------------------------------------------------------------------------

#[derive(Default, PartialEq, Clone, Copy)]
enum Language {
    #[default]
    Zh,
    En,
}

struct Translations {
    title: &'static str,
    subtitle: &'static str,
    input_file: &'static str,
    output_path: &'static str,
    ekey_label: &'static str,
    auto_fetch_ekey: &'static str,
    batch_mode: &'static str,
    browse: &'static str,
    show: &'static str,
    hide: &'static str,
    decrypt_btn: &'static str,
    info_btn: &'static str,
    enter_ekey: &'static str,
    working: &'static str,
    input_required: &'static str,
    select_file: &'static str,
    select_dir: &'static str,
    select_output_dir: &'static str,
    footer_qtag: &'static str,
    footer_v1: &'static str,
    footer_musicex: &'static str,
    footer_unknown: &'static str,
    song_id: &'static str,
    media_mid: &'static str,
    filename: &'static str,
    ekey_truncated: &'static str,
    key_size: &'static str,
    drag_drop_hint: &'static str,
    results: &'static str,
}

const ZH: Translations = Translations {
    title: "QMC Decoder",
    subtitle: "解密 QQ 音乐加密音频文件",
    input_file: "输入文件：",
    output_path: "输出路径：",
    ekey_label: "EKey：",
    auto_fetch_ekey: "自动获取 EKey（需本机登录 QQ 音乐）",
    batch_mode: "批量模式（选择文件夹）",
    browse: "浏览…",
    show: "显示",
    hide: "隐藏",
    decrypt_btn: "解密",
    info_btn: "信息",
    enter_ekey: "输入 EKey（Base64 编码）",
    working: "处理中…",
    input_required: "请选择输入文件或文件夹",
    select_file: "选择加密音频文件",
    select_dir: "选择包含加密文件的文件夹",
    select_output_dir: "选择输出目录",
    footer_qtag: "QTag",
    footer_v1: "V1",
    footer_musicex: "musicex",
    footer_unknown: "未知",
    song_id: "歌曲 ID：{}",
    media_mid: "媒体 MID：{}",
    filename: "文件名：{}",
    ekey_truncated: "EKey：{}…（共 {} 字符）",
    key_size: "密钥大小：{}",
    drag_drop_hint: "或将文件拖放到此处",
    results: "处理结果",
};

const EN: Translations = Translations {
    title: "QMC Decoder",
    subtitle: "Decrypt QQ Music encrypted audio files",
    input_file: "Input file:",
    output_path: "Output path:",
    ekey_label: "EKey:",
    auto_fetch_ekey: "Auto-fetch EKey (requires logged-in QQ Music)",
    batch_mode: "Batch mode (select folder)",
    browse: "Browse…",
    show: "Show",
    hide: "Hide",
    decrypt_btn: "Decrypt",
    info_btn: "Info",
    enter_ekey: "Enter EKey (Base64 encoded)",
    working: "Working…",
    input_required: "Please select an input file or folder",
    select_file: "Select encrypted audio file",
    select_dir: "Select folder with encrypted files",
    select_output_dir: "Select output directory",
    footer_qtag: "QTag",
    footer_v1: "V1",
    footer_musicex: "musicex",
    footer_unknown: "Unknown",
    song_id: "Song ID: {}",
    media_mid: "Media MID: {}",
    filename: "Filename: {}",
    ekey_truncated: "EKey: {}… ({} chars)",
    key_size: "Key size: {}",
    drag_drop_hint: "or drag and drop files here",
    results: "Results",
};

fn t(lang: Language) -> &'static Translations {
    match lang {
        Language::Zh => &ZH,
        Language::En => &EN,
    }
}

// ---------------------------------------------------------------------------
// CJK Font Loading
// ---------------------------------------------------------------------------

fn load_cjk_system_font() -> Option<Vec<u8>> {
    let candidates: &[&str] = &[
        // macOS
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
        // Windows
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyhbd.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
        // Linux
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
    ];

    for path in candidates {
        if let Ok(data) = std::fs::read(path) {
            return Some(data);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn run() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([680.0, 520.0])
            .with_min_inner_size([500.0, 420.0])
            .with_title("QMC Decoder"),
        ..Default::default()
    };

    eframe::run_native(
        "QMC Decoder",
        options,
        Box::new(|cc| {
            if let Some(font_data) = load_cjk_system_font() {
                cc.egui_ctx.add_font(egui::epaint::text::FontInsert::new(
                    "cjk_system_font",
                    egui::FontData::from_owned(font_data),
                    vec![
                        egui::epaint::text::InsertFontFamily {
                            family: egui::FontFamily::Proportional,
                            priority: egui::epaint::text::FontPriority::Lowest,
                        },
                        egui::epaint::text::InsertFontFamily {
                            family: egui::FontFamily::Monospace,
                            priority: egui::epaint::text::FontPriority::Lowest,
                        },
                    ],
                ));
            }
            Ok(Box::new(App::default()))
        }),
    )
    .expect("Failed to launch GUI");
}

// ---------------------------------------------------------------------------
// Operation status
// ---------------------------------------------------------------------------

#[derive(Default)]
enum OperationStatus {
    #[default]
    Idle,
    Running,
    Success(String),
    Error(String),
}

// ---------------------------------------------------------------------------
// Per-file result for batch mode
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FileResult {
    filename: String,
    success: bool,
    message: String,
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

#[derive(Default)]
struct App {
    input_path: String,
    output_path: String,
    ekey: String,
    show_ekey: bool,
    auto_fetch_ekey: bool,
    batch_mode: bool,
    lang: Language,

    // Status for single-file operations
    status: OperationStatus,
    result_rx: Option<mpsc::Receiver<Result<OpOutput, String>>>,

    // File picker receivers
    input_picker_rx: Option<mpsc::Receiver<Option<String>>>,
    output_picker_rx: Option<mpsc::Receiver<Option<String>>>,

    // Batch results
    batch_results: Vec<FileResult>,
}

/// Output from a background operation
enum OpOutput {
    Decrypt {
        input: PathBuf,
        output: PathBuf,
        bytes: usize,
    },
    Batch {
        results: Vec<FileResult>,
    },
}

impl App {
    /// Try to find the QQ Music download directory on macOS.
    /// Checks the sandbox container path first, then ~/Music/QQ音乐.
    fn qqmusic_download_dir() -> Option<PathBuf> {
        // macOS sandbox container download path
        if let Some(home) = dirs::home_dir() {
            let container_path = home.join(
                "Library/Containers/com.tencent.QQMusicMac/Data/Library/Application Support/QQMusicMac/iQmc",
            );
            if container_path.is_dir() {
                return Some(container_path);
            }
            // Common non-sandbox download path
            let music_path = home.join("Music/QQ音乐");
            if music_path.is_dir() {
                return Some(music_path);
            }
        }
        None
    }

    fn spawn_file_picker(
        ctx: &egui::Context,
        title: &str,
        filters: Vec<(String, Vec<String>)>,
    ) -> mpsc::Receiver<Option<String>> {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let title = title.to_string();
        let default_dir = Self::qqmusic_download_dir();
        std::thread::spawn(move || {
            let mut dialog = rfd::AsyncFileDialog::new().set_title(&title);
            if let Some(dir) = &default_dir {
                dialog = dialog.set_directory(dir);
            }
            for (name, exts) in &filters {
                dialog = dialog.add_filter(name, exts);
            }
            let file = pollster::block_on(dialog.pick_file());
            let path = file.map(|f| f.path().display().to_string());
            let _ = tx.send(path);
            ctx.request_repaint();
        });
        rx
    }

    fn spawn_folder_picker(ctx: &egui::Context, title: &str) -> mpsc::Receiver<Option<String>> {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let title = title.to_string();
        let default_dir = Self::qqmusic_download_dir();
        std::thread::spawn(move || {
            let mut dialog = rfd::AsyncFileDialog::new().set_title(&title);
            if let Some(dir) = &default_dir {
                dialog = dialog.set_directory(dir);
            }
            let folder = pollster::block_on(dialog.pick_folder());
            let path = folder.map(|f| f.path().display().to_string());
            let _ = tx.send(path);
            ctx.request_repaint();
        });
        rx
    }

    fn poll_pending_pickers(&mut self) {
        if let Some(rx) = self.input_picker_rx.take() {
            if let Ok(path) = rx.try_recv() {
                if let Some(p) = path {
                    self.input_path = p;
                    self.auto_populate_output();
                }
            } else {
                self.input_picker_rx = Some(rx);
            }
        }
        if let Some(rx) = self.output_picker_rx.take() {
            if let Ok(path) = rx.try_recv() {
                if let Some(p) = path {
                    self.output_path = p;
                }
            } else {
                self.output_picker_rx = Some(rx);
            }
        }
    }

    fn auto_populate_output(&mut self) {
        if self.output_path.is_empty() || self.output_path == self.default_output_path() {
            self.output_path = self.default_output_path();
        }
    }

    fn default_output_path(&self) -> String {
        let input = PathBuf::from(&self.input_path);
        if self.batch_mode {
            self.input_path.clone()
        } else {
            let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
            if let Some(fmt) = Format::from_extension(ext) {
                let stem = input.file_stem().unwrap_or_default().to_string_lossy();
                let parent = input.parent().unwrap_or(std::path::Path::new("."));
                format!("{}/{}.{}", parent.display(), stem, fmt.decrypted_extension())
            } else {
                self.input_path.clone()
            }
        }
    }

    fn poll_operation_result(&mut self) {
        if let Some(rx) = self.result_rx.take() {
            if let Ok(result) = rx.try_recv() {
                self.status = match result {
                    Ok(OpOutput::Decrypt { input, output, bytes }) => {
                        OperationStatus::Success(
                            format!("{} → {} ({} bytes)", input.display(), output.display(), bytes)
                        )
                    }
                    Ok(OpOutput::Batch { results }) => {
                        let ok_count = results.iter().filter(|r| r.success).count();
                        let fail_count = results.len() - ok_count;
                        self.batch_results = results;
                        OperationStatus::Success(
                            format!("{} / {} {}", ok_count, ok_count + fail_count,
                                if self.lang == Language::Zh { "个文件成功" } else { "files OK" })
                        )
                    }
                    Err(e) => OperationStatus::Error(e),
                };
            } else {
                self.result_rx = Some(rx);
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty() {
            if let Some(file) = dropped.first() {
                if let Some(path) = &file.path {
                    let path_str = path.display().to_string();
                    self.batch_mode = path.is_dir();
                    self.input_path = path_str;
                    self.auto_populate_output();
                }
            }
        }
    }

    fn format_footer_info(&self, footer_info: &FooterInfo, tr: &Translations) -> String {
        match footer_info {
            FooterInfo::QTag { ekey, song_id } => {
                let ekey_display = if ekey.len() > 40 {
                    tr.ekey_truncated
                        .replace("{}", &ekey[..40])
                        .replace(&ekey[..40].to_string(), &format!("{}… ({} chars)", &ekey[..40], ekey.len()))
                } else {
                    format!("{} chars", ekey.len())
                };
                format!(
                    "Footer: {}\n  {}: {}\n  EKey: {}",
                    tr.footer_qtag,
                    tr.song_id.replace("{}", song_id),
                    song_id,
                    ekey_display
                )
            }
            FooterInfo::V1 { key_size: ks } => {
                format!("Footer: {} ({})", tr.footer_v1, tr.key_size.replace("{}", &ks.to_string()))
            }
            FooterInfo::Musicex {
                song_id,
                mid,
                filename,
            } => {
                format!(
                    "Footer: {} (v1)\n  {}\n  {}\n  {}",
                    tr.footer_musicex,
                    tr.song_id.replace("{}", &song_id.to_string()),
                    tr.media_mid.replace("{}", mid),
                    tr.filename.replace("{}", filename),
                )
            }
            FooterInfo::Unknown => format!("Footer: {}", tr.footer_unknown),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_pending_pickers();
        self.poll_operation_result();
        self.handle_dropped_files(ctx);

        let tr = t(self.lang);
        let is_running = matches!(self.status, OperationStatus::Running);

        egui::CentralPanel::default().show(ctx, |ui| {
            // Title row with language toggle
            ui.horizontal(|ui| {
                ui.heading(tr.title);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let lang_label = match self.lang {
                        Language::Zh => "EN",
                        Language::En => "中文",
                    };
                    if ui.small_button(lang_label).clicked() {
                        self.lang = match self.lang {
                            Language::Zh => Language::En,
                            Language::En => Language::Zh,
                        };
                    }
                    ui.add_space(4.0);
                    ui.hyperlink_to("GitHub", "https://github.com/ownlight6/qmc-decoder");
                });
            });
            ui.label(tr.subtitle);
            ui.add_space(8.0);

            // ===== Input section =====
            ui.label(tr.input_file);
            ui.add_enabled(
                !is_running,
                egui::TextEdit::singleline(&mut self.input_path)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui.add_enabled(!is_running, egui::Button::new(tr.browse)).clicked() {
                    if self.batch_mode {
                        self.input_picker_rx =
                            Some(Self::spawn_folder_picker(ctx, tr.select_dir));
                    } else {
                        self.input_picker_rx = Some(Self::spawn_file_picker(
                            ctx,
                            tr.select_file,
                            vec![(
                                "QMC Audio".into(),
                                vec![
                                    "qmc0".into(),
                                    "qmc2".into(),
                                    "qmc3".into(),
                                    "qmcflac".into(),
                                    "qmcogg".into(),
                                    "mgg".into(),
                                    "mgg0".into(),
                                    "mgg1".into(),
                                    "mggl".into(),
                                    "mflac".into(),
                                    "mflac0".into(),
                                    "mflach".into(),
                                ],
                            )],
                        ));
                    }
                }
                if ui
                    .add_enabled(!is_running, egui::Checkbox::new(&mut self.batch_mode, tr.batch_mode))
                    .changed()
                {
                    self.auto_populate_output();
                }
                ui.label(
                    egui::RichText::new(tr.drag_drop_hint)
                        .small()
                        .color(egui::Color32::GRAY),
                );
            });
            ui.add_space(4.0);

            // ===== Output section =====
            ui.label(tr.output_path);
            ui.add_enabled(
                !is_running,
                egui::TextEdit::singleline(&mut self.output_path)
                    .desired_width(f32::INFINITY),
            );
            if ui.add_enabled(!is_running, egui::Button::new(tr.browse)).clicked() {
                self.output_picker_rx =
                    Some(Self::spawn_folder_picker(ctx, tr.select_output_dir));
            }
            ui.add_space(4.0);

            // ===== EKey section =====
            ui.label(tr.ekey_label);
            ui.add_enabled(
                !is_running && !self.auto_fetch_ekey,
                egui::TextEdit::singleline(&mut self.ekey)
                    .password(!self.show_ekey)
                    .desired_width(f32::INFINITY)
                    .hint_text(tr.enter_ekey),
            );
            ui.horizontal(|ui| {
                if ui
                    .small_button(if self.show_ekey { tr.hide } else { tr.show })
                    .clicked()
                {
                    self.show_ekey = !self.show_ekey;
                }
                ui.add_enabled(!is_running, egui::Checkbox::new(&mut self.auto_fetch_ekey, tr.auto_fetch_ekey));
            });
            ui.add_space(8.0);

            // ===== Action buttons =====
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!is_running, egui::Button::new(tr.decrypt_btn))
                    .clicked()
                {
                    if self.input_path.is_empty() {
                        self.status = OperationStatus::Error(tr.input_required.into());
                    } else {
                        let input = PathBuf::from(&self.input_path);
                        let output = self.output_path.clone();
                        let ekey = if self.auto_fetch_ekey || self.ekey.is_empty() {
                            None
                        } else {
                            Some(self.ekey.clone())
                        };
                        let fetch_ekey = self.auto_fetch_ekey;
                        let batch_mode = self.batch_mode;
                        let (tx, rx) = mpsc::channel();
                        let ctx = ctx.clone();

                        self.status = OperationStatus::Running;
                        self.batch_results.clear();

                        std::thread::spawn(move || {
                            let result: Result<OpOutput, String> = if batch_mode {
                                let input_dir = input;
                                let output_dir: Option<&std::path::Path> = if output.is_empty() {
                                    None
                                } else {
                                    Some(std::path::Path::new(&output))
                                };
                                let results =
                                    qmc_decoder::process_directory(
                                        &input_dir,
                                        output_dir,
                                        ekey.as_deref(),
                                        fetch_ekey,
                                    );
                                let file_results: Vec<FileResult> = results
                                    .into_iter()
                                    .map(|r| match r {
                                        Ok(dec) => FileResult {
                                            filename: dec.input_path.display().to_string(),
                                            success: true,
                                            message: format!(
                                                "→ {} ({} bytes)",
                                                dec.output_path.display(),
                                                dec.decrypted_bytes
                                            ),
                                        },
                                        Err(e) => FileResult {
                                            filename: input_dir.display().to_string(),
                                            success: false,
                                            message: e,
                                        },
                                    })
                                    .collect();
                                Ok(OpOutput::Batch {
                                    results: file_results,
                                })
                            } else {
                                // Single file
                                let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
                                let format = match Format::from_extension(ext) {
                                    Some(f) => f,
                                    None => {
                                        let _ = tx.send(Err(format!(
                                            "Unsupported file extension '{}'",
                                            ext
                                        )));
                                        ctx.request_repaint();
                                        return;
                                    }
                                };
                                let out_path = if output.is_empty() {
                                    determine_output_path(&input, None, format)
                                } else {
                                    PathBuf::from(&output)
                                };
                                match decrypt_file(
                                    &input,
                                    &out_path,
                                    format,
                                    ekey.as_deref(),
                                    fetch_ekey,
                                ) {
                                    Ok(dec) => Ok(OpOutput::Decrypt {
                                        input: dec.input_path,
                                        output: dec.output_path,
                                        bytes: dec.decrypted_bytes,
                                    }),
                                    Err(e) => Err(e),
                                }
                            };
                            let _ = tx.send(result);
                            ctx.request_repaint();
                        });

                        self.result_rx = Some(rx);
                    }
                }

                if ui
                    .add_enabled(!is_running && !self.batch_mode, egui::Button::new(tr.info_btn))
                    .clicked()
                {
                    if self.input_path.is_empty() {
                        self.status = OperationStatus::Error(tr.input_required.into());
                    } else {
                        let input = PathBuf::from(&self.input_path);
                        let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
                        let fetch_creds = self.auto_fetch_ekey;

                        match Format::from_extension(ext) {
                            Some(format) => {
                                match info_file(&input, format, fetch_creds) {
                                    Ok(info) => {
                                        let mut info_text = format!(
                                            "File: {}\n",
                                            info.input_path.display()
                                        );
                                        info_text.push_str(&format!(
                                            "Size: {} bytes\n",
                                            info.file_size
                                        ));
                                        info_text
                                            .push_str(&format!("Format: {:?}\n", info.format));
                                        info_text
                                            .push_str(&self.format_footer_info(&info.footer_info, tr));
                                        if fetch_creds {
                                            if let FooterInfo::Musicex { .. } = &info.footer_info {
                                                match qmc_decoder::get_qqmusic_credentials() {
                                                    Ok(creds) => {
                                                        info_text.push_str(&format!(
                                                            "\nQQ Music credentials: found (uin={})",
                                                            creds.uin
                                                        ));
                                                    }
                                                    Err(e) => {
                                                        info_text.push_str(&format!(
                                                            "\nQQ Music credentials: not found ({})",
                                                            e
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                        self.status = OperationStatus::Success(info_text);
                                    }
                                    Err(e) => {
                                        self.status = OperationStatus::Error(e);
                                    }
                                }
                            }
                            None => {
                                self.status = OperationStatus::Error(format!(
                                    "Unsupported file extension '{}'",
                                    ext
                                ));
                            }
                        }
                    }
                }
            });

            ui.add_space(8.0);

            // ===== Status display =====
            match &self.status {
                OperationStatus::Idle => {}
                OperationStatus::Running => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr.working);
                    });
                }
                OperationStatus::Success(msg) => {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("✓ {}", msg))
                                .color(egui::Color32::from_rgb(0x2E, 0x7D, 0x32)),
                        ).wrap(),
                    ).on_hover_text(msg);
                }
                OperationStatus::Error(err) => {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("✗ {}", err))
                                .color(egui::Color32::from_rgb(0xC6, 0x28, 0x28)),
                        ).wrap(),
                    ).on_hover_text(err);
                }
            }

            // ===== Batch results =====
            if !self.batch_results.is_empty() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(tr.results).strong());
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        // Simple vertical list
                        for result in &self.batch_results {
                            ui.horizontal(|ui| {
                                if result.success {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(0x2E, 0x7D, 0x32),
                                        "✓",
                                    );
                                } else {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(0xC6, 0x28, 0x28),
                                        "✗",
                                    );
                                }
                                ui.label(
                                    egui::RichText::new(&result.filename).small(),
                                );
                            });
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&result.message).small().color(
                                        if result.success {
                                            egui::Color32::GRAY
                                        } else {
                                            egui::Color32::from_rgb(0xC6, 0x28, 0x28)
                                        },
                                    ),
                                ).wrap(),
                            );
                        }
                    });
            }
        });
    }
}