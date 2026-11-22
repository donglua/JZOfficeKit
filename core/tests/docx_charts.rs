pub mod support;

use jz_office_core::{
    model::{Document, ElementType, PathCommand, WrapMode},
    parse_reader,
};
use std::io::Cursor;
use support::package::{archive, content_types, relationships, xml, Parts, TestResult};

const CHART_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";

fn theme_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office">
  <a:themeElements>
    <a:clrScheme name="Custom">
      <a:dk1><a:srgbClr val="112233"/></a:dk1>
      <a:lt1><a:srgbClr val="FFFFFF"/></a:lt1>
      <a:dk2><a:srgbClr val="445566"/></a:dk2>
      <a:lt2><a:srgbClr val="EEEEEE"/></a:lt2>
      <a:accent1><a:srgbClr val="2266AA"/></a:accent1>
      <a:accent2><a:srgbClr val="DD5522"/></a:accent2>
      <a:accent3><a:srgbClr val="339966"/></a:accent3>
      <a:accent4><a:srgbClr val="9955BB"/></a:accent4>
      <a:accent5><a:srgbClr val="CCAA22"/></a:accent5>
      <a:accent6><a:srgbClr val="5588AA"/></a:accent6>
    </a:clrScheme>
    <a:fmtScheme name="Office">
      <a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>
      <a:lnStyleLst><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>
    </a:fmtScheme>
  </a:themeElements>
</a:theme>"#
}

fn bar_chart_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"
              xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <c:date1904 val="0"/>
  <c:chart>
    <c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>DOCX Sales</a:t></a:r></a:p></c:rich></c:tx></c:title>
    <c:plotArea>
      <c:layout/>
      <c:barChart>
        <c:barDir val="col"/><c:grouping val="clustered"/>
        <c:ser>
          <c:idx val="0"/><c:order val="0"/>
          <c:tx><c:v>Revenue</c:v></c:tx>
          <c:cat><c:strLit><c:ptCount val="2"/><c:pt idx="0"><c:v>Q1</c:v></c:pt><c:pt idx="1"><c:v>Q2</c:v></c:pt></c:strLit></c:cat>
          <c:val><c:numLit><c:ptCount val="2"/><c:pt idx="0"><c:v>10</c:v></c:pt><c:pt idx="1"><c:v>25</c:v></c:pt></c:numLit></c:val>
        </c:ser>
        <c:axId val="100"/><c:axId val="200"/>
      </c:barChart>
      <c:catAx><c:axId val="100"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:tickLblPos val="nextTo"/><c:crossAx val="200"/></c:catAx>
      <c:valAx><c:axId val="200"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:tickLblPos val="nextTo"/><c:crossAx val="100"/></c:valAx>
    </c:plotArea>
    <c:legend><c:legendPos val="b"/></c:legend>
  </c:chart>
</c:chartSpace>"#
}

fn line_chart_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"
              xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <c:date1904 val="0"/>
  <c:chart>
    <c:plotArea>
      <c:layout/>
      <c:lineChart>
        <c:grouping val="standard"/>
        <c:ser>
          <c:idx val="0"/><c:order val="0"/>
          <c:tx><c:v>Trend</c:v></c:tx>
          <c:spPr><a:ln w="25400"><a:solidFill><a:srgbClr val="C03050"/></a:solidFill></a:ln></c:spPr>
          <c:cat><c:strLit><c:ptCount val="3"/><c:pt idx="0"><c:v>Jan</c:v></c:pt><c:pt idx="1"><c:v>Feb</c:v></c:pt><c:pt idx="2"><c:v>Mar</c:v></c:pt></c:strLit></c:cat>
          <c:val><c:numLit><c:ptCount val="3"/><c:pt idx="0"><c:v>10</c:v></c:pt><c:pt idx="1"><c:v>30</c:v></c:pt><c:pt idx="2"><c:v>15</c:v></c:pt></c:numLit></c:val>
        </c:ser>
        <c:axId val="100"/><c:axId val="200"/>
      </c:lineChart>
      <c:catAx><c:axId val="100"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:tickLblPos val="nextTo"/><c:crossAx val="200"/></c:catAx>
      <c:valAx><c:axId val="200"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:tickLblPos val="nextTo"/><c:crossAx val="100"/></c:valAx>
    </c:plotArea>
  </c:chart>
