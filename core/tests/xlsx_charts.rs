pub mod support;

use jz_office_core::{
    model::{Document, ElementType},
    parse_reader, Error,
};
use std::io::Cursor;
use support::{
    checks,
    package::{archive, content_types, relationships, xml, Parts, TestResult},
    pptx_charts::{self, RED},
};

fn drawing(anchor: &str) -> String {
    format!(
        r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"
 xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
 xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
{anchor}
</xdr:wsDr>"#
    )
}

fn graphic_frame() -> &'static str {
    r#"<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Chart 1"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart r:id="rChart"/></a:graphicData></a:graphic></xdr:graphicFrame>"#
}

fn two_cell_anchor() -> String {
    format!(
        r#"<xdr:twoCellAnchor editAs="twoCell"><xdr:from><xdr:col>1</xdr:col><xdr:colOff>12700</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>25400</xdr:rowOff></xdr:from><xdr:to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>7</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>{}<xdr:clientData/></xdr:twoCellAnchor>"#,
        graphic_frame()
    )
}

fn parts(drawing_xml: &str) -> Parts {
    vec![
        xml(
            "[Content_Types].xml",
            &content_types(&[
                ("xl/workbook.xml", "spreadsheetml.sheet.main"),
                ("xl/worksheets/sheet1.xml", "spreadsheetml.worksheet"),
                ("xl/drawings/drawing1.xml", "drawing"),
                ("xl/charts/chart1.xml", "drawingml.chart"),
                ("xl/theme/theme1.xml", "theme"),
            ]),
        ),
        xml(
            "_rels/.rels",
            &relationships(&[("rOffice", "officeDocument", "xl/workbook.xml")]),
        ),
        xml(
            "xl/workbook.xml",
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Charts" sheetId="1" r:id="rSheet"/></sheets></workbook>"#,
        ),
        xml(
            "xl/_rels/workbook.xml.rels",
            &relationships(&[
                ("rSheet", "worksheet", "worksheets/sheet1.xml"),
                ("rTheme", "theme", "theme/theme1.xml"),
            ]),
        ),
        xml(
            "xl/theme/theme1.xml",
            r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:clrScheme><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:accent1><a:srgbClr val="2266AA"/></a:accent1></a:clrScheme></a:themeElements></a:theme>"#,
        ),
        xml(
            "xl/worksheets/sheet1.xml",
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetFormatPr defaultRowHeight="20" defaultColWidth="10"/><sheetData><row r="1" ht="20"><c r="A1" t="inlineStr"><is><t>Data</t></is></c></row><row r="2" ht="20"><c r="A2"><v>10</v></c></row></sheetData><drawing r:id="rDrawing"/></worksheet>"#,
        ),
        xml(
            "xl/worksheets/_rels/sheet1.xml.rels",
            &relationships(&[("rDrawing", "drawing", "../drawings/drawing1.xml")]),
        ),
        xml("xl/drawings/drawing1.xml", drawing_xml),
        xml(
            "xl/drawings/_rels/drawing1.xml.rels",
            &relationships(&[("rChart", "chart", "../charts/chart1.xml")]),
        ),
        xml("xl/charts/chart1.xml", &pptx_charts::category_chart()),
    ]
}

fn parse(drawing_xml: &str) -> TestResult<Document> {
    Ok(parse_reader(Cursor::new(archive(&parts(drawing_xml))?))?)
}

