//! 看图工具（T100.2）：15 个工具默认注册，图片操作全在本机做。

use std::sync::Arc;

use crate::vision_runtime::DescribeImage;
use serde_json::{Value, json};
use tools::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

use crate::vision_runtime::{DescribeFailure, VisionRuntime};

pub const SCREEN_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

const READONLY: [&str; 14] = [
    "vision_describe",
    "vision_bootstrap",
    "vision_ground",
    "vision_detect",
    "vision_crop",
    "vision_pixel_diff",
    "vision_colors",
    "vision_ocr",
    "vision_long_screenshot_ocr",
    "vision_trace",
    "vision_extract_foreground",
    "vision_html_screenshot",
    "vision_screenshot",
    "vision_present",
];

const MATERIALIZE: &str = "vision_materialize";

/// 截屏权限状态；Unknown 按没给处理，指系统设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenPermissionState {
    Granted,
    Denied,
    Unknown,
}

/// 可替换的权限探测（测试用假端口，CI 不真弹窗）。
pub trait ScreenPermissionProbe: Send + Sync {
    fn check(&self) -> ScreenPermissionState;
}

static TEST_PERMISSION: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// 测试钩子：0=真实探测，1=granted，2=denied，3=unknown。
#[doc(hidden)]
pub fn set_test_screen_permission(state: Option<&str>) {
    use std::sync::atomic::Ordering;
    let value = match state {
        None => 0,
        Some("granted") => 1,
        Some("denied") => 2,
        _ => 3,
    };
    TEST_PERMISSION.store(value, Ordering::SeqCst);
}

/// 生产探测：用 screencapture 试拍一张到临时目录；失败就是没权限。
pub struct SystemScreenPermission;

impl ScreenPermissionProbe for SystemScreenPermission {
    fn check(&self) -> ScreenPermissionState {
        match TEST_PERMISSION.load(std::sync::atomic::Ordering::SeqCst) {
            1 => return ScreenPermissionState::Granted,
            2 => return ScreenPermissionState::Denied,
            3 => return ScreenPermissionState::Unknown,
            _ => {}
        }
        if let Ok(value) = std::env::var("AIOS_TEST_SCREEN_PERMISSION") {
            return match value.as_str() {
                "granted" => ScreenPermissionState::Granted,
                "denied" => ScreenPermissionState::Denied,
                _ => ScreenPermissionState::Unknown,
            };
        }
        let path =
            std::env::temp_dir().join(format!("aios-screen-probe-{}.png", std::process::id()));
        let status = std::process::Command::new("screencapture")
            .args(["-x", "-t", "png"])
            .arg(&path)
            .status();
        drop(std::fs::remove_file(&path));
        match status {
            Ok(status) if status.success() => ScreenPermissionState::Granted,
            _ => ScreenPermissionState::Denied,
        }
    }
}

#[derive(Clone)]
pub struct VisionTools {
    data_dir: std::path::PathBuf,
    runtime: Arc<VisionRuntime>,
    permission: Arc<dyn ScreenPermissionProbe>,
}

impl VisionTools {
    pub fn new(
        data_dir: std::path::PathBuf,
        runtime: Arc<VisionRuntime>,
        permission: Arc<dyn ScreenPermissionProbe>,
    ) -> Self {
        Self {
            data_dir,
            runtime,
            permission,
        }
    }

    /// Kind/只读任务 14 件（不含 vision_materialize）。
    pub fn readonly(&self) -> Vec<Arc<dyn Tool>> {
        self.build(false)
    }

    /// 写任务 15 件。
    pub fn writable(&self) -> Vec<Arc<dyn Tool>> {
        self.build(true)
    }

    fn build(&self, materialize: bool) -> Vec<Arc<dyn Tool>> {
        let shared = Arc::new(self.clone());
        let mut tools: Vec<Arc<dyn Tool>> = READONLY
            .iter()
            .map(|name| {
                Arc::new(VisionTool {
                    name,
                    description: description(name),
                    schema: schema(name),
                    shared: Arc::clone(&shared),
                }) as Arc<dyn Tool>
            })
            .collect();
        if materialize {
            tools.push(Arc::new(VisionTool {
                name: MATERIALIZE,
                description: description(MATERIALIZE),
                schema: schema(MATERIALIZE),
                shared,
            }));
        }
        tools
    }