</c:chartSpace>"#
}

fn build_docx(
    body: &str,
    extra_parts: Vec<(&'static str, Vec<u8>)>,
    doc_rels: &[(&str, &str, &str)],
) -> TestResult<Document> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
            xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
            xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
  <w:body>
    {body}
    <w:sectPr><w:pgSz w:w="11900" w:h="16840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr>
  </w:body>
</w:document>"#
    );
    let mut rels = vec![("rTheme", "theme", "theme/theme1.xml")];
    rels.extend_from_slice(doc_rels);
    let mut parts: Parts = vec![
        xml(
            "[Content_Types].xml",
            &content_types(&[
                ("word/document.xml", "wordprocessingml.document.main"),
                ("word/theme/theme1.xml", "theme"),
            ]),
        ),
        xml(
            "_rels/.rels",
            &relationships(&[("rOffice", "officeDocument", "word/document.xml")]),
        ),
        xml("word/document.xml", &document),
        xml("word/_rels/document.xml.rels", &relationships(&rels)),
        xml("word/theme/theme1.xml", theme_xml()),
    ];
    parts.extend(extra_parts);
    Ok(parse_reader(Cursor::new(archive(&parts)?))?)
}

#[test]
fn docx_inline_column_chart_renders_bars_and_respects_content_margins() -> TestResult {
    let body = r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0">
      <wp:extent cx="3810000" cy="2286000"/>
      <wp:docPr id="1" name="Chart 1"/>
      <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart">
        <c:chart r:id="rChart"/>
      </a:graphicData></a:graphic>
    </wp:inline></w:drawing></w:r></w:p>"#;

    let doc = build_docx(
        body,
        vec![xml("word/charts/chart1.xml", bar_chart_xml())],
        &[("rChart", CHART_REL_TYPE, "charts/chart1.xml")],
    )?;

    let bars: Vec<_> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, ElementType::RECT) && b.fill == 0xff22_66aa && b.height > 12.0)
        .collect();
    assert_eq!(bars.len(), 2, "Expected 2 themed column bars in DOCX");
    // 必须从左侧页边距 (72pt) 起算，不能贴在页面 0pt 左侧边缘
    assert!(bars.iter().all(|b| b.x >= 72.0));
    assert!(bars[1].height > bars[0].height);

    let last = doc.blocks.last().ok_or("missing chart blocks")?;
    assert_eq!(last.wrap_mode, WrapMode::TopAndBottom);
    assert!(doc.blocks[..doc.blocks.len() - 1]
        .iter()
        .all(|b| b.wrap_mode == WrapMode::None));

    let labels: Vec<String> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, ElementType::TEXT))
        .map(|b| support::checks::text(&b.paragraphs))
        .collect();
    for expected in ["DOCX Sales", "Revenue", "Q1", "Q2"] {
        assert!(
            labels.iter().any(|l| l == expected),
            "Missing DOCX chart label {expected}: {labels:?}"
        );
    }
    assert!(!doc
        .warnings
        .iter()
        .any(|w| w.contains("Non-image drawings, charts and diagrams")));
    Ok(())
}