#[test]
fn two_cell_chart_reuses_shared_engine_and_sheet_coordinates() -> TestResult {
    let document = parse(&drawing(&two_cell_anchor()))?;
    let sheet = &document.sheets[0];
    assert!(!sheet.elements.is_empty());
    assert_eq!(sheet.row_heights.len(), 8);
    assert_eq!(sheet.column_widths.len(), 5);
    let title = sheet.elements.iter().find(|element| {
        matches!(element.kind, ElementType::TEXT)
            && checks::text(&element.paragraphs) == "Cached categories"
    });
    let title = title.ok_or("Shared chart engine did not emit the cached title")?;
    assert!(
        title.x >= 53.5 && title.y >= 42.0,
        "Title must be inside the anchor: {title:?}"
    );
    checks::near(title.transform[4], title.x);
    checks::near(title.transform[5], title.y);
    for label in ["Alpha", "Beta", "Q1"] {
        assert!(
            sheet
                .elements
                .iter()
                .any(|element| { checks::text(&element.paragraphs) == label }),
            "Missing shared chart label {label}"
        );
    }
    assert!(sheet.elements.iter().any(|element| {
        matches!(element.kind, ElementType::LINE | ElementType::PATH) && element.stroke == RED
    }));
    assert!(!document.warnings.iter().any(|warning| {
        warning == "Worksheet drawings, tables, filters and conditional formatting are not rendered"
    }));
    Ok(())
}

#[test]
fn chart_line_flips_are_baked_into_the_serialized_transform() -> TestResult {
    let document = parse(&drawing(&two_cell_anchor()))?;
    let flipped = document.sheets[0].elements.iter().find(|element| {
        matches!(element.kind, ElementType::LINE) && element.stroke == RED && element.flip_v
    });
    let flipped = flipped.ok_or("Expected a rising shared-engine line segment")?;
    checks::near(flipped.transform[3], -1.0);
    checks::near(flipped.transform[5], flipped.y + flipped.height);

    let json = serde_json::to_value(&document)?;
    let elements = json["sheets"][0]["elements"]
        .as_array()
        .ok_or("Missing serialized sheet elements")?;
    assert!(elements.iter().any(|element| {
        element["transform"][3].as_f64() == Some(-1.0) && element.get("flipV").is_none()
    }));
    Ok(())
}

#[test]
fn one_cell_and_absolute_anchors_extend_the_scrollable_sheet() -> TestResult {
    let one_cell = drawing(&format!(
        r#"<xdr:oneCellAnchor><xdr:from><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>3</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="1905000" cy="1270000"/>{}<xdr:clientData/></xdr:oneCellAnchor>"#,
        graphic_frame()
    ));
    let document = parse(&one_cell)?;
    let sheet = &document.sheets[0];
    assert!(!sheet.elements.is_empty());
    assert!(sheet.column_widths.len() > 2 && sheet.row_heights.len() > 3);
    assert!(sheet
        .elements
        .iter()
        .all(|element| element.x >= 105.0 && element.y >= 60.0));

    let absolute = drawing(&format!(
        r#"<xdr:absoluteAnchor><xdr:pos x="762000" y="508000"/><xdr:ext cx="1905000" cy="1270000"/>{}<xdr:clientData/></xdr:absoluteAnchor>"#,
        graphic_frame()
    ));
    let document = parse(&absolute)?;
    let sheet = &document.sheets[0];
    assert!(!sheet.elements.is_empty());
    assert!(sheet.column_widths.iter().sum::<f32>() >= 210.0);
    assert!(sheet.row_heights.iter().sum::<f32>() >= 140.0);
    assert!(sheet
        .elements
        .iter()
        .all(|element| element.x >= 60.0 && element.y >= 40.0));
    Ok(())
}

#[test]
fn missing_or_external_chart_parts_are_omitted_with_a_warning() -> TestResult {
    let mut parts = parts(&drawing(&two_cell_anchor()));
    let index = parts
        .iter()
        .position(|(name, _)| *name == "xl/drawings/_rels/drawing1.xml.rels")
        .ok_or("Missing drawing relationships fixture")?;
    parts[index].1 = relationships(&[("rChart", "chart", "../charts/missing.xml")]).into_bytes();
    let document = parse_reader(Cursor::new(archive(&parts)?))?;
    assert!(document.sheets[0].elements.is_empty());
    // 缺失关系省略图表时不得提前撑大工作表行列维度
    assert_eq!(document.sheets[0].row_heights.len(), 2);
    assert_eq!(document.sheets[0].column_widths.len(), 1);
    assert!(document
        .warnings
        .iter()
        .any(|warning| { warning.contains("XLSX charts with missing or external parts") }));
    Ok(())
}

