//! Office 结构化读、xlsx 新建与 verify（纯 Rust，无进程/无运行时）。
//!
//! - read：xlsx=calamine（sheet/单元格/公式）、docx=docx-rs（段落/表格）、
//!  pptx=zip+quick-xml（幻灯片文字/形状锚点）；结果按 05 篇 7.5 有界返回
//!  （超出给 total_units 与 read_window 再读入口），带锚点（A1/段落号/slide+shape）。
//! - create_xlsx：rust_xlsxwriter；verify_xlsx：calamine 独立读回 + 容器/部件核对。
//!
//! 调用方只见 OfficeDocument/OfficeError；不承诺版式，只给结构与文字。

use std::io::{Cursor, Read};
use std::path::Path;

use calamine::Reader as _;
use serde::{Deserialize, Serialize};

mod office_tools;
pub use office_tools::{OfficeCreateTool, OfficeReadTool};

pub const DEFAULT_WINDOW: usize = 300;
const MAX_TEXT_CHARS: usize = 200_000;

/// 文件格式（只认这三种）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfficeFormat {
    Xlsx,
    Docx,
    Pptx,
}

impl OfficeFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Xlsx => "xlsx",
            Self::Docx => "docx",
            Self::Pptx => "pptx",
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "xlsx" | "xlsm" => Some(Self::Xlsx),
            "docx" => Some(Self::Docx),
            "pptx" => Some(Self::Pptx),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OfficeError {
    #[error("读不了文件：{0}")]
    Io(#[from] std::io::Error),
    #[error("不支持的 Office 格式：{0}")]
    UnsupportedFormat(String),
    #[error("文件损坏或格式不对：{0}")]
    Corrupt(String),
    #[error("本版只支持新建 xlsx")]
    OnlyXlsxSupported,
    #[error("再读偏移超出范围：offset {offset}，总数 {total}")]
    OffsetOutOfRange { offset: usize, total: usize },
}

/// 一个单元格的值（空单元格不进结果）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CellValue {
    Text { value: String },
    Number { value: f64 },
    Bool { value: bool },
}

/// 一个非空单元格；reference 是 A1 形式。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfficeCell {
    pub reference: String,
    #[serde(flatten)]
    pub value: CellValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
}

/// 一行（1 起）非空单元格；空行不进结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfficeRow {
    /// Excel 行号，1 起。
    pub index: usize,
    pub cells: Vec<OfficeCell>,
}

/// 工作表（窗口里的行 + 全表行/列计数）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfficeSheet {
    pub name: String,
    pub rows: Vec<OfficeRow>,
    /// 全表非空行总数（不受窗口截断影响）。
    pub total_rows: usize,
}

/// docx 段落；index 1 起（空段落也算，保留段落号锚点）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfficeParagraph {
    pub index: usize,
    pub text: String,
}

/// docx 表格；anchor 例如 table-1。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfficeTable {
    pub anchor: String,
    pub rows: Vec<Vec<String>>,
}

/// pptx 里一个形状的文字；anchor 例如 slide2/Title 1。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfficeShape {
    pub name: String,
    pub anchor: String,
    pub texts: Vec<String>,
}

/// 一张幻灯片（number 1 起）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfficeSlide {
    pub number: usize,
    pub shapes: Vec<OfficeShape>,
}

/// 读出来的 Office 文档（只含有界窗口内的内容）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfficeDocument {
    pub format: OfficeFormat,
    /// 所有工作表名（不受窗口截断影响）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sheet_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sheets: Vec<OfficeSheet>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paragraphs: Vec<OfficeParagraph>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<OfficeTable>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slides: Vec<OfficeSlide>,
    /// 这次窗口从第几个单元开始（0 起）。
    pub offset: usize,
    /// 全文可见单元总数；total_units > offset+已返回时用 read_window 再读。
    pub total_units: usize,
    /// 本次窗口是否被截断。
    pub truncated: bool,
}

impl OfficeDocument {
    pub fn is_empty(&self) -> bool {
        self.total_units == 0
    }
}

