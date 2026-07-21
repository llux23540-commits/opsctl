//! opsctl desktop client (egui/eframe 0.35).
//! 登录(设备绑定)→ 提交 SSH 命令 → 逐目标结果。中文 UI + 深/浅色主题。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod device;

use std::sync::mpsc::{Receiver, Sender};

use eframe::egui;
use egui::{Color32, CornerRadius, FontId, RichText, Stroke, TextStyle, Vec2};
use opsctl_core::api::{JobResult, LoginRequest, LoginResponse, SubmitSshJob};

// ---- 调色板(GitHub 系深色)----
fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
const ACCENT: Color32 = Color32::from_rgb(0x38, 0x8b, 0xfd);
const SUCCESS: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);
const DANGER: Color32 = Color32::from_rgb(0xf8, 0x51, 0x49);
const WARN: Color32 = Color32::from_rgb(0xd2, 0x99, 0x22);
const MUTED: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([940.0, 640.0]),
        // OpenGL renderer: smoother window dragging on Windows than default wgpu.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "opsctl",
        opts,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::new(OpsApp::new()))
        }),
    )
}

/// 系统 CJK 字体优先(命中即作为 Proportional/Monospace 的 fallback)。
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(bytes) = load_cjk_font() {
        let name = "cjk".to_string();
        fonts
            .font_data
            .insert(name.clone(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().push(name.clone());
        }
    } else {
        tracing_note("未找到系统中文字体,中文可能显示为方框");
    }
    ctx.set_fonts(fonts);
}

fn tracing_note(msg: &str) {
    eprintln!("[opsctl-client] {msg}");
}

fn load_cjk_font() -> Option<Vec<u8>> {
    // 系统优先;内置兜底可在此用 include_bytes! 补充。
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    ];
    CANDIDATES.iter().find_map(|p| std::fs::read(p).ok())
}

#[derive(Clone, Copy, PartialEq)]
enum ThemeMode {
    Dark,
    Light,
    System,
}

impl ThemeMode {
    fn next(self) -> Self {
        match self {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::System,
            ThemeMode::System => ThemeMode::Dark,
        }
    }
    fn label(self) -> &'static str {
        match self {
            ThemeMode::Dark => "主题:深色",
            ThemeMode::Light => "主题:浅色",
            ThemeMode::System => "主题:跟随系统",
        }
    }
}

/// 应用主题 + 统一间距/圆角/字号层级。
fn apply_style(ctx: &egui::Context, mode: ThemeMode) {
    let dark = match mode {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => !matches!(
            ctx.input(|i| i.raw.system_theme),
            Some(egui::Theme::Light)
        ),
    };

    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    if dark {
        visuals.panel_fill = rgb(0x161b22);
        visuals.window_fill = rgb(0x0d1117);
        visuals.extreme_bg_color = rgb(0x0d1117);
        visuals.faint_bg_color = rgb(0x1c2128);
        visuals.override_text_color = Some(rgb(0xe6edf3));
        let border = Stroke::new(1.0, rgb(0x30363d));
        visuals.widgets.noninteractive.bg_stroke = border;
        visuals.widgets.inactive.weak_bg_fill = rgb(0x21262d);
        visuals.widgets.inactive.bg_fill = rgb(0x21262d);
        visuals.widgets.inactive.bg_stroke = border;
        visuals.widgets.hovered.weak_bg_fill = rgb(0x30363d);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, rgb(0x8b949e));
        visuals.widgets.active.weak_bg_fill = rgb(0x282e33);
    }
    visuals.hyperlink_color = ACCENT;
    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(0x38, 0x8b, 0xfd, 70);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.window_corner_radius = CornerRadius::same(10);
    let cr = CornerRadius::same(6);
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = cr;
    }

    ctx.all_styles_mut(|style| {
        style.visuals = visuals.clone();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 6.0);
        style.text_styles = [
            (TextStyle::Heading, FontId::new(22.0, egui::FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(15.0, egui::FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(15.0, egui::FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.0, egui::FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.5, egui::FontFamily::Monospace)),
        ]
        .into();
    });
}

/// 消息:后台线程 → UI。
enum Msg {
    LoggedIn(LoginResponse),
    Error(String),
    JobDone(JobResult),
}