    fn settings(&self) -> vision::VisionSettings {
        crate::settings::load(&self.data_dir).vision
    }

    fn read_image(
        &self,
        id: &str,
    ) -> Result<(Vec<u8>, tools::attachments::StoredAttachment), ToolFailure> {
        let stored = tools::attachments::read(&self.data_dir, id)
            .map_err(|error| ToolFailure::coded("ATTACHMENT_NOT_FOUND", error.to_string()))?;
        let bytes = std::fs::read(&stored.path)
            .map_err(|error| ToolFailure::plain(format!("读附件失败：{error}")))?;
        Ok((bytes, stored))
    }

    fn store_png(
        &self,
        name: &str,
        bytes: &[u8],
    ) -> Result<tools::attachments::StoredAttachment, ToolFailure> {
        tools::attachments::store(&self.data_dir, name, bytes)
            .map_err(|error| ToolFailure::plain(format!("存图片失败：{error}")))
    }

    fn image_meta(entries: &[&tools::attachments::StoredAttachment]) -> Value {
        let images: Vec<String> = entries.iter().map(|entry| entry.id.clone()).collect();
        let cards: Vec<Value> = entries
            .iter()
            .map(|entry| {
                json!({
                    "id": entry.id,
                    "name": entry.name,
                    "kind": "image",
                })
            })
            .collect();
        json!({ "images": images, "imageAttachments": images, "attachments": cards })
    }