/// 新建 xlsx 的一格；不写空值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CellInput {
    Text { value: String },
    Number { value: f64 },
    Bool { value: bool },
    Formula { formula: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetPlan {
    pub name: String,
    pub rows: Vec<Vec<Option<CellInput>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkbookPlan {
    pub sheets: Vec<SheetPlan>,
}

impl WorkbookPlan {
    pub fn new(sheets: Vec<SheetPlan>) -> Self {
        Self { sheets }
    }
}

/// 读文件：按扩展名认格式，默认窗口。
pub fn read(path: impl AsRef<Path>) -> Result<OfficeDocument, OfficeError> {
    read_window(path, 0, DEFAULT_WINDOW)
}

/// 读文件 + 再读窗口。
pub fn read_window(
    path: impl AsRef<Path>,
    offset: usize,
    limit: usize,
) -> Result<OfficeDocument, OfficeError> {
    let path = path.as_ref();
    let format = OfficeFormat::from_path(path)
        .ok_or_else(|| OfficeError::UnsupportedFormat(path.display().to_string()))?;
    let bytes = std::fs::read(path)?;
    read_bytes_window(&bytes, format, offset, limit)
}

/// 按格式读字节，默认窗口。
pub fn read_bytes(bytes: &[u8], format: OfficeFormat) -> Result<OfficeDocument, OfficeError> {
    read_bytes_window(bytes, format, 0, DEFAULT_WINDOW)
}

/// 按格式读字节 + 再读窗口。
pub fn read_bytes_window(
    bytes: &[u8],
    format: OfficeFormat,
    offset: usize,
    limit: usize,
) -> Result<OfficeDocument, OfficeError> {
    let parsed = parse(bytes, format)?;
    window_of(parsed, offset, limit)
}

/// 新建 xlsx（只支持 xlsx；空计划也拒）。
pub fn create_xlsx(plan: &WorkbookPlan) -> Result<Vec<u8>, OfficeError> {
    if plan.sheets.is_empty() {
        return Err(OfficeError::Corrupt(
            "workbook 至少要有一个工作表".to_owned(),
        ));
    }
    let mut workbook = rust_xlsxwriter::Workbook::new();
    for sheet in &plan.sheets {
        if sheet.name.trim().is_empty() {
            return Err(OfficeError::Corrupt("工作表名不能为空".to_owned()));
        }
        let worksheet = workbook.add_worksheet();
        worksheet
            .set_name(sheet.name.as_str())
            .map_err(|error| OfficeError::Corrupt(error.to_string()))?;
        for (row_index, row) in sheet.rows.iter().enumerate() {
            for (col_index, cell) in row.iter().enumerate() {
                let Some(cell) = cell else { continue };
                let row = row_index as u32;
                let col = col_index as u16;
                let result = match cell {
                    CellInput::Text { value } => worksheet.write_string(row, col, value.as_str()),
                    CellInput::Number { value } => worksheet.write_number(row, col, *value),
                    CellInput::Bool { value } => worksheet.write_boolean(row, col, *value),
                    CellInput::Formula { formula } => {
                        worksheet.write_formula(row, col, formula.as_str())
                    }
                };
                result.map_err(|error| OfficeError::Corrupt(error.to_string()))?;
            }
        }
    }
    workbook
        .save_to_buffer()
        .map_err(|error| OfficeError::Corrupt(error.to_string()))
}

/// xlsx 独立读回：先核容器/部件，再用 calamine 读结构与单元格。
pub fn verify_xlsx(bytes: &[u8]) -> Result<OfficeDocument, OfficeError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec()))
        .map_err(|error| OfficeError::Corrupt(error.to_string()))?;
    for required in ["[Content_Types].xml", "xl/workbook.xml"] {
        archive
            .by_name(required)
            .map_err(|_| OfficeError::Corrupt(format!("缺部件 {required}")))?;
    }
    if !archive
        .file_names()
        .flatten()
        .any(|name| name.starts_with("xl/worksheets/"))
    {
        return Err(OfficeError::Corrupt("缺工作表部件".to_owned()));
    }
    let document = read_bytes(bytes, OfficeFormat::Xlsx)?;
    if document.sheet_names.is_empty() {
        return Err(OfficeError::Corrupt("workbook 没有工作表".to_owned()));
    }
    Ok(document)
}

