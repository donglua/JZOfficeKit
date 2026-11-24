pub mod support;

use jz_office_core::{
    metadata_path_to_json, model::Kind, parse_path, parse_path_scoped_to_json, search_path, Error,
    Scope,
};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use support::package::{archive, TestResult};

#[test]
fn parses_standard_rfc4180_csv_with_quotes_newlines_and_numbers() -> TestResult {
    let csv = concat!(
        "\u{FEFF}Name,Score,Note,Code\r\n",
        "\"Alice, Jr.\",98.5,\"Line 1\r\nLine 2\",00123\r\n",
        "\"Bob \"\"B\"\"\",\"1,234.50\",85.5%,0\r\n",
    );
    let file = TemporaryFile::new("sales_report.csv", csv.as_bytes())?;

    let doc = parse_path(&file.path)?;

    assert!(matches!(doc.kind, Kind::XLSX));
    assert_eq!(doc.schema_version, 9);
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    assert_eq!(doc.sheets.len(), 1);
    let sheet = &doc.sheets[0];
    assert_eq!(sheet.name, "sales_report");
    assert_eq!(sheet.row_heights.len(), 3);
    assert_eq!(sheet.column_widths.len(), 4);
    assert!(sheet.geometry.is_some());
    // 第 2 行有双行文本，行高应大于默认单行 20pt
    assert!(sheet.row_heights[1] > sheet.row_heights[0]);

    let cell = |r: u32, c: u32| {
        sheet
            .cells
            .iter()
            .find(|item| item.row == r && item.column == c)
    };
    assert_eq!(cell(0, 0).map(|c| c.text.as_str()), Some("Name"));
    assert_eq!(cell(1, 0).map(|c| c.text.as_str()), Some("Alice, Jr."));
    assert_eq!(cell(1, 1).map(|c| c.numeric), Some(true));
    assert_eq!(cell(1, 2).map(|c| c.text.as_str()), Some("Line 1\nLine 2"));
    assert_eq!(cell(1, 2).map(|c| c.style), Some(1));
    // 前导零编码 "00123" 不视为数字，避免右对齐误判
    assert_eq!(cell(1, 3).map(|c| c.numeric), Some(false));
    assert_eq!(cell(2, 0).map(|c| c.text.as_str()), Some("Bob \"B\""));
    assert_eq!(cell(2, 1).map(|c| c.numeric), Some(true));
    assert_eq!(cell(2, 2).map(|c| c.numeric), Some(true));
    assert_eq!(cell(2, 3).map(|c| c.numeric), Some(true));
    Ok(())
}

#[test]
fn parses_gbk_gb18030_csv_without_bom() -> TestResult {
    // 使用 GB18030 编码中文表格（国内 Excel 常见导出格式）
    let utf8_csv = "城市,销售额,备注\r\n北京,12800,华北核心区域\r\n上海,23500,华东总部\r\n";
    let (gbk_bytes, _, had_errors) = encoding_rs::GB18030.encode(utf8_csv);
    assert!(!had_errors);
    assert!(std::str::from_utf8(&gbk_bytes).is_err());

    let file = TemporaryFile::new("gbk_chinese.csv", &gbk_bytes)?;
    let doc = parse_path(&file.path)?;

    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    let sheet = &doc.sheets[0];
    let texts: Vec<&str> = sheet.cells.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "城市",
            "销售额",
            "备注",
            "北京",
            "12800",
            "华北核心区域",
            "上海",
            "23500",
            "华东总部"
        ]
    );
    Ok(())
}

#[test]
fn parses_utf16le_bom_csv_and_excel_sep_directive() -> TestResult {
    let content = "sep=;\r\n产品;单价\r\n笔记本;6999\r\n";
    let mut bytes = vec![0xFF, 0xFE];
    for unit in content.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let file = TemporaryFile::new("unicode.csv", &bytes)?;

    let doc = parse_path(&file.path)?;
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    let sheet = &doc.sheets[0];
    assert_eq!(sheet.row_heights.len(), 2);
    assert_eq!(sheet.column_widths.len(), 2);
    assert_eq!(sheet.cells[0].text, "产品");
    assert_eq!(sheet.cells[1].text, "单价");
    assert_eq!(sheet.cells[2].text, "笔记本");
    assert_eq!(sheet.cells[3].text, "6999");
    assert!(sheet.cells[3].numeric);
    Ok(())
}

