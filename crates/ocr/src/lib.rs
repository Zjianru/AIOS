//! 本机 OCR：PP-OCRv6 ONNX，纯 Rust 推理；不装系统 tesseract/Python。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pure_onnx_ocr::{OcrEngine, OcrEngineBuilder};

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    #[error("模型文件缺失：{0}")]
    Missing(String),
    #[error("模型加载失败：{0}")]
    Load(String),
    #[error("图片读取失败：{0}")]
    Read(String),
    #[error("识别失败：{0}")]
    Run(String),
}

/// 一行识别结果；坐标是原图像素。
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub text: String,
    pub confidence: f32,
    pub bbox: [i64; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrOutput {
    pub text: String,
    /// 全图行平均置信度（按字符数加权）；没有行时为 0。
    pub confidence: f32,
    pub lines: Vec<OcrLine>,
}

pub struct LocalOcr {
    engine: Arc<OcrEngine>,
}

impl LocalOcr {
    /// 模型路径来自安装器；三个文件都在才构建。
    pub fn load(det: &Path, rec: &Path, dict: &Path) -> Result<Self, OcrError> {
        for (label, path) in [("det", det), ("rec", rec), ("dict", dict)] {
            if !path.is_file() {
                return Err(OcrError::Missing(format!("{label}: {}", path.display())));
            }
        }
        let engine = OcrEngineBuilder::new()
            .det_model_path(det)
            .rec_model_path(rec)
            .dictionary_path(dict)
            .inference_threads(4)
            .build()
            .map_err(|error| OcrError::Load(error.to_string()))?;
        Ok(Self {
            engine: Arc::new(engine),
        })
    }

    pub fn recognize_bytes(&self, bytes: &[u8]) -> Result<OcrOutput, OcrError> {
        let image =
            image::load_from_memory(bytes).map_err(|error| OcrError::Read(error.to_string()))?;
        let results = self
            .engine
            .run_from_image(&image)
            .map_err(|error| OcrError::Run(error.to_string()))?;
        let mut lines = Vec::with_capacity(results.len());
        for result in results {
            let text = result.text.trim().to_owned();
            if text.is_empty() {
                continue;
            }
            let xs: Vec<f32> = result
                .bounding_box
                .exterior()
                .coords()
                .map(|coord| coord.x as f32)
                .collect();
            let ys: Vec<f32> = result
                .bounding_box
                .exterior()
                .coords()
                .map(|coord| coord.y as f32)
                .collect();
            let x = xs.iter().copied().fold(f32::INFINITY, f32::min).max(0.0) as i64;
            let y = ys.iter().copied().fold(f32::INFINITY, f32::min).max(0.0) as i64;
            let right = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max) as i64;
            let bottom = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max) as i64;
            lines.push(OcrLine {
                text,
                confidence: result.confidence,
                bbox: [x, y, (right - x).max(1), (bottom - y).max(1)],
            });
        }
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let total_chars = lines
            .iter()
            .map(|line| line.text.chars().count())
            .sum::<usize>();
        let confidence = if total_chars == 0 {
            0.0
        } else {
            lines
                .iter()
                .map(|line| line.confidence * line.text.chars().count() as f32)
                .sum::<f32>()
                / total_chars as f32
        };
        Ok(OcrOutput {
            text,
            confidence,
            lines,
        })
    }

    pub fn recognize_path(&self, path: &Path) -> Result<OcrOutput, OcrError> {
        let bytes = std::fs::read(path).map_err(|error| OcrError::Read(error.to_string()))?;
        self.recognize_bytes(&bytes)
    }
}

/// OCR 包的三个文件在版本目录里的既定相对路径。
pub const DET_FILE: &str = "det.onnx";
pub const REC_FILE: &str = "rec.onnx";
pub const DICT_FILE: &str = "dict.txt";

pub fn load_from_package_dir(dir: &Path) -> Result<LocalOcr, OcrError> {
    LocalOcr::load(
        &dir.join(DET_FILE),
        &dir.join(REC_FILE),
        &dir.join(DICT_FILE),
    )
}

/// 从 package 根目录找最新版本目录。
pub fn latest_version_dir(package_root: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(package_root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs.pop()
}