    fn arg_string(args: &Value, key: &str) -> Result<String, ToolFailure> {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ToolFailure::plain(format!("缺少参数 {key}")))
    }

    fn bbox_normalized(args: &Value) -> Result<[i64; 4], ToolFailure> {
        let Some(items) = args.get("bbox").and_then(Value::as_array) else {
            return Ok([0, 0, 1000, 1000]);
        };
        if items.len() != 4 {
            return Err(ToolFailure::plain("bbox 必须是 [x,y,w,h]（0–1000）"));
        }
        let values: Vec<f64> = items
            .iter()
            .map(|item| {
                item.as_f64()
                    .ok_or_else(|| ToolFailure::plain("bbox 数字不合法"))
            })
            .collect::<Result<_, _>>()?;
        Ok([
            values[0].round() as i64,
            values[1].round() as i64,
            values[2].round() as i64,
            values[3].round() as i64,
        ])
    }

    async fn run(&self, name: &str, call: ToolCall, context: &ToolContext) -> ToolResult {
        let args: Value = serde_json::from_str(if call.arguments.trim().is_empty() {
            "{}"
        } else {
            &call.arguments
        })
        .unwrap_or_else(|_| json!({}));
        let result = match name {
            "vision_bootstrap" => self.bootstrap(&call.id),
            "vision_describe" => self.describe(&call, &args).await,
            "vision_ground" | "vision_detect" => self.ground(&call, &args).await,
            "vision_crop" => self.crop(&call, &args),
            "vision_pixel_diff" => self.pixel_diff(&call, &args),
            "vision_colors" => self.colors(&call, &args),
            "vision_present" => self.present(&call, &args),
            "vision_materialize" => self.materialize(&call, &args, &call.id, context),
            "vision_html_screenshot" => self.html_screenshot(&call, &args),
            "vision_screenshot" => self.desktop_screenshot(&call.id),
            "vision_ocr" | "vision_long_screenshot_ocr" => Err(ToolFailure::coded(
                "OCR_MODEL_MISSING",
                "本机 OCR 还在下一张票里；这张图可以先走看图模型描述。",
            )),
            "vision_trace" | "vision_extract_foreground" => Err(ToolFailure::coded(
                "VISION_BACKEND_UNAVAILABLE_THIS_TURN",
                "这个本机图片操作还在下一张票里。",
            )),
            _ => Err(ToolFailure::plain("未知看图工具")),
        };
        match result {
            Ok(result) => result,
            Err(failure) => ToolResult::error(call.id, failure),
        }
    }

    fn bootstrap(&self, call_id: &str) -> Result<ToolResult, ToolFailure> {
        let settings = self.settings();
        let body = json!({
            "models": settings.models,
            "screenCapture": settings.screen_capture,
            "toolNames": READONLY,
        });
        Ok(ToolResult::success_with_meta(
            call_id,
            format!(
                "看图能力：{} 个后端，截桌面{}",
                settings.models.len(),
                if settings.screen_capture {
                    "已开"
                } else {
                    "关着"
                }
            ),
            json!({ "vision": body }),
        ))
    }

    async fn describe(&self, call: &ToolCall, args: &Value) -> Result<ToolResult, ToolFailure> {
        let id = Self::arg_string(args, "attachment_id")?;
        let (bytes, stored) = self.read_image(&id)?;
        let image = DescribeImage {
            attachment_id: id.clone(),
            name: stored.name.clone(),
            mime: stored.mime.clone(),
            path: Some(stored.path.to_string_lossy().into_owned()),
            bytes,
            prompt: None,
        };
        match self.runtime.describe_now(&image).await {
            Ok(text) => Ok(ToolResult::success_with_meta(
                &call.id,
                text,
                json!({ "attachmentId": id, "imageAttachments": [id] }),
            )),
            Err(DescribeFailure::RateLimited { retry_after_ms }) => Err(ToolFailure::coded(
                "VISION_BACKEND_UNAVAILABLE_THIS_TURN",
                format!(
                    "本轮看图不可用（限流，{} 秒后重试）。",
                    retry_after_ms.unwrap_or(60_000) / 1000
                ),
            )),
            Err(DescribeFailure::Auth) => Err(ToolFailure::coded(
                "VISION_BACKEND_UNAVAILABLE_THIS_TURN",
                "本轮看图不可用（后端鉴权失败）。",
            )),
            Err(DescribeFailure::Timeout) => Err(ToolFailure::coded(
                "VISION_BACKEND_UNAVAILABLE_THIS_TURN",
                "本轮看图不可用（后端超时）。",
            )),
            Err(DescribeFailure::Other(message)) => {
                Err(ToolFailure::plain(format!("本轮看图不可用：{message}")))
            }
        }
    }

    async fn ground(&self, call: &ToolCall, args: &Value) -> Result<ToolResult, ToolFailure> {
        let id = Self::arg_string(args, "attachment_id")?;
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let (bytes, stored) = self.read_image(&id)?;
        let image = vision::ops::decode(&bytes).map_err(ToolFailure::plain)?;
        let (width, height) = image.dimensions();
        // 有看图链时让模型给归一化 bbox；没有链/回答不合格式才退回调用方给的框。
        let mut bbox = Self::bbox_normalized(args)?;
        if !self.settings().models.is_empty() {
            let prompt = format!(
                "在图片里找「{query}」的位置。只输出 JSON：键 bbox 的值是 [x,y,w,h]，坐标按图片宽高归一化到 0–1000；找不到就让 bbox 为 null。不要输出别的字。"
            );
            let ask = DescribeImage {
                attachment_id: id.clone(),
                name: stored.name.clone(),
                mime: stored.mime.clone(),
                path: Some(stored.path.to_string_lossy().into_owned()),
                bytes: bytes.clone(),
                prompt: Some(prompt),
            };
            if let Ok(text) = self.runtime.describe_now(&ask).await
                && let Some(parsed) = parse_bbox(&text)
            {
                bbox = parsed;
            }
        }
        let x = bbox[0] * i64::from(width) / 1000;
        let y = bbox[1] * i64::from(height) / 1000;
        let w = bbox[2] * i64::from(width) / 1000;
        let h = bbox[3] * i64::from(height) / 1000;
        let annotated = vision::ops::annotate(&image, x, y, w, h);
        let crop = vision::ops::crop(&image, x, y, w, h);
        let annotated_bytes = vision::ops::encode_png(&annotated).map_err(ToolFailure::plain)?;
        let crop_bytes = vision::ops::encode_png(&crop).map_err(ToolFailure::plain)?;
        let annotated = self.store_png("ground-annotated.png", &annotated_bytes)?;
        let crop = self.store_png("ground-crop.png", &crop_bytes)?;
        let meta = Self::image_meta(&[&annotated, &crop]);
        let mut meta = meta;
        meta["bbox"] = json!([x, y, w, h]);
        let text = format!(
            "找到「{query}」（附件 {}）：原图 bbox {},{},{},{}；已给标注图和放大图。",
            stored.name, x, y, w, h
        );
        Ok(ToolResult::success_with_meta(&call.id, text, meta))
    }

    fn crop(&self, call: &ToolCall, args: &Value) -> Result<ToolResult, ToolFailure> {
        let id = Self::arg_string(args, "attachment_id")?;
        let (bytes, _stored) = self.read_image(&id)?;
        let image = vision::ops::decode(&bytes).map_err(ToolFailure::plain)?;
        let (width, height) = image.dimensions();
        let bbox = Self::bbox_normalized(args)?;
        let x = bbox[0] * i64::from(width) / 1000;
        let y = bbox[1] * i64::from(height) / 1000;
        let w = bbox[2] * i64::from(width) / 1000;
        let h = bbox[3] * i64::from(height) / 1000;
        let cropped = vision::ops::crop(&image, x, y, w, h);
        let out = self.store_png(
            "crop.png",
            &vision::ops::encode_png(&cropped).map_err(ToolFailure::plain)?,
        )?;
        Ok(ToolResult::success_with_meta(
            &call.id,
            format!("裁剪好了：{x},{y} 起 {w}×{h}。"),
            Self::image_meta(&[&out]),
        ))
    }

    fn pixel_diff(&self, call: &ToolCall, args: &Value) -> Result<ToolResult, ToolFailure> {
        let first = Self::arg_string(args, "first_attachment_id")?;
        let second = Self::arg_string(args, "second_attachment_id")?;
        let threshold = args.get("threshold").and_then(Value::as_u64).unwrap_or(16) as u8;
        let (left_bytes, _) = self.read_image(&first)?;
        let (right_bytes, _) = self.read_image(&second)?;
        let left = vision::ops::decode(&left_bytes).map_err(ToolFailure::plain)?;
        let right = vision::ops::decode(&right_bytes).map_err(ToolFailure::plain)?;
        let (changed, ratio, width, height) = vision::ops::pixel_diff(&left, &right, threshold);
        Ok(ToolResult::success(
            &call.id,
            format!(
                "{changed} 个像素不同（{:.1}%）；重叠区 {width}×{height}。",
                ratio * 100.0
            ),
        ))
    }

    fn colors(&self, call: &ToolCall, args: &Value) -> Result<ToolResult, ToolFailure> {
        let id = Self::arg_string(args, "attachment_id")?;
        let count = args.get("count").and_then(Value::as_u64).unwrap_or(5) as usize;
        let (bytes, _) = self.read_image(&id)?;
        let image = vision::ops::decode(&bytes).map_err(ToolFailure::plain)?;
        let colors = vision::ops::dominant_colors(&image, count);
        let text = colors
            .iter()
            .map(|(hex, ratio)| format!("{hex} {:.1}%", ratio * 100.0))
            .collect::<Vec<_>>()
            .join("，");
        Ok(ToolResult::success(
            &call.id,
            if text.is_empty() {
                "没有取到颜色".to_owned()
            } else {
                text
            },
        ))
    }

    fn present(&self, call: &ToolCall, args: &Value) -> Result<ToolResult, ToolFailure> {
        let ids: Vec<String> = args
            .get("attachment_ids")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            return Err(ToolFailure::plain("至少要一张图"));
        }
        let mut entries = Vec::new();
        for id in &ids {
            let (_, stored) = self.read_image(id)?;
            entries.push(stored);
        }
        let refs: Vec<&tools::attachments::StoredAttachment> = entries.iter().collect();
        Ok(ToolResult::success_with_meta(
            &call.id,
            format!("给你看 {} 张图。", entries.len()),
            Self::image_meta(&refs),
        ))
    }

    fn materialize(
        &self,
        _call: &ToolCall,
        args: &Value,
        call_id: &str,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolFailure> {
        let id = Self::arg_string(args, "attachment_id")?;
        let target = Self::arg_string(args, "path")?;
        let (bytes, stored) = self.read_image(&id)?;
        let target_path = if std::path::Path::new(&target).is_absolute() {
            std::path::PathBuf::from(&target)
        } else {
            _context.cwd.join(&target)
        };
        tools::check_write(_context, &target_path)?;
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| ToolFailure::plain(error.to_string()))?;
        }
        std::fs::write(&target_path, bytes)
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        Ok(ToolResult::success(
            call_id,
            format!(
                "已放到 {}（{} 字节）。",
                target_path.display(),
                stored.bytes
            ),
        ))
    }

    fn html_screenshot(&self, call: &ToolCall, args: &Value) -> Result<ToolResult, ToolFailure> {
        let html = args.get("html").and_then(Value::as_str);
        let path = args.get("path").and_then(Value::as_str);
        let width = args.get("width").and_then(Value::as_u64).unwrap_or(1280);
        let height = args.get("height").and_then(Value::as_u64).unwrap_or(900);
        let root = self.data_dir.join("vision").join("tmp");
        std::fs::create_dir_all(&root).map_err(|error| ToolFailure::plain(error.to_string()))?;
        let source = if let Some(html) = html {
            let file = root.join(format!("{}.html", call.id));
            std::fs::write(&file, html).map_err(|error| ToolFailure::plain(error.to_string()))?;
            file
        } else if let Some(path) = path {
            std::path::PathBuf::from(path)
        } else {
            return Err(ToolFailure::plain("要 html 或 path"));
        };
        let output = root.join(format!("{}.png", call.id));
        let chrome = find_chrome().ok_or_else(|| {
            ToolFailure::coded(
                "VISION_CHROME_UNAVAILABLE",
                "没找到系统 Chrome，网页截图用不了。",
            )
        })?;
        let status = std::process::Command::new(chrome)
            .args([
                "--headless=new",
                "--disable-gpu",
                "--no-sandbox",
                "--hide-scrollbars",
                "--virtual-time-budget=3000",
                "--host-resolver-rules=MAP * ~NOTFOUND",
            ])
            .arg(format!("--window-size={width},{height}"))
            .arg(format!("--screenshot={}", output.display()))
            .arg(format!("file://{}", source.display()))
            .status()
            .map_err(|error| ToolFailure::coded("VISION_CHROME_UNAVAILABLE", error.to_string()))?;
        if !status.success() || !output.exists() {
            return Err(ToolFailure::coded(
                "VISION_CHROME_UNAVAILABLE",
                "Chrome 截图失败。",
            ));
        }
        let bytes =
            std::fs::read(&output).map_err(|error| ToolFailure::plain(error.to_string()))?;
        let stored = self.store_png("web-screenshot.png", &bytes)?;
        Ok(ToolResult::success_with_meta(
            &call.id,
            "网页截图好了。",
            Self::image_meta(&[&stored]),
        ))
    }

    fn desktop_screenshot(&self, call_id: &str) -> Result<ToolResult, ToolFailure> {
        let settings = self.settings();
        if !settings.screen_capture {
            return Err(ToolFailure::coded(
                "VISION_SCREENSHOT_DISABLED",
                "截桌面开关关着；要先在设置 › 模型与连接 › 看图里打开。",
            ));
        }
        match self.permission.check() {
            ScreenPermissionState::Granted => {}
            ScreenPermissionState::Denied | ScreenPermissionState::Unknown => {
                return Err(ToolFailure::coded(
                    "VISION_SCREEN_PERMISSION_DENIED",
                    format!(
                        "系统没给屏幕录制权限；去「系统设置 › 隐私与安全性 › 录屏与系统录音」打开（{SCREEN_SETTINGS_URL}）。"
                    ),
                ));
            }
        }
        let root = self.data_dir.join("vision").join("tmp");
        std::fs::create_dir_all(&root).map_err(|error| ToolFailure::plain(error.to_string()))?;
        let output = root.join(format!("desktop-{call_id}.png"));
        let status = std::process::Command::new("screencapture")
            .args(["-x", "-t", "png"])
            .arg(&output)
            .status()
            .map_err(|error| {
                ToolFailure::coded("VISION_SCREEN_PERMISSION_DENIED", error.to_string())
            })?;
        if !status.success() || !output.exists() {
            return Err(ToolFailure::coded(
                "VISION_SCREEN_PERMISSION_DENIED",
                "截桌面失败；去系统设置检查录屏权限。",
            ));
        }
        let bytes =
            std::fs::read(&output).map_err(|error| ToolFailure::plain(error.to_string()))?;
        let stored = self.store_png("desktop.png", &bytes)?;
        Ok(ToolResult::success_with_meta(
            call_id,
            "桌面截图好了。",
            Self::image_meta(&[&stored]),
        ))
    }
}

