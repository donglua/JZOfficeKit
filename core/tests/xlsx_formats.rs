//! XLSX 数字格式与 ISO 8601 日期单元格的端到端集成测试。
//!
//! 第一个用例覆盖货币、区域货币标签、会计格式、千分位缩放、科学计数与中文日期模板
//! 从 `styles.xml` 到 `Cell::text` 的完整链路；第二个用例覆盖 `t="d"` 单元格在
//! General 与显式样式下的渲染结果，以及非法 ISO 值保留原文并告警的回退路径。

pub mod support;

use jz_office_core::model::Document;
use jz_office_core::parse_reader;
use jz_office_core::spreadsheet::{Cell, Sheet};
use std::io::Cursor;
use support::{
    package::{archive, replace, TestResult},
    xlsx,
};

/// 测试用样式表：自定义 `numFmtId` 取 164 起的保留区段。
///
/// `cellXfs` 索引同时是单元格 `s` 属性的取值：
/// 0 = General、1 = `$#,##0.00`、2 = `[$¥-804]#,##0.00`、3 = 内置会计格式 44、
/// 4 = `#,##0.0,," M"`、5 = 内置科学计数 11、6 = `yyyy"年"m"月"d"日"`。
const STYLES: &str = r##"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<numFmts count="4"><numFmt numFmtId="164" formatCode="$#,##0.00"/><numFmt numFmtId="165" formatCode="[$¥-804]#,##0.00"/><numFmt numFmtId="166" formatCode="#,##0.0,,&quot; M&quot;"/><numFmt numFmtId="167" formatCode="yyyy&quot;年&quot;m&quot;月&quot;d&quot;日&quot;"/></numFmts>
<fonts count="1"><font><sz val="11"/><color rgb="FF202124"/></font></fonts>
<fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>
<borders count="1"><border/></borders>
<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
<cellXfs count="7"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="164" fontId="0" fillId="0" borderId="0"/><xf numFmtId="165" fontId="0" fillId="0" borderId="0"/><xf numFmtId="44" fontId="0" fillId="0" borderId="0"/><xf numFmtId="166" fontId="0" fillId="0" borderId="0"/><xf numFmtId="11" fontId="0" fillId="0" borderId="0"/><xf numFmtId="167" fontId="0" fillId="0" borderId="0"/></cellXfs>
</styleSheet>"##;

/// 构造一行「A 列标签 + B 列数值」，数值按 `style` 索引的格式渲染。
fn value_row(row: u32, label: &str, style: u32, value: &str) -> String {
    format!(
        r#"<row r="{row}"><c r="A{row}" t="inlineStr"><is><t>{label}</t></is></c><c r="B{row}" s="{style}"><v>{value}</v></c></row>"#
    )
}

/// 按名称取工作表。
///
/// 固件的关联关系把 `xl/worksheets/sheet2.xml` 挂在名为 "Overview" 的工作表上，
/// 因此覆盖该分部后必须按名称定位，不能假定 `document.sheets` 的下标。
fn worksheet<'a>(document: &'a Document, name: &str) -> TestResult<&'a Sheet> {
    document
        .sheets
        .iter()
        .find(|sheet| sheet.name == name)
        .ok_or_else(|| format!("缺少工作表: {name}").into())
}

/// 按行列精确定位单元格，不依赖 `Sheet::cells` 的存储顺序。
fn cell(sheet: &Sheet, row: u32, column: u32) -> TestResult<&Cell> {
    sheet
        .cells
        .iter()
        .find(|cell| cell.row == row && cell.column == column)
        .ok_or_else(|| format!("缺少第 {row} 行第 {column} 列的单元格").into())
}

/// 精确断言单元格文本；`context` 说明该用例的格式来源。
fn assert_text(sheet: &Sheet, row: u32, column: u32, expected: &str, context: &str) -> TestResult {
    assert_eq!(cell(sheet, row, column)?.text, expected, "{context}");
    Ok(())
}