#[test]
fn detects_semicolon_csv_and_parses_tsv() -> TestResult {
    let semi_file = TemporaryFile::new(
        "euro.csv",
        b"City;Revenue;Growth\nBerlin;1000;12%\nParis;2000;8%\n",
    )?;
    let semi_doc = parse_path(&semi_file.path)?;
    assert_eq!(semi_doc.sheets[0].column_widths.len(), 3);
    assert_eq!(semi_doc.sheets[0].cells[3].text, "Berlin");

    let tsv_file = TemporaryFile::new("metrics.tsv", "Key\tValue\tNote\nA,1\t42\tOK\n".as_bytes())?;
    let tsv_doc = parse_path(&tsv_file.path)?;
    assert_eq!(tsv_doc.sheets[0].name, "metrics");
    assert_eq!(tsv_doc.sheets[0].column_widths.len(), 3);
    // TSV 中逗号属于普通正文，不应被拆分
    assert_eq!(tsv_doc.sheets[0].cells[3].text, "A,1");
    assert_eq!(tsv_doc.sheets[0].cells[4].text, "42");
    assert!(tsv_doc.sheets[0].cells[4].numeric);
    Ok(())
}

#[test]
fn supports_metadata_first_viewport_search_and_empty_csv() -> TestResult {
    // 构造 120 行 CSV 验证 FirstViewport 截断行数与 Full 完整解析
    let mut big = String::from("ID,Label\n");
    for i in 1..=120 {
        big.push_str(&format!("{i},RowItem-{i}\n"));
    }
    let file = TemporaryFile::new("large_list.csv", big.as_bytes())?;

    let meta: serde_json::Value = serde_json::from_str(&metadata_path_to_json(&file.path)?)?;
    assert_eq!(meta["kind"], "XLSX");
    assert_eq!(meta["pageCount"], 1);
    assert_eq!(meta["sheetNames"], serde_json::json!(["large_list"]));

    let viewport: serde_json::Value = serde_json::from_str(&parse_path_scoped_to_json(
        &file.path,
        Scope::FirstViewport,
    )?)?;
    assert_eq!(
        viewport["sheets"][0]["rowHeights"].as_array().map(Vec::len),
        Some(64)
    );
    assert_eq!(
        viewport["warnings"].as_array().map(Vec::len),
        Some(0),
        "FirstViewport 不应产生用户告警"
    );

    let full: serde_json::Value =
        serde_json::from_str(&parse_path_scoped_to_json(&file.path, Scope::Full)?)?;
    assert_eq!(
        full["sheets"][0]["rowHeights"].as_array().map(Vec::len),
        Some(121)
    );

    // 全文检索直接命中末尾行
    let matches = search_path(&file.path, "rowitem-120")?;
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].row, Some(120));
    assert_eq!(matches[0].column, Some(1));

    // 空 CSV 应安全生成 1x1 空白表
    let empty = TemporaryFile::new("empty.csv", b"")?;
    let empty_doc = parse_path(&empty.path)?;
    assert_eq!(empty_doc.sheets[0].row_heights.len(), 1);
    assert_eq!(empty_doc.sheets[0].column_widths.len(), 1);
    Ok(())
}

#[test]
fn rejects_binary_csv_and_falls_back_when_xlsx_renamed_to_csv() -> TestResult {
    let binary = TemporaryFile::new("fake.csv", &[0x89, 0x50, 0x4E, 0x47, 0x00, 0x01, 0x02])?;
    assert!(matches!(parse_path(&binary.path), Err(Error::Invalid(_))));
    assert!(matches!(
        metadata_path_to_json(&binary.path),
        Err(Error::Invalid(_))
    ));

    // 被误重命名为 .csv 的真实 XLSX ZIP 包仍能被 OOXML 解析器识别
    let xlsx_bytes = archive(&support::xlsx::parts())?;
    let disguised = TemporaryFile::new("disguised.csv", &xlsx_bytes)?;
    let doc = parse_path(&disguised.path)?;
    assert!(matches!(doc.kind, Kind::XLSX));
    assert_eq!(doc.sheets.len(), 2);
    Ok(())
}

struct TemporaryFile {
    directory: PathBuf,
    path: PathBuf,
}

impl TemporaryFile {
    fn new(name: &str, bytes: &[u8]) -> TestResult<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "jz-office-csv-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&directory)?;
        let temporary = Self {
            path: directory.join(name),
            directory,
        };
        fs::write(&temporary.path, bytes)?;
        Ok(temporary)
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