#[test]
fn docx_anchor_line_chart_normalizes_flipped_lines_for_flow_rendering() -> TestResult {
    let body = r#"<w:p><w:r><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0">
      <wp:simplePos x="0" y="0"/>
      <wp:positionH relativeFrom="column"><wp:align>left</wp:align></wp:positionH>
      <wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV>
      <wp:extent cx="3810000" cy="2286000"/>
      <wp:wrapTopAndBottom/>
      <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart">
        <c:chart r:id="rChart"/>
      </a:graphicData></a:graphic>
    </wp:anchor></w:drawing></w:r></w:p>"#;

    let doc = build_docx(
        body,
        vec![xml("word/charts/chart1.xml", line_chart_xml())],
        &[("rChart", CHART_REL_TYPE, "charts/chart1.xml")],
    )?;

    assert!(doc.blocks.iter().all(|b| !b.flip_h && !b.flip_v));
    let paths: Vec<_> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, ElementType::PATH) && b.stroke == 0xffc0_3050)
        .collect();
    assert_eq!(
        paths.len(),
        1,
        "Rising line segment should normalize to PATH"
    );
    assert!(matches!(
        paths[0].paths[0].commands.as_slice(),
        [PathCommand::Move([x0, y0]), PathCommand::Line([x1, y1])] if x1 > x0 && y0 > y1
    ));

    let falling_lines: Vec<_> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, ElementType::LINE) && b.stroke == 0xffc0_3050)
        .collect();
    assert_eq!(falling_lines.len(), 1);
    Ok(())
}

#[test]
fn docx_header_chart_and_missing_chart_part_handling() -> TestResult {
    let header_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
       xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
       xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
       xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
  <w:p><w:r><w:drawing><wp:inline>
    <wp:extent cx="2540000" cy="1270000"/>
    <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart">
      <c:chart r:id="rHdrChart"/>
    </a:graphicData></a:graphic>
  </wp:inline></w:drawing></w:r></w:p>
</w:hdr>"#;

    let body = r#"<w:p><w:r><w:drawing><wp:inline>
      <wp:extent cx="2540000" cy="1270000"/>
      <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart">
        <c:chart r:id="rMissingChart"/>
      </a:graphicData></a:graphic>
    </wp:inline></w:drawing></w:r></w:p>"#;
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
            xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
            xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
  <w:body>
    {body}
    <w:sectPr>
      <w:headerReference w:type="default" r:id="rHdr"/>
      <w:pgSz w:w="11900" w:h="16840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/>
    </w:sectPr>
  </w:body>
</w:document>"#
    );
    let header_rel_type =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
    let parts: Parts = vec![
        xml(
            "[Content_Types].xml",
            &content_types(&[
                ("word/document.xml", "wordprocessingml.document.main"),
                ("word/theme/theme1.xml", "theme"),
            ]),
        ),
        xml(
            "_rels/.rels",
            &relationships(&[("rOffice", "officeDocument", "word/document.xml")]),
        ),
        xml("word/document.xml", &document),
        xml(
            "word/_rels/document.xml.rels",
            &relationships(&[
                ("rTheme", "theme", "theme/theme1.xml"),
                ("rHdr", header_rel_type, "header1.xml"),
            ]),
        ),
        xml("word/theme/theme1.xml", theme_xml()),
        xml("word/header1.xml", header_xml),
        xml(
            "word/_rels/header1.xml.rels",
            &relationships(&[("rHdrChart", CHART_REL_TYPE, "charts/chart1.xml")]),
        ),
        xml("word/charts/chart1.xml", bar_chart_xml()),
    ];
    let doc = parse_reader(Cursor::new(archive(&parts)?))?;
    assert!(doc.blocks.iter().all(|b| {
        matches!(b.kind, ElementType::TEXT)
            && b.paragraphs
                .iter()
                .all(|p| p.runs.iter().all(|r| r.text.is_empty()))
    }));
    assert!(doc
        .header_blocks
        .iter()
        .any(|b| matches!(b.kind, ElementType::RECT) && b.fill == 0xff22_66aa));
    assert!(doc
        .warnings
        .iter()
        .any(|w| w.contains("charts with missing or external parts were omitted")));
    Ok(())
}