#[derive(Debug)]
struct Parsed {
    format: OfficeFormat,
    sheet_names: Vec<String>,
    units: Vec<Unit>,
}

#[derive(Debug, Clone)]
enum Unit {
    Row { sheet: String, row: OfficeRow },
    Paragraph(OfficeParagraph),
    Table(OfficeTable),
    Shape { slide: usize, shape: OfficeShape },
    EmptySlide(usize),
}

fn window_of(parsed: Parsed, offset: usize, limit: usize) -> Result<OfficeDocument, OfficeError> {
    let total = parsed.units.len();
    if offset > total {
        return Err(OfficeError::OffsetOutOfRange { offset, total });
    }
    let limit = limit.max(1);
    let end = (offset + limit).min(total);
    let window = &parsed.units[offset..end];
    let mut sheets: Vec<OfficeSheet> = Vec::new();
    let mut paragraphs = Vec::new();
    let mut tables = Vec::new();
    let mut slides: Vec<OfficeSlide> = Vec::new();
    for unit in window {
        match unit {
            Unit::Row { sheet, row } => {
                if let Some(target) = sheets.iter_mut().find(|item| &item.name == sheet) {
                    target.rows.push(row.clone());
                } else {
                    sheets.push(OfficeSheet {
                        name: sheet.clone(),
                        rows: vec![row.clone()],
                        total_rows: 0,
                    });
                }
            }
            Unit::Paragraph(paragraph) => paragraphs.push(paragraph.clone()),
            Unit::Table(table) => tables.push(table.clone()),
            Unit::Shape { slide, shape } => {
                match slides.iter_mut().find(|item| item.number == *slide) {
                    Some(target) => target.shapes.push(shape.clone()),
                    None => slides.push(OfficeSlide {
                        number: *slide,
                        shapes: vec![shape.clone()],
                    }),
                }
            }
            Unit::EmptySlide(number) => {
                if !slides.iter().any(|item| item.number == *number) {
                    slides.push(OfficeSlide {
                        number: *number,
                        shapes: Vec::new(),
                    });
                }
            }
        }
    }
    // 全表行计数：从完整 parsed 里按 sheet 数一遍。
    let mut totals: Vec<(String, usize)> = Vec::new();
    for unit in &parsed.units {
        if let Unit::Row { sheet, .. } = unit {
            match totals.iter_mut().find(|(name, _)| name == sheet) {
                Some((_, count)) => *count += 1,
                None => totals.push((sheet.clone(), 1)),
            }
        }
    }
    for sheet in &mut sheets {
        sheet.total_rows = totals
            .iter()
            .find(|(name, _)| name == &sheet.name)
            .map(|(_, count)| *count)
            .unwrap_or(0);
    }
    Ok(OfficeDocument {
        format: parsed.format,
        sheet_names: parsed.sheet_names,
        sheets,
        paragraphs,
        tables,
        slides,
        offset,
        total_units: total,
        truncated: end < total,
    })
}

fn parse(bytes: &[u8], format: OfficeFormat) -> Result<Parsed, OfficeError> {
    match format {
        OfficeFormat::Xlsx => parse_xlsx(bytes),
        OfficeFormat::Docx => parse_docx(bytes),
        OfficeFormat::Pptx => parse_pptx(bytes),
    }
}