struct OpsApp {
    server_url: String,
    device_id: String,
    username: String,
    password: String,
    show_pw: bool,
    token: Option<String>,
    who: String,
    targets: String,
    command: String,
    results: Vec<opsctl_core::api::TargetResult>,
    status: String,
    busy: bool,
    theme: ThemeMode,
    theme_dirty: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl OpsApp {
    fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            server_url: "http://127.0.0.1:8443".into(),
            device_id: device::device_id(),
            username: "admin".into(),
            password: String::new(),
            show_pw: false,
            token: None,
            who: String::new(),
            targets: "node1".into(),
            command: "uname -a".into(),
            results: Vec::new(),
            status: String::new(),
            busy: false,
            theme: ThemeMode::Dark,
            theme_dirty: true,
            tx,
            rx,
        }
    }

    fn spawn_login(&mut self, ctx: &egui::Context) {
        self.busy = true;
        self.status = "登录中…".into();
        let (url, dev, user, pass) = (
            self.server_url.clone(),
            self.device_id.clone(),
            self.username.clone(),
            self.password.clone(),
        );
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match login_blocking(&url, &user, &pass, &dev) {
                Ok(resp) => Msg::LoggedIn(resp),
                Err(e) => Msg::Error(e),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn spawn_job(&mut self, ctx: &egui::Context) {
        let Some(token) = self.token.clone() else {
            return;
        };
        self.busy = true;
        self.status = "执行中…".into();
        let (url, dev) = (self.server_url.clone(), self.device_id.clone());
        let targets: Vec<String> = self
            .targets
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let command = self.command.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match job_blocking(&url, &token, &dev, targets, command) {
                Ok(r) => Msg::JobDone(r),
                Err(e) => Msg::Error(e),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.busy = false;
            match msg {
                Msg::LoggedIn(r) => {
                    self.token = Some(r.token);
                    self.who = format!("{} · {:?}", r.user.name, r.user.role);
                    self.status = "已登录".into();
                    self.password.clear();
                }
                Msg::JobDone(r) => {
                    self.results = r.results;
                    let ok = self.results.iter().filter(|t| t.ok).count();
                    let fail = self.results.len() - ok;
                    self.status = format!("任务完成 · 成功 {ok} / 失败 {fail}");
                }
                Msg::Error(e) => {
                    if e.contains("401") {
                        self.token = None;
                        self.status = "会话失效,已退出登录".into();
                    } else {
                        self.status = format!("错误:{e}");
                    }
                }
            }
        }
    }
}

impl eframe::App for OpsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.theme_dirty {
            apply_style(&ctx, self.theme);
            self.theme_dirty = false;
        }
        self.drain();

        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.header(ui);
            ui.separator();
            ui.add_space(6.0);
            if self.token.is_none() {
                self.login_ui(ui, &ctx);
            } else {
                self.job_ui(ui, &ctx);
            }
            // 底部状态行
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                }
                ui.small(RichText::new(&self.status).color(MUTED));
            });
        });
    }
}