#[test]
fn formats_currency_accounting_scientific_and_chinese_dates() -> TestResult {
    let mut parts = xlsx::parts();
    let rows = [
        value_row(1, "Currency", 1, "1234.5"),
        value_row(2, "Locale currency", 2, "1234.5"),
        value_row(3, "Accounting", 3, "1234.5"),
        value_row(4, "Scaled", 4, "12345000"),
        value_row(5, "Scientific", 5, "123"),
        value_row(6, "Chinese date", 6, "45292"),
    ]
    .join("");
    replace(&mut parts, "xl/styles.xml", STYLES);
    replace(
        &mut parts,
        "xl/worksheets/sheet2.xml",
        &xlsx::sheet(&format!("<sheetData>{rows}</sheetData>")),
    );
    let document = parse_reader(Cursor::new(archive(&parts)?))?;
    let sheet = worksheet(&document, "Overview")?;

    assert_text(sheet, 0, 1, "$1,234.50", "自定义货币格式 $#,##0.00")?;
    assert_text(sheet, 1, 1, "¥1,234.50", "区域货币标签 [$¥-804]")?;
    assert_text(sheet, 2, 1, "$ 1,234.50", "内置会计格式 numFmtId 44")?;
    assert_text(sheet, 3, 1, "12.3 M", "千分位缩放 ,, 与字面量后缀")?;
    assert_text(sheet, 4, 1, "1.23E+02", "内置科学计数 numFmtId 11")?;
    assert_text(sheet, 5, 1, "2024年1月1日", "中文日期模板")?;
    assert!(
        !document
            .warnings
            .iter()
            .any(|warning| warning.contains("Unsupported number format")),
        "受支持的格式不应产生未支持告警: {:?}",
        document.warnings
    );
    Ok(())
}

#[test]
fn formats_iso_date_cells_with_general_and_explicit_styles() -> TestResult {
    let mut parts = xlsx::parts();
    let rows = [
        r#"<row r="1"><c r="A1" s="0" t="d"><v>2024-01-15</v></c></row>"#,
        r#"<row r="2"><c r="A2" s="0" t="d"><v>2024-01-15T14:30:00</v></c></row>"#,
        r#"<row r="3"><c r="A3" s="6" t="d"><v>2024-01-15</v></c></row>"#,
        r#"<row r="4"><c r="A4" s="0" t="d"><v>2024-01-15T18:00:00Z</v></c></row>"#,
        r#"<row r="5"><c r="A5" s="0" t="d"><v>2024-02-30</v></c></row>"#,
    ]
    .join("");
    replace(&mut parts, "xl/styles.xml", STYLES);
    replace(
        &mut parts,
        "xl/worksheets/sheet2.xml",
        &xlsx::sheet(&format!("<sheetData>{rows}</sheetData>")),
    );
    let document = parse_reader(Cursor::new(archive(&parts)?))?;
    let sheet = worksheet(&document, "Overview")?;

    // General 样式按是否含时间选择默认模板，并标记为数值单元格以便右对齐
    assert_text(sheet, 0, 0, "2024-01-15", "General 纯日期")?;
    assert!(
        cell(sheet, 0, 0)?.numeric,
        "General 纯日期应标记为数值单元格"
    );
    assert_text(sheet, 1, 0, "2024-01-15 14:30:00", "General 日期时间")?;
    assert!(
        cell(sheet, 1, 0)?.numeric,
        "General 日期时间应标记为数值单元格"
    );
    // 显式中文日期样式覆盖 General 默认模板
    assert_text(sheet, 2, 0, "2024年1月15日", "显式中文日期样式")?;
    assert!(cell(sheet, 2, 0)?.numeric, "显式样式日期应标记为数值单元格");
    // 带 `Z` 后缀时区直接取墙上时间
    assert_text(sheet, 3, 0, "2024-01-15 18:00:00", "带 UTC 时区后缀")?;
    assert!(cell(sheet, 3, 0)?.numeric, "带时区日期应标记为数值单元格");

    // 非法 ISO 值保留原文、不标记数值，并给出专用告警
    assert_text(sheet, 4, 0, "2024-02-30", "非法 ISO 值保留原文")?;
    assert!(
        !cell(sheet, 4, 0)?.numeric,
        "非法 ISO 值不应标记为数值单元格"
    );
    assert!(
        document
            .warnings
            .iter()
            .any(|warning| warning.contains("Invalid ISO date cell; raw value retained")),
        "非法 ISO 值应给出保留原文的告警: {:?}",
        document.warnings
    );
    assert!(
        !document.warnings.iter().any(|warning| warning
            .contains("ISO date cells are displayed as stored without number formatting")),
        "已接入格式化后不应再出现旧的 ISO 告警: {:?}",
        document.warnings
    );
    Ok(())
}