fn parse_xlsx(bytes: &[u8]) -> Result<Parsed, OfficeError> {
    let mut workbook = calamine::open_workbook_auto_from_rs(Cursor::new(bytes.to_vec()))
        .map_err(|error| OfficeError::Corrupt(error.to_string()))?;
    let sheet_names = workbook.sheet_names();
    let mut units = Vec::new();
    for name in &sheet_names {
        let formulas = workbook
            .worksheet_formula(name)
            .ok()
            .map(|range| formula_map(&range));
        let range = workbook
            .worksheet_range(name)
            .map_err(|error| OfficeError::Corrupt(error.to_string()))?;
        let start = range.start().unwrap_or((0, 0));
        for (row_offset, row) in range.rows().enumerate() {
            let row_index = start.0 as usize + row_offset + 1;
            let mut cells = Vec::new();
            for (col_offset, cell) in row.iter().enumerate() {
                let col_index = start.1 as usize + col_offset;
                let Some(value) = cell_value(cell) else {
                    continue;
                };
                let reference = format!("{}{}", column_letter(col_index), row_index);
                let formula = formulas
                    .as_ref()
                    .and_then(|map| {
                        map.get(&(start.0 as usize + row_offset, col_index))
                            .cloned()
                    })
                    .filter(|formula| !formula.is_empty());
                cells.push(OfficeCell {
                    reference,
                    value,
                    formula,
                });
            }
            if !cells.is_empty() {
                units.push(Unit::Row {
                    sheet: name.clone(),
                    row: OfficeRow {
                        index: row_index,
                        cells,
                    },
                });
            }
        }
    }
    Ok(Parsed {
        format: OfficeFormat::Xlsx,
        sheet_names,
        units,
    })
}

fn formula_map(
    range: &calamine::Range<String>,
) -> std::collections::HashMap<(usize, usize), String> {
    let start = range.start().unwrap_or((0, 0));
    let mut map = std::collections::HashMap::new();
    for (row_offset, row) in range.rows().enumerate() {
        for (col_offset, formula) in row.iter().enumerate() {
            if !formula.is_empty() {
                map.insert(
                    (start.0 as usize + row_offset, start.1 as usize + col_offset),
                    formula.clone(),
                );
            }
        }
    }
    map
}

fn cell_value(cell: &calamine::Data) -> Option<CellValue> {
    match cell {
        calamine::Data::Empty => None,
        calamine::Data::String(value) => Some(CellValue::Text {
            value: truncate(value.clone()),
        }),
        calamine::Data::Int(value) => Some(CellValue::Number {
            value: *value as f64,
        }),
        calamine::Data::Float(value) => Some(CellValue::Number { value: *value }),
        calamine::Data::Bool(value) => Some(CellValue::Bool { value: *value }),
        calamine::Data::DateTime(_)
        | calamine::Data::DateTimeIso(_)
        | calamine::Data::DurationIso(_)
        | calamine::Data::Error(_) => Some(CellValue::Text {
            value: truncate(cell.to_string()),
        }),
    }
}

fn parse_docx(bytes: &[u8]) -> Result<Parsed, OfficeError> {
    let docx =
        docx_rs::read_docx(bytes).map_err(|error| OfficeError::Corrupt(error.to_string()))?;
    let mut units = Vec::new();
    let mut paragraph_index = 0usize;
    let mut table_index = 0usize;
    for child in &docx.document.children {
        match child {
            docx_rs::DocumentChild::Paragraph(paragraph) => {
                paragraph_index += 1;
                let text = paragraph_text(paragraph);
                units.push(Unit::Paragraph(OfficeParagraph {
                    index: paragraph_index,
                    text: truncate(text),
                }));
            }
            docx_rs::DocumentChild::Table(table) => {
                table_index += 1;
                let mut rows = Vec::new();
                for row in &table.rows {
                    let docx_rs::TableChild::TableRow(row) = row;
                    let mut cells = Vec::new();
                    for cell in &row.cells {
                        let docx_rs::TableRowChild::TableCell(cell) = cell;
                        let mut text = String::new();
                        for content in &cell.children {
                            if let docx_rs::TableCellContent::Paragraph(paragraph) = content {
                                text.push_str(&paragraph_text(paragraph));
                            }
                        }
                        cells.push(truncate(text));
                    }
                    rows.push(cells);
                }
                units.push(Unit::Table(OfficeTable {
                    anchor: format!("table-{table_index}"),
                    rows,
                }));
            }
            _ => {}
        }
    }
    Ok(Parsed {
        format: OfficeFormat::Docx,
        sheet_names: Vec::new(),
        units,
    })
}

