//! 工作表 DrawingML（图片、形状、连接线、组合图形）解析与安全预算测试。

pub mod support;

use jz_office_core::{
    model::{Document, ElementType},
    parse_reader, Error,
};
use std::io::Cursor;
use support::{
    checks,
    package::{archive, content_types, relationships, xml, Parts, TestResult, PNG},
};

const BLUE: u32 = 0xff1a73e8;

fn drawing(anchor: &str) -> String {
    format!(
        r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"
 xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
 xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"
 xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
{anchor}
</xdr:wsDr>"#
    )
}

fn two_cell(from: (u32, u32), to: (u32, u32), payload: &str) -> String {
    format!(
        r#"<xdr:twoCellAnchor editAs="twoCell"><xdr:from><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>{payload}<xdr:clientData/></xdr:twoCellAnchor>"#,
        from.0, from.1, to.0, to.1
    )
}

fn one_cell(from: (u32, u32), cx: i64, cy: i64, payload: &str) -> String {
    format!(
        r#"<xdr:oneCellAnchor><xdr:from><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="{cx}" cy="{cy}"/>{payload}<xdr:clientData/></xdr:oneCellAnchor>"#,
        from.0, from.1
    )
}

fn absolute(x: i64, y: i64, cx: i64, cy: i64, payload: &str) -> String {
    format!(
        r#"<xdr:absoluteAnchor><xdr:pos x="{x}" y="{y}"/><xdr:ext cx="{cx}" cy="{cy}"/>{payload}<xdr:clientData/></xdr:absoluteAnchor>"#
    )
}

/// 构造一个最小 `<xdr:pic>`；`extra_sp_pr` 用于注入旋转/描边，`blip` 覆盖图片引用。
fn picture(id: u32, blip: &str, src_rect: &str, extra_sp_pr: &str, xfrm: &str) -> String {
    format!(
        r#"<xdr:pic><xdr:nvPicPr><xdr:cNvPr id="{id}" name="Picture {id}"/><xdr:cNvPicPr/></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="{blip}"/>{src_rect}<a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr>{xfrm}{extra_sp_pr}</xdr:spPr></xdr:pic>"#
    )
}

fn plain_picture() -> String {
    picture(1, "rImg", "", "", "")
}

/// 群组内子图片：`Bounds::parse` 依赖子节点自身的 `spPr/a:xfrm`，必须显式给出 off/ext。
fn framed_picture(id: u32, x: i64, y: i64, cx: i64, cy: i64) -> String {
    picture(
        id,
        "rImg",
        "",
        "",
        &format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>"#),
    )
}