#[test]
fn invalid_or_oversized_anchors_do_not_inflate_sheet_extents() -> TestResult {
    // 1) inverted twoCellAnchor (row 500 -> row 1) must be omitted without growing to 501 rows
    let inverted = drawing(&format!(
        r#"<xdr:twoCellAnchor><xdr:from><xdr:col>50</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>500</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>{}<xdr:clientData/></xdr:twoCellAnchor>"#,
        graphic_frame()
    ));
    let document = parse(&inverted)?;
    assert!(document.sheets[0].elements.is_empty());
    assert_eq!(document.sheets[0].row_heights.len(), 2);
    assert_eq!(document.sheets[0].column_widths.len(), 1);

    // 2) absoluteAnchor exceeding MAX_COLUMNS must be omitted without bloating column_widths to 256
    let oversized_abs = drawing(&format!(
        r#"<xdr:absoluteAnchor><xdr:pos x="127000000" y="0"/><xdr:ext cx="190500000" cy="1270000"/>{}<xdr:clientData/></xdr:absoluteAnchor>"#,
        graphic_frame()
    ));
    let document = parse(&oversized_abs)?;
    assert!(document.sheets[0].elements.is_empty());
    assert_eq!(document.sheets[0].row_heights.len(), 2);
    assert_eq!(document.sheets[0].column_widths.len(), 1);
    assert!(document
        .warnings
        .iter()
        .any(|w| w.contains("invalid or empty anchors")));
    Ok(())
}

#[test]
fn non_chart_drawings_warn_and_alternate_content_charts_render() -> TestResult {
    // `<xdr:pic/>` 现在会被当作受支持负载（缺关系时另发缺失图片告警），
    // 因此用未知 URI 的 graphicFrame 触发「其它未支持负载」告警。
    let xml = drawing(&format!(
        r#"<xdr:twoCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>0</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Diagram"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor>
        <mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="xdr">{}</mc:Choice><mc:Fallback/></mc:AlternateContent>"#,
        two_cell_anchor()
    ));
    let document = parse(&xml)?;
    assert!(!document.sheets[0].elements.is_empty());
    assert!(document.warnings.iter().any(|w| {
        w == "Worksheet drawings other than supported charts, pictures, and shapes are not rendered"
    }));
    Ok(())
}

#[test]
fn enforces_sheet_chart_count_and_workbook_element_budgets() -> TestResult {
    // 1) 单工作表超过 128 张图表必须拒绝
    let too_many_charts = drawing(&two_cell_anchor().repeat(129));
    assert!(matches!(
        parse_reader(Cursor::new(archive(&parts(&too_many_charts))?)),
        Err(Error::Limit("XLSX worksheet exceeds 128 charts"))
    ));

    // 2) 工作簿级 5000 元素预算（多表累计）
    let single_count = parse(&drawing(&two_cell_anchor()))?.sheets[0]
        .elements
        .len();
    assert!(single_count > 10);
    // 用多张工作表复用同一 worksheet 与 drawing（每表 <= 128 张图表）触发跨表累计 5000 元素上限
    let sheet_count = (5000 / (single_count * 128)) + 2;
    assert!(sheet_count <= 32);
    let charts_per_sheet = (5000 / (single_count * sheet_count)) + 1;
    assert!(charts_per_sheet <= 128);
    let mut parts = parts(&drawing(&two_cell_anchor().repeat(charts_per_sheet)));
    let sheets_xml = (1..=sheet_count)
        .map(|i| format!(r#"<sheet name="S{i}" sheetId="{i}" r:id="rSheet"/>"#))
        .collect::<String>();
    let wb_idx = parts
        .iter()
        .position(|(n, _)| *n == "xl/workbook.xml")
        .ok_or("missing xl/workbook.xml")?;
    parts[wb_idx].1 = format!(
        r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{sheets_xml}</sheets></workbook>"#
    )
    .into_bytes();
    assert!(matches!(
        parse_reader(Cursor::new(archive(&parts)?)),
        Err(Error::Limit("XLSX exceeds 5000 drawing elements"))
    ));
    Ok(())
}