fn paragraph_text(paragraph: &docx_rs::Paragraph) -> String {
    let mut text = String::new();
    for child in &paragraph.children {
        if let docx_rs::ParagraphChild::Run(run) = child {
            for run_child in &run.children {
                match run_child {
                    docx_rs::RunChild::Text(value) => text.push_str(&value.text),
                    docx_rs::RunChild::Tab(_) => text.push(' '),
                    docx_rs::RunChild::Break(_) => text.push(' '),
                    _ => {}
                }
            }
        }
    }
    text
}

fn parse_pptx(bytes: &[u8]) -> Result<Parsed, OfficeError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec()))
        .map_err(|error| OfficeError::Corrupt(error.to_string()))?;
    let mut slide_parts = Vec::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| OfficeError::Corrupt(error.to_string()))?;
        let name = entry
            .name()
            .map_err(|error| OfficeError::Corrupt(error.to_string()))?
            .into_owned();
        if let Some(number) = slide_number(&name) {
            slide_parts.push((number, name));
        }
    }
    slide_parts.sort_by_key(|(number, _)| *number);
    if slide_parts.is_empty() {
        return Err(OfficeError::Corrupt("没有幻灯片部件".to_owned()));
    }
    let mut units = Vec::new();
    for (number, name) in slide_parts {
        let mut part = archive
            .by_name(name.as_str())
            .map_err(|error| OfficeError::Corrupt(error.to_string()))?;
        let mut xml = String::new();
        part.read_to_string(&mut xml)
            .map_err(|error| OfficeError::Corrupt(error.to_string()))?;
        let shapes = slide_shapes(&xml, number)?;
        if shapes.is_empty() {
            units.push(Unit::EmptySlide(number));
        } else {
            for shape in shapes {
                units.push(Unit::Shape {
                    slide: number,
                    shape,
                });
            }
        }
    }
    Ok(Parsed {
        format: OfficeFormat::Pptx,
        sheet_names: Vec::new(),
        units,
    })
}

fn slide_number(name: &str) -> Option<usize> {
    let rest = name.strip_prefix("ppt/slides/slide")?;
    let digits = rest.strip_suffix(".xml")?;
    digits.parse::<usize>().ok()
}