impl OpsApp {
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("opsctl").heading().strong().color(ACCENT));
            ui.small(RichText::new("运维平台").color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(self.theme.label()).clicked() {
                    self.theme = self.theme.next();
                    self.theme_dirty = true;
                }
                if self.token.is_some() {
                    ui.separator();
                    if ui.button("退出登录").clicked() {
                        self.token = None;
                        self.results.clear();
                        self.status = "已退出登录".into();
                    }
                    ui.label(RichText::new(format!("● {}", self.who)).color(SUCCESS));
                }
            });
        });
    }

    fn login_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(36.0);
        ui.vertical_centered(|ui| {
            let card = egui::Frame {
                inner_margin: egui::Margin::same(22),
                fill: ui.visuals().panel_fill,
                stroke: Stroke::new(1.0, rgb(0x30363d)),
                corner_radius: CornerRadius::same(10),
                ..Default::default()
            };
            card.show(ui, |ui| {
                ui.set_width(340.0);
                ui.label(RichText::new("登录 opsctl").heading().strong());
                ui.small(RichText::new("连接到 vault 服务端").color(MUTED));
                ui.add_space(14.0);

                ui.label("服务地址");
                ui.add(egui::TextEdit::singleline(&mut self.server_url).desired_width(f32::INFINITY));
                ui.add_space(8.0);

                ui.label("账号");
                ui.add(egui::TextEdit::singleline(&mut self.username).desired_width(f32::INFINITY));
                ui.add_space(8.0);

                ui.label("密码");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.password)
                            .password(!self.show_pw)
                            .desired_width(f32::INFINITY),
                    );
                });
                if ui
                    .selectable_label(self.show_pw, RichText::new("显示密码").small())
                    .clicked()
                {
                    self.show_pw = !self.show_pw;
                }
                ui.add_space(16.0);

                let btn = egui::Button::new(RichText::new("登 录").color(Color32::WHITE).strong())
                    .fill(ACCENT);
                if ui
                    .add_enabled(!self.busy, btn)
                    .on_hover_text("使用账号密码登录")
                    .clicked()
                {
                    self.spawn_login(ctx);
                }
                ui.add_space(4.0);
                ui.small(RichText::new("测试账号:admin/operator/viewer(密码同名)").color(MUTED));
            });
        });
    }

    fn job_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(RichText::new("远程执行").heading());
        ui.add_space(6.0);
        ui.label("目标(条目 id,逗号分隔)");
        ui.add(egui::TextEdit::singleline(&mut self.targets).desired_width(f32::INFINITY));
        ui.add_space(6.0);
        ui.label("命令");
        ui.add(
            egui::TextEdit::singleline(&mut self.command)
                .font(TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(10.0);

        let btn = egui::Button::new(RichText::new("▶  执行").color(Color32::WHITE).strong()).fill(ACCENT);
        if ui.add_enabled(!self.busy, btn).clicked() {
            self.spawn_job(ctx);
        }

        ui.add_space(10.0);
        // 汇总胶囊
        if !self.results.is_empty() {
            let ok = self.results.iter().filter(|t| t.ok).count();
            let fail = self.results.len() - ok;
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("✓ 成功 {ok}")).color(SUCCESS).strong());
                ui.label(RichText::new(format!("✗ 失败 {fail}")).color(DANGER).strong());
            });
            ui.add_space(4.0);
        }
        ui.separator();

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.results.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("暂无结果").color(MUTED));
                    ui.small(RichText::new("填写目标与命令后点击「执行」").color(MUTED));
                });
                return;
            }
            for r in &self.results {
                result_card(ui, r);
                ui.add_space(6.0);
            }
        });
    }
}

fn result_card(ui: &mut egui::Ui, r: &opsctl_core::api::TargetResult) {
    let (accent, icon) = if r.ok {
        (SUCCESS, "✓")
    } else {
        (DANGER, "✗")
    };
    let card = egui::Frame {
        inner_margin: egui::Margin::same(10),
        fill: ui.visuals().faint_bg_color,
        stroke: Stroke::new(1.0, rgb(0x30363d)),
        corner_radius: CornerRadius::same(8),
        ..Default::default()
    };
    card.show(ui, |ui| {
        // 左侧色条效果:用一个彩色标题
        let head = format!("{icon}  {}", r.target);
        let exit = r
            .exit_code
            .map(|c| format!("  ·  exit {c}"))
            .unwrap_or_default();
        egui::CollapsingHeader::new(
            RichText::new(format!("{head}{exit}")).color(accent).strong(),
        )
        .default_open(true)
        .show(ui, |ui| {
            if let Some(err) = &r.error {
                ui.label(RichText::new(format!("⚠ {err}")).color(DANGER));
                ui.small(RichText::new("请检查目标可达性 / 凭据后重试").color(MUTED));
            }
            if !r.stdout.is_empty() {
                ui.small(RichText::new("stdout").color(MUTED));
                ui.add(egui::Label::new(RichText::new(&r.stdout).monospace()).wrap());
            }
            if !r.stderr.is_empty() {
                ui.small(RichText::new("stderr").color(MUTED));
                ui.label(RichText::new(&r.stderr).monospace().color(WARN));
            }
        });
    });
}

// ---- 阻塞 HTTP(工作线程)----

fn login_blocking(url: &str, user: &str, pass: &str, device: &str) -> Result<LoginResponse, String> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("{url}/login"))
        .json(&LoginRequest {
            username: user.to_string(),
            password: pass.to_string(),
            device_id: device.to_string(),
        })
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{} 登录失败", resp.status().as_u16()));
    }
    resp.json::<LoginResponse>().map_err(|e| e.to_string())
}

fn job_blocking(
    url: &str,
    token: &str,
    device: &str,
    targets: Vec<String>,
    command: String,
) -> Result<JobResult, String> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("{url}/jobs/ssh"))
        .bearer_auth(token)
        .header("x-device-id", device)
        .json(&SubmitSshJob { targets, command })
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{} 任务失败", resp.status().as_u16()));
    }
    resp.json::<JobResult>().map_err(|e| e.to_string())
}