fn xfrm(extra: &str) -> String {
    format!(r#"<a:xfrm{extra}><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></a:xfrm>"#)
}

fn parts(drawing_xml: &str, extra: Vec<(&'static str, Vec<u8>)>) -> Parts {
    let mut parts = vec![
        xml(
            "[Content_Types].xml",
            &content_types(&[
                ("xl/workbook.xml", "spreadsheetml.sheet.main"),
                ("xl/worksheets/sheet1.xml", "spreadsheetml.worksheet"),
                ("xl/drawings/drawing1.xml", "drawing"),
                ("xl/theme/theme1.xml", "theme"),
            ]),
        ),
        xml(
            "_rels/.rels",
            &relationships(&[("rOffice", "officeDocument", "xl/workbook.xml")]),
        ),
        xml(
            "xl/workbook.xml",
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Drawings" sheetId="1" r:id="rSheet"/></sheets></workbook>"#,
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
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetFormatPr defaultRowHeight="20" defaultColWidth="10"/><sheetData><row r="1" ht="20"><c r="A1" t="inlineStr"><is><t>Data</t></is></c></row></sheetData><drawing r:id="rDrawing"/></worksheet>"#,
        ),
        xml(
            "xl/worksheets/_rels/sheet1.xml.rels",
            &relationships(&[("rDrawing", "drawing", "../drawings/drawing1.xml")]),
        ),
        xml("xl/drawings/drawing1.xml", drawing_xml),
        xml(
            "xl/drawings/_rels/drawing1.xml.rels",
            &relationships(&[("rImg", "image", "../media/image1.png")]),
        ),
    ];
    parts.extend(extra);
    parts
}

fn parse(drawing_xml: &str) -> TestResult<Document> {
    parse_with(drawing_xml, Vec::new())
}

fn parse_with(drawing_xml: &str, extra: Vec<(&'static str, Vec<u8>)>) -> TestResult<Document> {
    let mut all = parts(drawing_xml, extra);
    if !all.iter().any(|(name, _)| *name == "xl/media/image1.png") {
        all.push(("xl/media/image1.png", PNG.to_vec()));
    }
    Ok(parse_reader(Cursor::new(archive(&all)?))?)
}

fn images(document: &Document) -> Vec<&jz_office_core::model::Element> {
    document.sheets[0]
        .elements
        .iter()
        .filter(|element| matches!(element.kind, ElementType::IMAGE))
        .collect()
}

#[test]
fn renders_two_cell_one_cell_and_absolute_pictures() -> TestResult {
    for anchor in [
        two_cell((1, 1), (3, 3), &plain_picture()),
        one_cell((1, 1), 1_905_000, 952_500, &plain_picture()),
        absolute(1_270_000, 635_000, 1_905_000, 952_500, &plain_picture()),
    ] {
        let document = parse(&drawing(&anchor))?;
        let sheet = &document.sheets[0];
        let found = images(&document);
        assert_eq!(found.len(), 1, "anchor should yield exactly one image");
        assert_eq!(found[0].image.as_deref(), Some("xl/media/image1.png"));
        // 外框来自锚点换算，宽高必须为正
        assert!(found[0].width > 0.0 && found[0].height > 0.0);
        // 未声明 srcRect 时不应产生裁剪矩形
        assert!(found[0].image_bounds.is_none());
        // twoCell / oneCell 锚点应扩展工作表行列
        assert!(sheet.row_heights.len() >= 2);
        assert!(sheet.column_widths.len() >= 2);
        assert!(!document
            .warnings
            .iter()
            .any(|w| w.contains("other than supported charts")));
    }
    Ok(())
}

#[test]
fn applies_picture_rotation_flip_border_and_crop() -> TestResult {
    let fill = picture(
        1,
        "rImg",
        r#"<a:srcRect l="10000" t="0" r="10000" b="0"/>"#,
        r#"<a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:ln w="25400"><a:solidFill><a:srgbClr val="1A73E8"/></a:solidFill></a:ln>"#,
        &xfrm(r#" rot="5400000" flipH="1""#),
    );
    let document = parse(&drawing(&one_cell((1, 1), 1_270_000, 635_000, &fill)))?;
    let found = images(&document);
    assert_eq!(found.len(), 1);
    let image = found[0];
    // 旋转 90° 与 flipH 烘焙进 transform：矩阵含 90° 分量
    assert!(
        (image.transform[1] + 1.0).abs() < 1e-4,
        "flipH 必须翻转 90° 旋转矩阵的 b 分量: {:?}",
        image.transform
    );
    assert!(
        (image.transform[2] + 1.0).abs() < 1e-4,
        "90° 旋转必须保留 c 分量: {:?}",
        image.transform
    );
    // 外框描边取 spPr/a:ln 的颜色与线宽（25400 EMU = 2.0 pt）
    assert_eq!(image.stroke, BLUE);
    assert!((image.stroke_width - 2.0).abs() < 1e-3);
    // srcRect 裁剪生成非空 image_bounds，且比元素外框更宽（图源被放大）
    let Some(bounds) = image.image_bounds else {
        panic!("srcRect must produce image bounds");
    };
    assert!(bounds[2] - bounds[0] > image.width);
    Ok(())
}

#[test]
fn picture_border_resolves_the_workbook_theme() -> TestResult {
    let themed = picture(
        1,
        "rImg",
        "",
        r#"<a:ln w="12700"><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:ln>"#,
        "",
    );
    let document = parse(&drawing(&one_cell((1, 1), 1_270_000, 635_000, &themed)))?;
    let found = images(&document);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].stroke, 0xff2266aa);
    assert!((found[0].stroke_width - 1.0).abs() < 1e-3);
    Ok(())
}

#[test]
fn skips_hidden_and_missing_pictures_without_expanding_sheet() -> TestResult {
    // 1) 隐藏图片必须直接跳过，且不产生缺失图片告警
    let hidden = r#"<xdr:pic><xdr:nvPicPr><xdr:cNvPr id="1" name="Hidden" hidden="1"/><xdr:cNvPicPr/></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="rImg"/></xdr:blipFill><xdr:spPr/></xdr:pic>"#;
    let document = parse(&drawing(&two_cell((2, 2), (6, 6), hidden)))?;
    assert!(document.sheets[0].elements.is_empty());
    // 隐藏图形不计预算、不扩展行列：初始表为 1 行 1 列
    assert_eq!(document.sheets[0].row_heights.len(), 1);
    assert_eq!(document.sheets[0].column_widths.len(), 1);
    assert!(!document
        .warnings
        .iter()
        .any(|w| w == "XLSX linked or missing pictures were omitted."));

    // 2) 外链图片（关系缺失）必须告警且不扩展行列
    let linked = picture(1, "rMissing", "", "", "");
    let document = parse(&drawing(&two_cell((2, 2), (6, 6), &linked)))?;
    assert!(document.sheets[0].elements.is_empty());
    assert_eq!(document.sheets[0].row_heights.len(), 1);
    assert_eq!(document.sheets[0].column_widths.len(), 1);
    assert!(document
        .warnings
        .iter()
        .any(|w| w == "XLSX linked or missing pictures were omitted."));

    // 3) 关系存在但 ZIP 部件缺失同样告警
    let mut all = parts(
        &drawing(&one_cell((1, 1), 1_270_000, 635_000, &plain_picture())),
        Vec::new(),
    );
    all.retain(|(name, _)| *name != "xl/media/image1.png");
    let document: Document = parse_reader(Cursor::new(archive(&all)?))?;
    assert!(document.sheets[0].elements.is_empty());
    assert!(document
        .warnings
        .iter()
        .any(|w| w == "XLSX linked or missing pictures were omitted."));
    Ok(())
}

#[test]
fn warns_on_legacy_vml_drawing_and_invalid_crop() -> TestResult {
    // 无效 srcRect（左右各 60%）必须回退全图并告警
    let invalid_crop = picture(1, "rImg", r#"<a:srcRect l="60000" r="60000"/>"#, "", "");
    let mut all = parts(
        &drawing(&one_cell((1, 1), 1_270_000, 635_000, &invalid_crop)),
        Vec::new(),
    );
    // 用带 <legacyDrawing> 的工作表替换默认工作表
    let Some(sheet) = all
        .iter_mut()
        .find(|(name, _)| *name == "xl/worksheets/sheet1.xml")
    else {
        panic!("sheet part is present");
    };
    sheet.1 = br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetFormatPr defaultRowHeight="20" defaultColWidth="10"/><sheetData><row r="1" ht="20"><c r="A1" t="inlineStr"><is><t>Data</t></is></c></row></sheetData><drawing r:id="rDrawing"/><legacyDrawing r:id="rVml"/></worksheet>"#.to_vec();
    all.push(("xl/media/image1.png", PNG.to_vec()));
    let document: Document = parse_reader(Cursor::new(archive(&all)?))?;
    assert!(document
        .warnings
        .iter()
        .any(|w| w == "XLSX legacy VML drawings and comments are not supported."));
    assert!(document
        .warnings
        .iter()
        .any(|w| w == "XLSX pictures with invalid crop bounds use the full image."));
    // 无效裁剪仍应渲染完整图片（回退全图而非丢弃元素）
    let found = images(&document);
    assert_eq!(found.len(), 1);
    assert!(found[0].image_bounds.is_none());
    Ok(())
}

#[test]
fn warns_on_non_rectangular_picture_mask() -> TestResult {
    // 非矩形蒙版同时带边框时：按矩形蒙版告警且保留边框，不得误触发 PPTX 复杂几何告警
    let masked = picture(
        1,
        "rImg",
        "",
        r#"<a:prstGeom prst="cloud"><a:avLst/></a:prstGeom><a:ln w="12700"><a:solidFill><a:srgbClr val="1A73E8"/></a:solidFill></a:ln>"#,
        "",
    );
    let document = parse(&drawing(&one_cell((1, 1), 1_270_000, 635_000, &masked)))?;
    let found = images(&document);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].stroke, BLUE);
    assert!(document
        .warnings
        .iter()
        .any(|w| w == "XLSX picture masks use rectangular bounds."));
    assert!(!document
        .warnings
        .iter()
        .any(|w| w.contains("PPTX complex geometry")));
    Ok(())
}

#[test]
fn warns_when_anchor_payload_uses_alternate_fallback() -> TestResult {
    let payload = format!(
        r#"<mc:AlternateContent><mc:Choice Requires="a14">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
        plain_picture(),
        plain_picture()
    );
    let document = parse(&drawing(&one_cell((1, 1), 1_270_000, 635_000, &payload)))?;
    assert_eq!(images(&document).len(), 1);
    assert!(document
        .warnings
        .iter()
        .any(|w| w == "XLSX alternate content uses one saved representation; unsupported features may be lost."));
    Ok(())
}

/// 带线性渐变、右箭头预设几何与文本的 `<xdr:sp>`。
fn arrow_shape() -> String {
    r#"<xdr:sp><xdr:nvSpPr><xdr:cNvPr id="2" name="Arrow"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:prstGeom prst="rightArrow"><a:avLst/></a:prstGeom><a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="E53935"/></a:gs><a:gs pos="100000"><a:srgbClr val="1A73E8"/></a:gs></a:gsLst><a:lin ang="0" scaled="0"/></a:gradFill><a:ln w="12700"><a:solidFill><a:srgbClr val="112233"/></a:solidFill></a:ln></xdr:spPr><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>标注</a:t></a:r></a:p></xdr:txBody></xdr:sp>"#
        .to_owned()
}

#[test]
fn renders_shapes_with_preset_geometry_gradient_and_textbox() -> TestResult {
    let document = parse(&drawing(&two_cell((1, 1), (4, 4), &arrow_shape())))?;
    let elements = &document.sheets[0].elements;
    let shape = elements
        .iter()
        .find(|element| matches!(element.kind, ElementType::PATH))
        .ok_or("rightArrow 应编译为 PATH")?;
    assert!(!shape.paths.is_empty(), "预设几何必须产生路径");
    let gradient = shape
        .fill_gradient
        .as_ref()
        .ok_or("线性渐变必须写入 fill_gradient")?;
    assert_eq!(gradient.colors.len(), 2);
    // 局部渐变端点以元素自身宽高为参照
    assert!(gradient.points[2] > gradient.points[0]);
    assert_eq!(shape.stroke, 0xff112233);

    let text = elements
        .iter()
        .find(|element| matches!(element.kind, ElementType::TEXT))
        .ok_or("txBody 应输出独立 TEXT 元素")?;
    assert_eq!(checks::text(&text.paragraphs), "标注");
    // XLSX 形状文本默认字号 11pt（Excel 语义），而非 PPTX 的 18pt
    let size = text.paragraphs[0].runs[0].size;
    assert!((size - 11.0).abs() < 1e-3, "default font size {size}");
    Ok(())
}

#[test]
fn renders_connectors_with_dash_and_arrowheads() -> TestResult {
    let connector = r#"<xdr:cxnSp><xdr:nvCxnSpPr><xdr:cNvPr id="3" name="Connector"/><xdr:cNvCxnSpPr/></xdr:nvCxnSpPr><xdr:spPr><a:prstGeom prst="line"><a:avLst/></a:prstGeom><a:ln w="25400"><a:solidFill><a:srgbClr val="1A73E8"/></a:solidFill><a:prstDash val="dash"/><a:tailEnd type="triangle"/></a:ln></xdr:spPr></xdr:cxnSp>"#;
    let document = parse(&drawing(&two_cell((1, 1), (5, 5), connector)))?;
    let elements = &document.sheets[0].elements;
    let path = elements
        .iter()
        .find(|element| matches!(element.kind, ElementType::PATH))
        .ok_or("虚线连接线应编译为 PATH")?;
    assert!(!path.stroke_dash.is_empty(), "prstDash 应写入 stroke_dash");
    assert_eq!(path.stroke, BLUE);
    // 箭头端点由共享样式引擎生成额外子路径（主干 + 三角箭头）
    assert!(
        path.paths.len() >= 2,
        "箭头端点应产生独立子路径: {:?}",
        path.paths
    );
    Ok(())
}

#[test]
fn renders_nested_groups_and_preserves_z_order() -> TestResult {
    // 顶层群组：子群组（含文本形状）+ 子图片，验证仿射矩阵与 Z-order
    let group = format!(
        r#"<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="4" name="Group"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2540000" cy="1270000"/><a:chOff x="0" y="0"/><a:chExt cx="2540000" cy="1270000"/></a:xfrm></xdr:grpSpPr>{}{}</xdr:grpSp>"#,
        r#"<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="5" name="Inner"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/><a:chOff x="0" y="0"/><a:chExt cx="1270000" cy="635000"/></a:xfrm></xdr:grpSpPr><xdr:sp><xdr:nvSpPr><xdr:cNvPr id="6" name="InnerShape"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="E53935"/></a:solidFill></xdr:spPr><xdr:txBody><a:bodyPr/><a:p><a:r><a:t>组内文本</a:t></a:r></a:p></xdr:txBody></xdr:sp></xdr:grpSp>"#,
        &framed_picture(7, 1_270_000, 0, 1_270_000, 635_000),
    );
    let document = parse(&drawing(&one_cell((1, 1), 2_540_000, 1_270_000, &group)))?;
    let elements = &document.sheets[0].elements;
    // 群组内图片必须产出 IMAGE 且坐标落在锚点外框内
    let image = elements
        .iter()
        .find(|element| matches!(element.kind, ElementType::IMAGE))
        .ok_or("群组内图片应渲染")?;
    assert!(image.x >= 0.0 && image.y >= 0.0);
    assert!(image.x + image.width <= 200.0 + 1.0, "{image:?}");
    // 群组内形状与文本均需保留
    assert!(elements
        .iter()
        .any(|element| matches!(element.kind, ElementType::RECT | ElementType::PATH)));
    assert!(elements
        .iter()
        .any(|element| matches!(element.kind, ElementType::TEXT)
            && checks::text(&element.paragraphs) == "组内文本"));
    assert!(!document
        .warnings
        .iter()
        .any(|w| w.contains("nesting exceeds")));
    Ok(())
}

#[test]
fn preserves_z_order_across_picture_and_shape() -> TestResult {
    // 图片在前、形状在后：Sheet.elements 必须按 XML 出现顺序排列
    let anchors = format!(
        "{}{}",
        two_cell((1, 1), (3, 3), &plain_picture()),
        two_cell((2, 2), (4, 4), &arrow_shape())
    );
    let document = parse(&drawing(&anchors))?;
    let kinds: Vec<ElementType> = document.sheets[0]
        .elements
        .iter()
        .map(|element| element.kind.clone())
        .collect();
    let image_index = kinds
        .iter()
        .position(|kind| matches!(kind, ElementType::IMAGE))
        .ok_or("缺少图片元素")?;
    let path_index = kinds
        .iter()
        .position(|kind| matches!(kind, ElementType::PATH))
        .ok_or("缺少形状元素")?;
    assert!(
        image_index < path_index,
        "Z-order 必须与 XML 一致: {kinds:?}"
    );
    Ok(())
}

#[test]
fn enforces_group_depth_limit() -> TestResult {
    // 构造 17 层嵌套 grpSp，必须返回 Limit 错误
    let mut nested = String::new();
    for depth in 0..17 {
        nested.push_str(&format!(
            r#"<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="{}" name="G{depth}"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/><a:chOff x="0" y="0"/><a:chExt cx="1270000" cy="635000"/></a:xfrm></xdr:grpSpPr>"#,
            depth + 10
        ));
    }
    nested.push_str(&plain_picture());
    for _ in 0..17 {
        nested.push_str("</xdr:grpSp>");
    }
    let mut all = parts(
        &drawing(&one_cell((1, 1), 1_270_000, 635_000, &nested)),
        Vec::new(),
    );
    all.push(("xl/media/image1.png", PNG.to_vec()));
    let result = parse_reader(Cursor::new(archive(&all)?));
    assert!(
        matches!(
            result,
            Err(Error::Limit("XLSX drawing group nesting exceeds 16 levels"))
        ),
        "17 层嵌套必须被拒绝: {result:?}"
    );
    Ok(())
}

#[test]
fn alternate_content_does_not_consume_group_depth() -> TestResult {
    // 16 层是允许的最大群组深度；选择分支只是负载包装，不应被误算成第 17 层群组。
    let mut nested = String::new();
    for depth in 0..16 {
        nested.push_str(&format!(
            r#"<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="{}" name="G{depth}"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/><a:chOff x="0" y="0"/><a:chExt cx="1270000" cy="635000"/></a:xfrm></xdr:grpSpPr>"#,
            depth + 100
        ));
    }
    nested.push_str("<mc:AlternateContent><mc:Choice Requires=\"xdr\">");
    nested.push_str(&framed_picture(200, 0, 0, 1_270_000, 635_000));
    nested.push_str("</mc:Choice><mc:Fallback/></mc:AlternateContent>");
    for _ in 0..16 {
        nested.push_str("</xdr:grpSp>");
    }
    let document = parse(&drawing(&one_cell((1, 1), 1_270_000, 635_000, &nested)))?;
    assert_eq!(images(&document).len(), 1);
    Ok(())
}

/// 构造 `sheets` 张工作表的包，每张表使用同一份 drawing XML。
fn multi_sheet_parts(sheet_count: usize, drawing_xml: &str) -> Parts {
    let mut parts: Parts = vec![
        xml(
            "[Content_Types].xml",
            &content_types(&[("xl/workbook.xml", "spreadsheetml.sheet.main")]),
        ),
        xml(
            "_rels/.rels",
            &relationships(&[("rOffice", "officeDocument", "xl/workbook.xml")]),
        ),
        xml(
            "xl/theme/theme1.xml",
            r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:clrScheme><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:accent1><a:srgbClr val="2266AA"/></a:accent1></a:clrScheme></a:themeElements></a:theme>"#,
        ),
    ];
    // 动态部件名需要在进程内长期存活，测试规模有限，直接泄漏为 'static
    let leak = |value: String| -> &'static str { Box::leak(value.into_boxed_str()) };
    let mut sheet_entries = String::new();
    let mut workbook_rels: Vec<(&'static str, &'static str, &'static str)> =
        vec![("rTheme", "theme", "theme/theme1.xml")];
    for index in 1..=sheet_count {
        sheet_entries.push_str(&format!(
            r#"<sheet name="S{index}" sheetId="{index}" r:id="rSheet{index}"/>"#
        ));
        workbook_rels.push((
            leak(format!("rSheet{index}")),
            "worksheet",
            leak(format!("worksheets/sheet{index}.xml")),
        ));
        parts.push(xml(
            leak(format!("xl/worksheets/sheet{index}.xml")),
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetFormatPr defaultRowHeight="20" defaultColWidth="10"/><sheetData/><drawing r:id="rDrawing"/></worksheet>"#,
        ));
        parts.push(xml(
            leak(format!("xl/worksheets/_rels/sheet{index}.xml.rels")),
            &relationships(&[(
                "rDrawing",
                "drawing",
                leak(format!("../drawings/drawing{index}.xml")),
            )]),
        ));
        parts.push(xml(
            leak(format!("xl/drawings/drawing{index}.xml")),
            drawing_xml,
        ));
        parts.push(xml(
            leak(format!("xl/drawings/_rels/drawing{index}.xml.rels")),
            &relationships(&[("rImg", "image", "../media/image1.png")]),
        ));
    }
    parts.push(xml(
        "xl/workbook.xml",
        &format!(
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{sheet_entries}</sheets></workbook>"#
        ),
    ));
    parts.push(xml(
        "xl/_rels/workbook.xml.rels",
        &relationships(&workbook_rels),
    ));
    parts.push(("xl/media/image1.png", PNG.to_vec()));
    parts
}

#[test]
fn enforces_path_command_budget() -> TestResult {
    // 单个 custGeom 形状的路径命令上限为 10_000（`pptx::path::parse` 约束），
    // 且单份 XML 受 60_000 节点上限约束。路径命令预算在工作簿级累计，
    // 因此用 11 张工作表各承载 9_990 条命令：累计 109_890 > 100_000 必须被拒绝。
    let commands = format!(
        "<a:moveTo><a:pt x=\"0\" y=\"0\"/></a:moveTo>{}",
        "<a:lnTo><a:pt x=\"1\" y=\"1\"/></a:lnTo>".repeat(9_989)
    );
    let anchor = format!(
        r#"<xdr:twoCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>0</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:sp><xdr:nvSpPr><xdr:cNvPr id="10" name="P"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="0" t="0" r="r" b="b"/><a:pathLst><a:path w="1000" h="1000">{commands}</a:path></a:pathLst></a:custGeom><a:solidFill><a:srgbClr val="E53935"/></a:solidFill></xdr:spPr></xdr:sp><xdr:clientData/></xdr:twoCellAnchor>"#
    );
    let result = parse_reader(Cursor::new(archive(&multi_sheet_parts(
        11,
        &drawing(&anchor),
    ))?));
    assert!(
        matches!(
            result,
            Err(Error::Limit("XLSX exceeds 100000 custom path commands"))
        ),
        "路径命令超限必须被拒绝: {:?}",
        result.map(|document| document.sheets.len())
    );
    Ok(())
}

#[test]
fn drawings_count_towards_the_element_budget() -> TestResult {
    // 带 txBody 的形状每个锚点产出 2 个元素（PATH + TEXT），统一计入 5_000 元素预算。
    // 用 2 张工作表各 1_251 个锚点累计 5_004 > 5_000 必须被拒绝。
    let one = r#"<xdr:twoCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>0</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:sp><xdr:nvSpPr><xdr:cNvPr id="2" name="S"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="E53935"/></a:solidFill></xdr:spPr><xdr:txBody><a:bodyPr/><a:p><a:r><a:t>t</a:t></a:r></a:p></xdr:txBody></xdr:sp><xdr:clientData/></xdr:twoCellAnchor>"#;
    let anchors = one.repeat(1_251);
    let result = parse_reader(Cursor::new(archive(&multi_sheet_parts(
        2,
        &drawing(&anchors),
    ))?));
    assert!(
        matches!(
            result,
            Err(Error::Limit("XLSX exceeds 5000 drawing elements"))
        ),
        "超过 5000 元素必须被拒绝: {:?}",
        result.map(|document| document.sheets[0].elements.len())
    );
    Ok(())
}

#[test]
fn unsupported_payload_warns_and_alternate_content_renders() -> TestResult {
    // 未知 graphicFrame（如 SmartArt）必须收敛到统一未支持告警
    let unknown = r#"<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Diagram"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:graphic></xdr:graphicFrame>"#;
    let document = parse(&drawing(&two_cell((1, 1), (5, 5), unknown)))?;
    assert!(document.sheets[0].elements.is_empty());
    assert_eq!(document.sheets[0].row_heights.len(), 1);
    assert!(document.warnings.iter().any(|w| {
        w == "Worksheet drawings other than supported charts, pictures, and shapes are not rendered"
    }));

    // mc:AlternateContent 内的图片必须择优解包后正常渲染
    let alternate = format!(
        r#"<mc:AlternateContent><mc:Choice Requires="xdr">{}</mc:Choice><mc:Fallback/></mc:AlternateContent>"#,
        two_cell((1, 1), (3, 3), &plain_picture())
    );
    let document = parse(&drawing(&alternate))?;
    assert_eq!(images(&document).len(), 1);
    Ok(())
}

#[test]
fn group_flip_warns_but_still_renders() -> TestResult {
    let group = format!(
        r#"<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="4" name="Flipped"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm flipH="1"><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/><a:chOff x="0" y="0"/><a:chExt cx="1270000" cy="635000"/></a:xfrm></xdr:grpSpPr>{}</xdr:grpSp>"#,
        framed_picture(8, 0, 0, 1_270_000, 635_000)
    );
    let document = parse(&drawing(&one_cell((1, 1), 1_270_000, 635_000, &group)))?;
    assert!(document
        .warnings
        .iter()
        .any(|w| w == "XLSX group flips are not supported."));
    // 翻转未支持不影响其余渲染
    assert_eq!(images(&document).len(), 1);
    Ok(())
}

#[test]
fn supported_picture_mask_emits_paths_without_rectangular_warning() -> TestResult {
    let masked = picture(
        1,
        "rImg",
        "",
        r#"<a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom><a:ln w="12700"><a:solidFill><a:srgbClr val="1A73E8"/></a:solidFill></a:ln>"#,
        "",
    );
    let document = parse(&drawing(&one_cell((1, 1), 1_270_000, 1_270_000, &masked)))?;
    let found = images(&document);
    assert_eq!(found.len(), 1);
    assert!(!found[0].paths.is_empty());
    assert_eq!(found[0].stroke, BLUE);
    assert!(!document
        .warnings
        .iter()
        .any(|w| w == "XLSX picture masks use rectangular bounds."));
    Ok(())
}