fn slide_shapes(xml: &str, slide: usize) -> Result<Vec<OfficeShape>, OfficeError> {
    use quick_xml::Reader;
    use quick_xml::events::Event;
    let mut reader = Reader::from_str(xml);
    let mut shapes = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    let mut in_text = false;
    let mut text_buf = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => match event.name().as_ref() {
                "p:sp" => current = Some((String::new(), Vec::new())),
                "p:cNvPr" => {
                    if let Some((name, _)) = current.as_mut() {
                        for attribute in event.attributes().flatten() {
                            if attribute.key.as_ref() == "name" {
                                *name = attribute.value.to_string();
                            }
                        }
                    }
                }
                "a:t" => {
                    in_text = true;
                    text_buf.clear();
                }
                _ => {}
            },
            Ok(Event::Empty(event)) => {
                if event.name().as_ref() == "p:cNvPr"
                    && let Some((name, _)) = current.as_mut()
                {
                    for attribute in event.attributes().flatten() {
                        if attribute.key.as_ref() == "name" {
                            *name = attribute.value.to_string();
                        }
                    }
                }
            }
            Ok(Event::Text(text)) => {
                if in_text {
                    text_buf.push_str(&text.into_inner());
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if in_text && let Some(value) = entity_char(reference.as_ref()) {
                    text_buf.push(value);
                }
            }
            Ok(Event::CData(text)) => {
                if in_text {
                    text_buf.push_str(&text.into_inner());
                }
            }
            Ok(Event::End(event)) => match event.name().as_ref() {
                "a:t" => {
                    if in_text && let Some((_, texts)) = current.as_mut() {
                        texts.push(truncate(text_buf.clone()));
                    }
                    in_text = false;
                }
                "p:sp" => {
                    if let Some((name, texts)) = current.take() {
                        let name = if name.is_empty() {
                            format!("shape-{}", shapes.len() + 1)
                        } else {
                            name
                        };
                        shapes.push(OfficeShape {
                            anchor: format!("slide{slide}/{name}"),
                            name,
                            texts,
                        });
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => return Err(OfficeError::Corrupt(error.to_string())),
            _ => {}
        }
    }
    Ok(shapes)
}

fn entity_char(reference: &str) -> Option<char> {
    match reference {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => reference
            .strip_prefix("#x")
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .or_else(|| {
                reference
                    .strip_prefix('#')
                    .and_then(|dec| dec.parse::<u32>().ok())
            })
            .and_then(char::from_u32),
    }
}

fn truncate(value: String) -> String {
    if value.chars().count() <= MAX_TEXT_CHARS {
        return value;
    }
    value.chars().take(MAX_TEXT_CHARS).collect()
}

fn column_letter(mut index: usize) -> String {
    let mut letters = String::new();
    loop {
        letters.insert(0, (b'A' + (index % 26) as u8) as char);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    letters
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_sheet() -> WorkbookPlan {
        WorkbookPlan::new(vec![SheetPlan {
            name: "表一".to_owned(),
            rows: vec![
                vec![
                    Some(CellInput::Text {
                        value: "名字".to_owned(),
                    }),
                    Some(CellInput::Text {
                        value: "数量".to_owned(),
                    }),
                ],
                vec![
                    Some(CellInput::Text {
                        value: "苹果".to_owned(),
                    }),
                    Some(CellInput::Number { value: 3.0 }),
                ],
                vec![
                    Some(CellInput::Formula {
                        formula: "=B2*2".to_owned(),
                    }),
                    Some(CellInput::Bool { value: true }),
                ],
            ],
        }])
    }

    #[test]
    fn xlsx_create_then_read_back_and_verify_parts() {
        let bytes = create_xlsx(&plan_sheet()).expect("建 xlsx");
        let document = verify_xlsx(&bytes).expect("verify");
        assert_eq!(document.format, OfficeFormat::Xlsx);
        assert_eq!(document.sheet_names, vec!["表一"]);
        assert_eq!(document.sheets.len(), 1);
        let sheet = &document.sheets[0];
        assert_eq!(sheet.rows.len(), 3);
        assert_eq!(sheet.rows[0].index, 1);
        assert_eq!(sheet.rows[0].cells[0].reference, "A1");
        assert_eq!(
            sheet.rows[1].cells[1].value,
            CellValue::Number { value: 3.0 }
        );
        assert_eq!(sheet.rows[2].cells[0].formula.as_deref(), Some("B2*2"));
        assert_eq!(document.total_units, 3);
        assert!(!document.truncated);
    }

    #[test]
    fn xlsx_window_returns_count_and_more_entry() {
        let rows = (0..320)
            .map(|index| {
                vec![Some(CellInput::Text {
                    value: format!("第{index}行"),
                })]
            })
            .collect();
        let bytes = create_xlsx(&WorkbookPlan::new(vec![SheetPlan {
            name: "长表".to_owned(),
            rows,
        }]))
        .expect("建 xlsx");
        let first = read_bytes(&bytes, OfficeFormat::Xlsx).expect("默认窗口");
        assert_eq!(first.total_units, 320);
        assert!(first.truncated);
        assert_eq!(first.sheets[0].rows.len(), DEFAULT_WINDOW);
        assert_eq!(first.sheets[0].total_rows, 320);
        let more = read_bytes_window(&bytes, OfficeFormat::Xlsx, 300, 100).expect("再读");
        assert_eq!(more.offset, 300);
        assert_eq!(more.sheets[0].rows.len(), 20);
        assert!(!more.truncated);
        assert!(matches!(
            read_bytes_window(&bytes, OfficeFormat::Xlsx, 500, 100),
            Err(OfficeError::OffsetOutOfRange { .. })
        ));
    }

    fn docx_bytes() -> Vec<u8> {
        let table = docx_rs::Table::new(vec![docx_rs::TableRow::new(vec![
            docx_rs::TableCell::new().add_paragraph(
                docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text("格一")),
            ),
            docx_rs::TableCell::new().add_paragraph(
                docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text("格二")),
            ),
        ])]);
        let docx = docx_rs::Docx::new()
            .add_paragraph(
                docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text("第一段")),
            )
            .add_table(table);
        let mut cursor = Cursor::new(Vec::new());
        docx.build().pack(&mut cursor).expect("建 docx");
        cursor.into_inner()
    }

    #[test]
    fn docx_reads_paragraphs_and_tables() {
        let bytes = docx_bytes();
        let document = read_bytes(&bytes, OfficeFormat::Docx).expect("读 docx");
        assert_eq!(document.format, OfficeFormat::Docx);
        assert_eq!(document.paragraphs.len(), 1);
        assert_eq!(document.paragraphs[0].index, 1);
        assert_eq!(document.paragraphs[0].text, "第一段");
        assert_eq!(document.tables.len(), 1);
        assert_eq!(document.tables[0].anchor, "table-1");
        assert_eq!(document.tables[0].rows, vec![vec!["格一", "格二"]]);
    }

    fn pptx_bytes() -> Vec<u8> {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <p:cSld><p:spTree>
    <p:sp>
      <p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr/></p:nvSpPr>
      <p:txBody><a:p><a:r><a:t>第一页标题</a:t></a:r></a:p></p:txBody>
    </p:sp>
    <p:sp>
      <p:nvSpPr><p:cNvPr id="3" name="Body 2"/><p:cNvSpPr/></p:nvSpPr>
      <p:txBody><a:p><a:r><a:t>要点 &amp; 说明</a:t></a:r></a:p></p:txBody>
    </p:sp>
  </p:spTree></p:cSld>
</p:sld>"#;
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default();
            use std::io::Write as _;
            writer
                .start_file("ppt/slides/slide1.xml", options)
                .expect("zip 项");
            writer.write_all(xml.as_bytes()).expect("写 slide");
            writer.finish().expect("完成 zip");
        }
        cursor.into_inner()
    }

    #[test]
    fn pptx_reads_slide_text_and_shape_anchors() {
        let bytes = pptx_bytes();
        let document = read_bytes(&bytes, OfficeFormat::Pptx).expect("读 pptx");
        assert_eq!(document.slides.len(), 1);
        let slide = &document.slides[0];
        assert_eq!(slide.number, 1);
        assert_eq!(slide.shapes.len(), 2);
        assert_eq!(slide.shapes[0].name, "Title 1");
        assert_eq!(slide.shapes[0].anchor, "slide1/Title 1");
        assert_eq!(slide.shapes[0].texts, vec!["第一页标题"]);
        assert_eq!(slide.shapes[1].texts, vec!["要点 & 说明"]);
    }

    #[test]
    fn bad_file_is_an_explicit_error() {
        let mut bytes = create_xlsx(&plan_sheet()).expect("建 xlsx");
        bytes.truncate(20);
        assert!(matches!(
            read_bytes(&bytes, OfficeFormat::Xlsx),
            Err(OfficeError::Corrupt(_))
        ));
        let not_zip = b"not a docx";
        assert!(matches!(
            read_bytes(not_zip, OfficeFormat::Docx),
            Err(OfficeError::Corrupt(_))
        ));
    }

    #[test]
    fn unknown_extension_is_unsupported() {
        assert!(matches!(
            read("/tmp/not-office.bin"),
            Err(OfficeError::UnsupportedFormat(_)) | Err(OfficeError::Io(_))
        ));
        assert_eq!(
            OfficeFormat::from_path(Path::new("a.xlsx")),
            Some(OfficeFormat::Xlsx)
        );
        assert_eq!(
            OfficeFormat::from_path(Path::new("a.docx")),
            Some(OfficeFormat::Docx)
        );
        assert_eq!(
            OfficeFormat::from_path(Path::new("a.pptx")),
            Some(OfficeFormat::Pptx)
        );
        assert_eq!(OfficeFormat::from_path(Path::new("a.pdf")), None);
    }

    #[test]
    fn empty_workbook_plan_is_rejected() {
        assert!(matches!(
            create_xlsx(&WorkbookPlan::new(Vec::new())),
            Err(OfficeError::Corrupt(_))
        ));
    }
}
