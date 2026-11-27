//! 本机 OCR 真模型测试：需要本地已下载 PP-OCRv6 三个文件（AIOS_OCR_MODELS 指目录）。

#[test]
fn reads_mixed_chinese_english_and_numbers() {
    let Some(dir) = std::env::var_os("AIOS_OCR_MODELS").map(std::path::PathBuf::from) else {
        return;
    };
    let engine = ocr::LocalOcr::load(
        &dir.join("det.onnx"),
        &dir.join("rec.onnx"),
        &dir.join("dict.txt"),
    )
    .expect("加载本机 OCR 模型");
    let bytes = include_bytes!("../../../web/e2e/fixtures/ocr-mixed.png");
    let output = engine.recognize_bytes(bytes).expect("识别中英混排 fixture");
    eprintln!("confidence={} text={:?}", output.confidence, output.text);
    for expected in ["订单", "A-1024", "已发货", "Total", "1,280.50", "CNY"] {
        assert!(
            output.text.contains(expected),
            "缺 {expected}：{}",
            output.text
        );
    }
    assert!(output.confidence > 0.5, "置信度过低：{}", output.confidence);
}
