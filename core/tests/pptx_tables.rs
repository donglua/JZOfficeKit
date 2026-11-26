pub mod support;

use jz_office_core::{
    model::{Document, ElementType},
    parse_reader,
};
use std::io::Cursor;
use support::{
    package::{archive, content_types, relationships, replace, upsert, xml, TestResult},
    pptx,
};

fn document(rows: &str, columns: usize) -> TestResult<Document> {
    let mut parts = pptx::parts();
    let grid = (0..columns)
        .map(|_| r#"<a:gridCol w="3048000"/>"#)
        .collect::<String>();
    replace(
        &mut parts,
        "ppt/slides/slide2.xml",
        &format!(
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<p:cSld><p:spTree><p:graphicFrame><p:xfrm><a:off x="0" y="0"/><a:ext cx="9144000" cy="2528580"/></p:xfrm>
<a:graphic><a:graphicData><a:tbl><a:tblGrid>{grid}</a:tblGrid>{rows}</a:tbl></a:graphicData></a:graphic>
</p:graphicFrame></p:spTree></p:cSld></p:sld>"#
        ),
    );
    Ok(parse_reader(Cursor::new(archive(&parts)?))?)
}

fn cell(text: &str, properties: &str) -> String {
    format!(
        r#"<a:tc {properties}><a:txBody><a:bodyPr/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>"#
    )
}

fn table(document: &Document) -> TestResult<&jz_office_core::model::Element> {
    document.pages[0]
        .elements
        .iter()
        .find(|element| matches!(element.kind, ElementType::TABLE))
        .ok_or("PPTX table missing".into())
}

#[test]
fn horizontal_and_vertical_spans_produce_merge_ranges() -> TestResult {
    let document = document(
        &format!(
            "<a:tr h=\"360000\">{}{}{}{}</a:tr><a:tr h=\"360000\">{}{}{}{}</a:tr>",
            cell("Wide", r#"gridSpan="2""#),
            cell("Hidden H", r#"hMerge="1""#),
            cell("Tall", r#"rowSpan="2""#),
            cell("R0C3", ""),
            cell("R1C0", ""),
            cell("R1C1", ""),
            cell("Hidden V", r#"vMerge="1""#),
            cell("R1C3", ""),
        ),
        4,
    )?;
    let table = table(&document)?;

    assert_eq!(table.merges.len(), 2);
    let horizontal = table.merges[0];
    assert_eq!(
        (
            horizontal.start_row,
            horizontal.start_column,
            horizontal.end_row,
            horizontal.end_column,
        ),
        (0, 0, 0, 1)
    );
    let vertical = table.merges[1];
    assert_eq!(
        (
            vertical.start_row,
            vertical.start_column,
            vertical.end_row,
            vertical.end_column,
        ),
        (0, 2, 1, 2)
    );
    assert!(document.search("Hidden H").is_empty());
    assert!(document.search("Hidden V").is_empty());
    assert_eq!(document.search("Wide").len(), 1);
    assert!(!document
        .warnings
        .iter()
        .any(|warning| warning.contains("merged table cells")));
    Ok(())
}

#[test]
fn rectangular_merge_clears_all_covered_cells() -> TestResult {
    let document = document(
        &format!(
            "<a:tr h=\"360000\">{}{}{}</a:tr><a:tr h=\"360000\">{}{}{}</a:tr>",
            cell("Anchor", r#"gridSpan="2" rowSpan="2""#),
            cell("Covered 01", r#"hMerge="1""#),
            cell("R0C2", ""),
            cell("Covered 10", r#"vMerge="1""#),
            cell("Covered 11", r#"hMerge="1" vMerge="1""#),
            cell("R1C2", ""),
        ),
        3,
    )?;
    let table = table(&document)?;

    assert_eq!(table.merges.len(), 1);
    let merge = table.merges[0];
    assert_eq!(
        (
            merge.start_row,
            merge.start_column,
            merge.end_row,
            merge.end_column,
        ),
        (0, 0, 1, 1)
    );
    assert!(table.rows[0][1].is_empty());
    assert!(table.rows[1][0].is_empty());
    assert!(table.rows[1][1].is_empty());
    for covered in ["Covered 01", "Covered 10", "Covered 11"] {
        assert!(document.search(covered).is_empty());
    }
    Ok(())
}

#[test]
fn merge_spans_outside_the_table_are_rejected() -> TestResult {
    let result = document(
        &format!(
            "<a:tr h=\"360000\">{}{}</a:tr>",
            cell("Invalid", r#"gridSpan="3""#),
            cell("Covered", r#"hMerge="1""#),
        ),
        2,
    );
    assert!(result.is_err());
    Ok(())
}

#[test]
fn overlapping_merge_origins_are_rejected() -> TestResult {
    let result = document(
        &format!(
            "<a:tr h=\"360000\">{}{}</a:tr><a:tr h=\"360000\">{}{}</a:tr>",
            cell("Wide", r#"gridSpan="2""#),
            cell("Tall", r#"rowSpan="2""#),
            cell("R1C0", ""),
            cell("R1C1", ""),
        ),
        2,
    );
    assert!(result.is_err());
    Ok(())
}

// ---------------------------------------------------------------------------
// PPTX 表格独立边框、表样式级联、行高与内边距
// ---------------------------------------------------------------------------

/// 接收 `xml(name, body)` 的返回值并写入包，已存在则覆盖。
fn upsert_part(parts: &mut support::package::Parts, part: (&'static str, Vec<u8>)) {
    upsert(parts, part.0, part.1);
}

/// 构造一个可注入 `ppt/tableStyles.xml` 的 PPTX 包并在第 2 张幻灯片放一张表格。
///
/// `table_styles` 为 `Some` 时追加 `ppt/tableStyles.xml` 及 presentation 关系，
/// 并通过 `[Content_Types].xml` 声明其内容类型。
fn document_with_styles(
    rows: &str,
    columns: usize,
    table_styles: Option<&str>,
) -> TestResult<Document> {
    let mut parts = pptx::parts();
    let grid = (0..columns)
        .map(|_| r#"<a:gridCol w="3048000"/>"#)
        .collect::<String>();
    replace(
        &mut parts,
        "ppt/slides/slide2.xml",
        &format!(
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<p:cSld><p:spTree><p:graphicFrame><p:xfrm><a:off x="0" y="0"/><a:ext cx="9144000" cy="2528580"/></p:xfrm>
<a:graphic><a:graphicData><a:tbl><a:tblGrid>{grid}</a:tblGrid>{rows}</a:tbl></a:graphicData></a:graphic>
</p:graphicFrame></p:spTree></p:cSld></p:sld>"#
        ),
    );
    if let Some(styles) = table_styles {
        // 追加 tableStyles 部件与 presentation 关系
        upsert(
            &mut parts,
            "ppt/tableStyles.xml",
            styles.as_bytes().to_vec(),
        );
        upsert_part(
            &mut parts,
            xml(
                "ppt/_rels/presentation.xml.rels",
                &relationships(&[
                    ("rFirst", "slide", "slides/slide1.xml"),
                    ("rSecond", "slide", "slides/slide2.xml"),
                    ("rMaster", "slideMaster", "slideMasters/slideMaster1.xml"),
                    ("rTableStyles", "tableStyles", "tableStyles.xml"),
                ]),
            ),
        );
        upsert_part(&mut parts, xml(
            "[Content_Types].xml",
            &content_types(&[
                ("ppt/presentation.xml", "presentationml.presentation.main"),
                ("ppt/slides/slide1.xml", "presentationml.slide"),
                ("ppt/slides/slide2.xml", "presentationml.slide"),
                (
                    "ppt/slideLayouts/slideLayout1.xml",
                    "presentationml.slideLayout",
                ),
                (
                    "ppt/slideMasters/slideMaster1.xml",
                    "presentationml.slideMaster",
                ),
                ("ppt/theme/theme1.xml", "theme"),
                // presentationml.tableStyles 的内容类型不带 +xml 后缀，单独拼接
                ("_placeholder_", "placeholder"),
            ])
            .replace(
                r#"<Override PartName="/_placeholder_" ContentType="application/vnd.openxmlformats-officedocument.placeholder+xml"/>"#,
                r#"<Override PartName="/ppt/tableStyles.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml"/>"#,
            ),
        ));
    }
    Ok(parse_reader(Cursor::new(archive(&parts)?))?)
}

#[test]
fn cell_tc_pr_borders_and_nofill_produce_cell_borders_and_suppress_grid() -> TestResult {
    // 四边不同颜色线宽的独立边框
    let document = document_with_styles(
        r#"<a:tr h="360000"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Framed</a:t></a:r></a:p></a:txBody>
            <a:tcPr marL="0" marR="0" marT="0" marB="0">
                <a:lnT w="25400"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:lnT>
                <a:lnR w="12700"><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill></a:lnR>
                <a:lnB w="6350"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill></a:lnB>
                <a:lnL w="19050"><a:solidFill><a:srgbClr val="FFFF00"/></a:solidFill></a:lnL>
            </a:tcPr></a:tc></a:tr>"#,
        1,
        None,
    )?;
    let framed = table(&document)?;
    assert_eq!(framed.cell_borders.len(), 1, "cell borders emitted");
    let edges = framed.cell_borders[0][0];
    assert_eq!(edges[0].color, 0xffff_0000, "top color");
    assert!((edges[0].width - 2.0).abs() < 1e-4, "top width 2pt");
    assert_eq!(edges[1].color, 0xff00_ff00, "right color");
    assert!((edges[1].width - 1.0).abs() < 1e-4, "right width 1pt");
    assert_eq!(edges[2].color, 0xff00_00ff, "bottom color");
    assert!((edges[2].width - 0.5).abs() < 1e-4, "bottom width 0.5pt");
    assert_eq!(edges[3].color, 0xffff_ff00, "left color");
    assert!((edges[3].width - 1.5).abs() < 1e-4, "left width 1.5pt");

    // 四边全 noFill：仍须输出零宽 cell_borders 以阻止平台默认网格线
    let document = document_with_styles(
        r#"<a:tr h="360000"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>No border</a:t></a:r></a:p></a:txBody>
            <a:tcPr><a:lnT><a:noFill/></a:lnT><a:lnR><a:noFill/></a:lnR>
                <a:lnB><a:noFill/></a:lnB><a:lnL><a:noFill/></a:lnL></a:tcPr></a:tc></a:tr>"#,
        1,
        None,
    )?;
    let unfilled = table(&document)?;
    assert_eq!(
        unfilled.cell_borders.len(),
        1,
        "noFill borders still emitted"
    );
    for edge in &unfilled.cell_borders[0][0] {
        assert_eq!(edge.width, 0.0, "noFill edge is zero width");
    }

    // 完全未声明边框：cell_borders 保持为空，沿用平台默认网格
    let document = document_with_styles(
        r#"<a:tr h="360000"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Plain</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc></a:tr>"#,
        1,
        None,
    )?;
    let plain = table(&document)?;
    assert!(
        plain.cell_borders.is_empty(),
        "plain table has no cell borders"
    );
    Ok(())
}

#[test]
fn table_style_cascade_applies_banding_fills_borders_and_tx_style() -> TestResult {
    let styles = r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
        <a:tblStyle styleId="{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}">
            <a:wholeTbl><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill></a:fill></a:tcStyle></a:wholeTbl>
            <a:band1H><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="EEEEEE"/></a:solidFill></a:fill></a:tcStyle></a:band1H>
            <a:band2H><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="DDDDDD"/></a:solidFill></a:fill></a:tcStyle></a:band2H>
            <a:firstCol><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="CCCCCC"/></a:solidFill></a:fill></a:tcStyle></a:firstCol>
            <a:firstRow>
                <a:tcStyle>
                    <a:fill><a:solidFill><a:srgbClr val="2A9D8F"/></a:solidFill></a:fill>
                    <a:tcBdr><a:bottom><a:ln w="25400"><a:solidFill><a:srgbClr val="112233"/></a:solidFill></a:ln></a:bottom></a:tcBdr>
                </a:tcStyle>
                <a:tcTxStyle b="on"><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></a:tcTxStyle>
            </a:firstRow>
            <a:nwCell><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="E76F51"/></a:solidFill></a:fill></a:tcStyle></a:nwCell>
        </a:tblStyle>
    </a:tblStyleLst>"#;
    // 2 行 2 列，启用表头、首列与行带；第二行第二格显式 noFill，第一行第一格无填充子树
    let table_xml = r#"<a:tr h="360000">
            <a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Header</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>
            <a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Header2</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>
        </a:tr>
        <a:tr h="360000">
            <a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Body</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>
            <a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Clear</a:t></a:r></a:p></a:txBody>
                <a:tcPr><a:noFill/></a:tcPr></a:tc>
        </a:tr>"#;
    let mut parts = pptx::parts();
    let grid = r#"<a:gridCol w="4572000"/><a:gridCol w="4572000"/>"#;
    replace(
        &mut parts,
        "ppt/slides/slide2.xml",
        &format!(
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<p:cSld><p:spTree><p:graphicFrame><p:xfrm><a:off x="0" y="0"/><a:ext cx="9144000" cy="2528580"/></p:xfrm>
<a:graphic><a:graphicData><a:tbl>
<a:tblPr firstRow="1" firstCol="1" bandRow="1"><a:tableStyleId>{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}</a:tableStyleId></a:tblPr>
<a:tblGrid>{grid}</a:tblGrid>{table_xml}</a:tbl></a:graphicData></a:graphic>
</p:graphicFrame></p:spTree></p:cSld></p:sld>"#
        ),
    );
    upsert(
        &mut parts,
        "ppt/tableStyles.xml",
        styles.as_bytes().to_vec(),
    );
    upsert_part(
        &mut parts,
        xml(
            "ppt/_rels/presentation.xml.rels",
            &relationships(&[
                ("rFirst", "slide", "slides/slide1.xml"),
                ("rSecond", "slide", "slides/slide2.xml"),
                ("rMaster", "slideMaster", "slideMasters/slideMaster1.xml"),
                ("rTableStyles", "tableStyles", "tableStyles.xml"),
            ]),
        ),
    );
    upsert_part(&mut parts, xml(
        "[Content_Types].xml",
        &content_types(&[
            ("ppt/presentation.xml", "presentationml.presentation.main"),
            ("ppt/slides/slide1.xml", "presentationml.slide"),
            ("ppt/slides/slide2.xml", "presentationml.slide"),
            (
                "ppt/slideLayouts/slideLayout1.xml",
                "presentationml.slideLayout",
            ),
            ("ppt/slideMasters/slideMaster1.xml", "presentationml.slideMaster"),
            ("ppt/theme/theme1.xml", "theme"),
            ("_t", "t"),
        ])
        .replace(
            r#"<Override PartName="/_t" ContentType="application/vnd.openxmlformats-officedocument.t+xml"/>"#,
            r#"<Override PartName="/ppt/tableStyles.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml"/>"#,
        ),
    ));
    let document = parse_reader(Cursor::new(archive(&parts)?))?;
    let table = table(&document)?;

    // nwCell 最高优先覆盖 firstRow
    assert_eq!(table.cell_fills[0][0], 0xffe7_6f51, "nwCell fill wins");
    // 表头第二格取 firstRow
    assert_eq!(table.cell_fills[0][1], 0xff2a_9d8f, "firstRow fill");
    // 第二行第一格取 firstCol
    assert_eq!(table.cell_fills[1][0], 0xffcc_cccc, "firstCol fill");
    // 第二行第二格显式 noFill 清零底纹
    assert_eq!(
        table.cell_fills[1][1], 0,
        "explicit noFill clears style shading"
    );

    // 表头 tcTxStyle：粗体（母版 otherStyle 会覆盖颜色基线，故仅断言粗体）
    let header_run = &table.rows[0][1][0].runs[0];
    assert!(header_run.bold, "header run is bold from tcTxStyle");
    // 主题字体由 tcTxStyle 的 fontRef/@idx="minor" 解析而来
    assert_eq!(
        header_run.font_face, "Arial",
        "header run uses theme minorFont latin typeface"
    );

    // 表头 tcBdr 底线落到 cell_borders
    assert!(
        !table.cell_borders.is_empty(),
        "header bottom border emitted"
    );
    assert_eq!(
        table.cell_borders[0][1][2].color, 0xff11_2233,
        "header bottom border color"
    );
    Ok(())
}

#[test]
fn merged_cells_collapse_outer_borders_from_covered_edges() -> TestResult {
    // (0,0) 跨 2 列 2 行合并；四格各自声明不同的右边与下边
    let rows = r#"<a:tr h="360000">
            <a:tc gridSpan="2" rowSpan="2"><a:txBody><a:bodyPr/><a:p><a:r><a:t>Merged</a:t></a:r></a:p></a:txBody>
                <a:tcPr>
                    <a:lnR w="12700"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:lnR>
                    <a:lnB w="12700"><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill></a:lnB>
                </a:tcPr></a:tc>
            <a:tc hMerge="1"><a:txBody><a:bodyPr/><a:p/></a:txBody>
                <a:tcPr><a:lnR w="12700"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill></a:lnR></a:tcPr></a:tc>
        </a:tr>
        <a:tr h="360000">
            <a:tc vMerge="1"><a:txBody><a:bodyPr/><a:p/></a:txBody>
                <a:tcPr><a:lnB w="12700"><a:solidFill><a:srgbClr val="FFFF00"/></a:solidFill></a:lnB></a:tcPr></a:tc>
            <a:tc hMerge="1" vMerge="1"><a:txBody><a:bodyPr/><a:p/></a:txBody><a:tcPr/></a:tc>
        </a:tr>"#;
    let document = document_with_styles(rows, 2, None)?;
    let table = table(&document)?;
    assert_eq!(table.merges.len(), 1, "one merged range");
    assert!(
        !table.cell_borders.is_empty(),
        "borders emitted for merged table"
    );
    let anchor = table.cell_borders[0][0];
    // 锚点右边来自被覆盖格 (0,1) 的右边
    assert_eq!(
        anchor[1].color, 0xff00_00ff,
        "anchor right comes from covered (0,1)"
    );
    // 锚点下边来自被覆盖格 (1,0) 的下边
    assert_eq!(
        anchor[2].color, 0xffff_ff00,
        "anchor bottom comes from covered (1,0)"
    );
    Ok(())
}

#[test]
fn row_heights_and_cell_margins_output_with_drawingml_defaults() -> TestResult {
    // h="508000" EMU = 40pt；marL=0 marR=127000(10pt) marT=63500(5pt) marB=0
    let rows = r#"<a:tr h="508000">
            <a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Padded</a:t></a:r></a:p></a:txBody>
                <a:tcPr marL="0" marR="127000" marT="63500" marB="0"/></a:tc>
            <a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Default</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>
        </a:tr>"#;
    let document = document_with_styles(rows, 2, None)?;
    let table = table(&document)?;

    assert_eq!(table.row_heights.len(), 1, "rowHeights emitted");
    assert!(
        (table.row_heights[0] - 40.0).abs() < 1e-3,
        "tr/@h converts to 40pt"
    );

    assert_eq!(table.cell_paddings.len(), 1, "cellPaddings emitted");
    let explicit = table.cell_paddings[0][0];
    // [top, right, bottom, left]
    assert!((explicit[0] - 5.0).abs() < 1e-3, "marT 5pt");
    assert!((explicit[1] - 10.0).abs() < 1e-3, "marR 10pt");
    assert!((explicit[2] - 0.0).abs() < 1e-3, "marB 0");
    assert!((explicit[3] - 0.0).abs() < 1e-3, "marL 0");
    // 未声明 mar* 的单元格使用 DrawingML 默认 [3.6, 7.2, 3.6, 7.2]
    let default = table.cell_paddings[0][1];
    assert!((default[0] - 3.6).abs() < 1e-3, "default marT 3.6pt");
    assert!((default[1] - 7.2).abs() < 1e-3, "default marR 7.2pt");
    assert!((default[2] - 3.6).abs() < 1e-3, "default marB 3.6pt");
    assert!((default[3] - 7.2).abs() < 1e-3, "default marL 7.2pt");
    Ok(())
}

/// 样式里声明了 `<a:tcBdr>`、但该节未命中任何单元格（`tblPr` 未开启对应开关）时，
/// 全表实际上没有任何边框，必须保持 `cell_borders` 为空以沿用平台默认网格，
/// 否则平台层会因非空 `cell_borders` 而画不出默认框，表格视觉上变成无框表。
#[test]
fn unmatched_style_border_section_keeps_default_grid() -> TestResult {
    let styles = r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
        <a:tblStyle styleId="{9F1B0C7E-2D3A-4B58-8C1E-6A5D7E9B2C40}">
            <a:lastRow><a:tcStyle><a:tcBdr>
                <a:top><a:ln w="25400"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></a:top>
                <a:bottom><a:ln w="25400"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></a:bottom>
                <a:left><a:ln w="25400"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></a:left>
                <a:right><a:ln w="25400"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></a:right>
            </a:tcBdr></a:tcStyle></a:lastRow>
        </a:tblStyle>
    </a:tblStyleLst>"#;
    // tblPr 未启用 lastRow，故 lastRow 节不会命中任何单元格
    let rows = r#"<a:tr h="360000">
            <a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>A</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>
            <a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>B</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>
        </a:tr>"#;
    let mut parts = pptx::parts();
    let grid = r#"<a:gridCol w="4572000"/><a:gridCol w="4572000"/>"#;
    replace(
        &mut parts,
        "ppt/slides/slide2.xml",
        &format!(
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<p:cSld><p:spTree><p:graphicFrame><p:xfrm><a:off x="0" y="0"/><a:ext cx="9144000" cy="2528580"/></p:xfrm>
<a:graphic><a:graphicData><a:tbl>
<a:tblPr><a:tableStyleId>{{9F1B0C7E-2D3A-4B58-8C1E-6A5D7E9B2C40}}</a:tableStyleId></a:tblPr>
<a:tblGrid>{grid}</a:tblGrid>{rows}</a:tbl></a:graphicData></a:graphic>
</p:graphicFrame></p:spTree></p:cSld></p:sld>"#
        ),
    );
    upsert(
        &mut parts,
        "ppt/tableStyles.xml",
        styles.as_bytes().to_vec(),
    );
    upsert_part(
        &mut parts,
        xml(
            "ppt/_rels/presentation.xml.rels",
            &relationships(&[
                ("rFirst", "slide", "slides/slide1.xml"),
                ("rSecond", "slide", "slides/slide2.xml"),
                ("rMaster", "slideMaster", "slideMasters/slideMaster1.xml"),
                ("rTableStyles", "tableStyles", "tableStyles.xml"),
            ]),
        ),
    );
    upsert_part(
        &mut parts,
        xml(
            "[Content_Types].xml",
            &content_types(&[
                ("ppt/presentation.xml", "presentationml.presentation.main"),
                ("ppt/slides/slide1.xml", "presentationml.slide"),
                ("ppt/slides/slide2.xml", "presentationml.slide"),
                (
                    "ppt/slideLayouts/slideLayout1.xml",
                    "presentationml.slideLayout",
                ),
                (
                    "ppt/slideMasters/slideMaster1.xml",
                    "presentationml.slideMaster",
                ),
                ("ppt/theme/theme1.xml", "theme"),
                ("_t", "t"),
            ])
            .replace(
                r#"<Override PartName="/_t" ContentType="application/vnd.openxmlformats-officedocument.t+xml"/>"#,
                r#"<Override PartName="/ppt/tableStyles.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml"/>"#,
            ),
        ),
    );
    let document = parse_reader(Cursor::new(archive(&parts)?))?;
    let plain = table(&document)?;
    assert!(
        plain.cell_borders.is_empty(),
        "unmatched tcBdr section must keep cell_borders empty so the platform draws its default grid"
    );
    Ok(())
}

#[test]
fn unsupported_table_border_extras_emit_targeted_warnings() -> TestResult {
    let rows = r#"<a:tr h="360000">
            <a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Diag</a:t></a:r></a:p></a:txBody>
                <a:tcPr marL="12700">
                    <a:lnL w="12700"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:lnL>
                    <a:lnTlToBr w="12700"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:lnTlToBr>
                    <a:cell3D/>
                </a:tcPr></a:tc>
        </a:tr>"#;
    let document = document_with_styles(rows, 1, None)?;
    let warnings = &document.warnings;
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("diagonal cell borders and 3D effects are omitted")),
        "diagonal and 3D warning emitted: {warnings:?}"
    );
    // 已落地的能力不再产生旧的泛化告警
    assert!(
        !warnings
            .iter()
            .any(|w| w.contains("cell borders and margins are simplified")),
        "old border/margin warning removed: {warnings:?}"
    );
    assert!(
        !warnings
            .iter()
            .any(|w| w.contains("table styles are simplified")),
        "old table style warning removed: {warnings:?}"
    );
    Ok(())
}