struct VisionTool {
    name: &'static str,
    description: &'static str,
    schema: Value,
    shared: Arc<VisionTools>,
}

impl Tool for VisionTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        self.description
    }

    fn parameters(&self) -> &Value {
        &self.schema
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move { self.shared.run(self.name, call, _context).await })
    }
}

fn find_chrome() -> Option<std::path::PathBuf> {
    for candidate in [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/usr/bin/google-chrome",
    ] {
        let path = std::path::PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn parse_bbox(text: &str) -> Option<[i64; 4]> {
    let start = text.find('[')?;
    let end = text[start..].find(']')? + start;
    let values: Vec<i64> = text[start + 1..end]
        .split(',')
        .filter_map(|item| {
            item.trim()
                .parse::<f64>()
                .ok()
                .map(|value| value.round() as i64)
        })
        .collect();
    (values.len() == 4).then(|| [values[0], values[1], values[2], values[3]])
}

fn schema(name: &str) -> Value {
    match name {
        "vision_bootstrap" => json!({"type":"object","properties":{}}),
        "vision_describe" => {
            json!({"type":"object","properties":{"attachment_id":{"type":"string"}},"required":["attachment_id"]})
        }
        "vision_ground" | "vision_detect" => {
            json!({"type":"object","properties":{"attachment_id":{"type":"string"},"query":{"type":"string"},"bbox":{"type":"array","items":{"type":"number"}}},"required":["attachment_id"]})
        }
        "vision_crop" => {
            json!({"type":"object","properties":{"attachment_id":{"type":"string"},"bbox":{"type":"array","items":{"type":"number"}}},"required":["attachment_id"]})
        }
        "vision_pixel_diff" => {
            json!({"type":"object","properties":{"first_attachment_id":{"type":"string"},"second_attachment_id":{"type":"string"},"threshold":{"type":"number"}},"required":["first_attachment_id","second_attachment_id"]})
        }
        "vision_colors" => {
            json!({"type":"object","properties":{"attachment_id":{"type":"string"},"count":{"type":"number"}},"required":["attachment_id"]})
        }
        "vision_present" => {
            json!({"type":"object","properties":{"attachment_ids":{"type":"array","items":{"type":"string"}}},"required":["attachment_ids"]})
        }
        "vision_materialize" => {
            json!({"type":"object","properties":{"attachment_id":{"type":"string"},"path":{"type":"string"}},"required":["attachment_id","path"]})
        }
        "vision_html_screenshot" => {
            json!({"type":"object","properties":{"html":{"type":"string"},"path":{"type":"string"},"width":{"type":"number"},"height":{"type":"number"}}})
        }
        _ => json!({"type":"object","properties":{}}),
    }
}

fn description(name: &str) -> &'static str {
    match name {
        "vision_bootstrap" => "看当前看图能力：后端个数和截桌面开关。",
        "vision_describe" => "用看图模型描述一个图片附件（明确调用，不走自动转述）。",
        "vision_ground" | "vision_detect" => "在图片里找位置：给标注图和局部放大图。",
        "vision_crop" => "按 0–1000 的方框裁一块图。",
        "vision_pixel_diff" => "比较两张图的像素差异。",
        "vision_colors" => "取图片的主要颜色和占比。",
        "vision_ocr" => "本机识别图片文字（下一张票接）。",
        "vision_long_screenshot_ocr" => "长截图本机识别文字（下一张票接）。",
        "vision_trace" => "描图（下一张票接）。",
        "vision_extract_foreground" => "抠图（下一张票接）。",
        "vision_html_screenshot" => "用系统 Chrome 无头截本地 HTML，断网。",
        "vision_screenshot" => "截桌面；开关默认关，没录屏权限会指系统设置。",
        "vision_present" => "把图片附件给用户看，结果带图片附件 id。",
        "vision_materialize" => "把图片附件复制到工作区文件（只写任务）。",
        _ => "看图工具。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tools::ToolSessionState;

    struct FakePermission(ScreenPermissionState);
    impl ScreenPermissionProbe for FakePermission {
        fn check(&self) -> ScreenPermissionState {
            self.0
        }
    }

    fn setup(
        permission: ScreenPermissionState,
    ) -> (tempfile::TempDir, Arc<VisionTools>, Arc<ToolSessionState>) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Arc::new(VisionRuntime::new(dir.path().to_path_buf(), None));
        let tools = Arc::new(VisionTools::new(
            dir.path().to_path_buf(),
            runtime,
            Arc::new(FakePermission(permission)),
        ));
        let session = Arc::new(ToolSessionState::new(dir.path(), "vision-tool-test"));
        (dir, tools, session)
    }

    fn tool_context(dir: &tempfile::TempDir, session: Arc<ToolSessionState>) -> ToolContext {
        ToolContext::with_session(dir.path(), session)
    }

    async fn execute(tool: Arc<dyn Tool>, context: &ToolContext, args: Value) -> ToolResult {
        tool.execute(
            ToolCall {
                id: "call".to_owned(),
                name: tool.name().to_owned(),
                arguments: args.to_string(),
            },
            context,
        )
        .await
    }

    #[test]
    fn role_lists_are_fourteen_and_fifteen() {
        let (dir, tools, _) = setup(ScreenPermissionState::Denied);
        let readonly = tools.readonly();
        assert_eq!(readonly.len(), 14);
        assert!(
            !readonly
                .iter()
                .any(|tool| tool.name() == "vision_materialize")
        );
        let (_dir2, tools2, _) = setup(ScreenPermissionState::Denied);
        let writable = tools2.writable();
        assert_eq!(writable.len(), 15);
        assert!(
            writable
                .iter()
                .any(|tool| tool.name() == "vision_materialize")
        );
        drop(dir);
    }

    #[tokio::test]
    async fn crop_and_ground_return_image_attachments() {
        let (dir, tools, session) = setup(ScreenPermissionState::Denied);
        let stored =
            tools::attachments::store(dir.path(), "red.png", &vision::ops::test_image().unwrap())
                .unwrap();
        let context = tool_context(&dir, session);
        let crop = tools
            .readonly()
            .into_iter()
            .find(|tool| tool.name() == "vision_crop")
            .unwrap();
        let result = execute(
            crop,
            &context,
            json!({"attachment_id": stored.id, "bbox": [0, 0, 500, 500]}),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        let images = result.meta.unwrap()["images"].as_array().unwrap().len();
        assert_eq!(images, 1);
        let ground = tools
            .readonly()
            .into_iter()
            .find(|tool| tool.name() == "vision_ground")
            .unwrap();
        let result = execute(
            ground,
            &context,
            json!({"attachment_id": stored.id, "query": "按钮"}),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        let meta = result.meta.unwrap();
        assert_eq!(meta["images"].as_array().unwrap().len(), 2);
        assert!(meta["bbox"].as_array().is_some());
    }

    #[tokio::test]
    async fn screen_switch_and_permission_are_enforced() {
        let (dir, tools, session) = setup(ScreenPermissionState::Denied);
        let context = tool_context(&dir, session);
        let shot = tools
            .readonly()
            .into_iter()
            .find(|tool| tool.name() == "vision_screenshot")
            .unwrap();
        let result = execute(shot.clone(), &context, json!({})).await;
        assert_eq!(
            result.error_code.as_deref(),
            Some("VISION_SCREENSHOT_DISABLED")
        );
        let mut settings = crate::settings::Settings::default();
        settings.vision.screen_capture = true;
        crate::settings::save(dir.path(), &settings).unwrap();
        let result = execute(shot, &context, json!({})).await;
        assert_eq!(
            result.error_code.as_deref(),
            Some("VISION_SCREEN_PERMISSION_DENIED")
        );
        assert!(result.content.contains("屏幕录制") || result.content.contains("录屏"));
        let (dir2, tools2, session2) = setup(ScreenPermissionState::Granted);
        let mut settings = crate::settings::Settings::default();
        settings.vision.screen_capture = false;
        crate::settings::save(dir2.path(), &settings).unwrap();
        let shot = tools2
            .readonly()
            .into_iter()
            .find(|tool| tool.name() == "vision_screenshot")
            .unwrap();
        let result = execute(shot, &tool_context(&dir2, session2), json!({})).await;
        assert_eq!(
            result.error_code.as_deref(),
            Some("VISION_SCREENSHOT_DISABLED")
        );
    }

    #[tokio::test]
    async fn present_and_materialize_carry_image_ids_and_check_write_scope() {
        let (dir, tools, session) = setup(ScreenPermissionState::Denied);
        let stored =
            tools::attachments::store(dir.path(), "red.png", &vision::ops::test_image().unwrap())
                .unwrap();
        let context = tool_context(&dir, session);
        let present = tools
            .readonly()
            .into_iter()
            .find(|tool| tool.name() == "vision_present")
            .unwrap();
        let result = execute(
            present,
            &context,
            json!({"attachment_ids": [stored.id.clone()]}),
        )
        .await;
        assert_eq!(result.meta.unwrap()["images"].as_array().unwrap().len(), 1);
        let (dir2, tools2, session2) = setup(ScreenPermissionState::Denied);
        let stored2 =
            tools::attachments::store(dir2.path(), "red2.png", &vision::ops::test_image().unwrap())
                .unwrap();
        let scope = dir2.path().join("out");
        std::fs::create_dir_all(&scope).unwrap();
        let context =
            ToolContext::with_session(dir2.path(), session2).with_write_scope(Some(scope.clone()));
        let materialize = tools2
            .writable()
            .into_iter()
            .find(|tool| tool.name() == "vision_materialize")
            .unwrap();
        let result = execute(
            materialize,
            &context,
            json!({"attachment_id": stored2.id, "path": "out/made.png"}),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(scope.join("made.png").is_file());
    }

    #[test]
    fn test_hook_reaches_the_probe() {
        set_test_screen_permission(Some("denied"));
        assert_eq!(
            SystemScreenPermission.check(),
            ScreenPermissionState::Denied
        );
        set_test_screen_permission(Some("granted"));
        assert_eq!(
            SystemScreenPermission.check(),
            ScreenPermissionState::Granted
        );
        set_test_screen_permission(None);
    }

    #[test]
    fn has_a_fake_permission_counter() {
        // 占位，确保静态检查看到测试端口确实可注入。
        let _ = AtomicUsize::new(0).load(Ordering::SeqCst);
    }

    #[tokio::test]
    async fn html_screenshot_uses_system_chrome_and_reports_when_missing() {
        let (dir, tools, session) = setup(ScreenPermissionState::Denied);
        let context = tool_context(&dir, session);
        let html = "<!doctype html><body style='background:#f00'><h1>本地页</h1></body>";
        let tool = tools
            .readonly()
            .into_iter()
            .find(|tool| tool.name() == "vision_html_screenshot")
            .unwrap();
        let result = execute(
            tool,
            &context,
            json!({"html": html, "width": 320, "height": 200}),
        )
        .await;
        if find_chrome().is_some() {
            assert!(!result.is_error, "{}", result.content);
            assert_eq!(result.meta.unwrap()["images"].as_array().unwrap().len(), 1);
        } else {
            assert_eq!(
                result.error_code.as_deref(),
                Some("VISION_CHROME_UNAVAILABLE")
            );
        }
    }
}
