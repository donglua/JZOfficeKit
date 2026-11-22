pub mod support;

use jz_office_core::{model::ElementType, parse_reader};
use std::io::Cursor;
use support::package::{archive, replace, xml, TestResult};

const DRAWING_REL_TYPE: &str =
    "http://schemas.microsoft.com/office/2007/relationships/diagramDrawing";
const DATA_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramData";

fn smartart_slide(frame_xfrm: &str, in_group: bool) -> String {
    let frame = format!(
        r#"<p:graphicFrame>
          <p:nvGraphicFramePr><p:cNvPr id="4" name="SmartArt"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>
          {frame_xfrm}
          <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram">
            <dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rDm"/>
          </a:graphicData></a:graphic>
        </p:graphicFrame>"#
    );
    let body = if in_group {
        format!(
            r#"<p:grpSp>
              <p:grpSpPr><a:xfrm><a:off x="127000" y="127000"/><a:ext cx="2540000" cy="1270000"/><a:chOff x="0" y="0"/><a:chExt cx="1270000" cy="635000"/></a:xfrm></p:grpSpPr>
              {frame}
            </p:grpSp>"#
        )
    } else {
        frame
    };
    format!(
        r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
          xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
          xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
          <p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{body}</p:spTree></p:cSld>
          <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>
        </p:sld>"#
    )
}

fn build_package(
    data_xml: &str,
    drawing_xml: Option<&str>,
    slide_rels: &str,
    in_group: bool,
) -> TestResult<jz_office_core::model::Document> {
    let mut parts = support::pptx::parts();
    replace(
        &mut parts,
        "ppt/slides/slide2.xml",
        &smartart_slide(
            r#"<p:xfrm><a:off x="635000" y="635000"/><a:ext cx="2540000" cy="1270000"/></p:xfrm>"#,
            in_group,
        ),
    );
    replace(&mut parts, "ppt/slides/_rels/slide2.xml.rels", slide_rels);
    parts.push(xml("ppt/diagrams/data1.xml", data_xml));
    if let Some(drawing) = drawing_xml {
        parts.push(xml("ppt/diagrams/drawing1.xml", drawing));
        parts.push(xml(
            "ppt/diagrams/_rels/drawing1.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rImg" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/pixel.png"/>
</Relationships>"#,
        ));
    }
    Ok(parse_reader(Cursor::new(archive(&parts)?))?)
}

#[test]
fn smartart_renders_shapes_arrows_and_text_from_diagram_drawing() -> TestResult {
    let data_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"
               xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram">
  <dgm:extLst><dgm:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram">
    <dsp:dataModelExt relId="rDraw"/>
  </dgm:ext></dgm:extLst>
</dgm:dataModel>"#;
    let drawing_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dsp:drawing xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram"
             xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <dsp:spTree>
    <dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/><dsp:nvPr/></dsp:nvGrpSpPr>
    <dsp:grpSpPr/>
    <dsp:sp modelId="node-1">
      <dsp:nvSpPr><dsp:cNvPr id="1" name="Step 1"/><dsp:cNvSpPr/><dsp:nvPr/></dsp:nvSpPr>
      <dsp:spPr>
        <a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm>
        <a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom>
        <a:solidFill><a:srgbClr val="1155CC"/></a:solidFill>
      </dsp:spPr>
      <dsp:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Plan</a:t></a:r></a:p></dsp:txBody>
    </dsp:sp>
    <dsp:sp modelId="arrow-1">
      <dsp:nvSpPr><dsp:cNvPr id="2" name="Arrow"/><dsp:cNvSpPr/><dsp:nvPr/></dsp:nvSpPr>
      <dsp:spPr>
        <a:xfrm><a:off x="1270000" y="127000"/><a:ext cx="635000" cy="381000"/></a:xfrm>
        <a:prstGeom prst="rightArrow"><a:avLst/></a:prstGeom>
        <a:solidFill><a:srgbClr val="EE6622"/></a:solidFill>
      </dsp:spPr>
    </dsp:sp>
  </dsp:spTree>
</dsp:drawing>"#;
    let slide_rels = &format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rLayout" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
  <Relationship Id="rDm" Type="{DATA_REL_TYPE}" Target="../diagrams/data1.xml"/>
  <Relationship Id="rDraw" Type="{DRAWING_REL_TYPE}" Target="../diagrams/drawing1.xml"/>
</Relationships>"#
    );

    let doc = build_package(data_xml, Some(drawing_xml), slide_rels, false)?;
    let page = &doc.pages[0];
    // 1 roundRect(PATH) + 1 text(TEXT) + 1 rightArrow(PATH) = 3 elements
    assert_eq!(page.elements.len(), 3, "Expected 2 shapes + 1 text element");
    // Step 1 starts at graphicFrame origin (50pt, 50pt)
    let box_el = &page.elements[0];
    assert!(matches!(box_el.kind, ElementType::PATH));
    assert_eq!(box_el.fill, 0xff11_55cc);
    assert!((box_el.transform[4] - 50.0).abs() < 0.01);
    assert!((box_el.transform[5] - 50.0).abs() < 0.01);

    let text_el = &page.elements[1];
    assert!(matches!(text_el.kind, ElementType::TEXT));
    assert_eq!(text_el.paragraphs[0].runs[0].text, "Plan");

    let arrow_el = &page.elements[2];
    assert!(matches!(arrow_el.kind, ElementType::PATH));
    assert_eq!(arrow_el.fill, 0xffee_6622);
    // Arrow x offset = 50 + 100 = 150pt
    assert!((arrow_el.transform[4] - 150.0).abs() < 0.01);

    assert!(
        !doc.warnings
            .iter()
            .any(|w| w.contains("SmartArt is omitted")),
        "Rendered SmartArt should not warn omitted: {:?}",
        doc.warnings
    );
    assert!(
        doc.warnings
            .iter()
            .any(|w| w.contains("SmartArt uses saved diagram drawings")),
        "Expected saved diagram drawing warning: {:?}",
        doc.warnings
    );
    Ok(())
}

#[test]
fn smartart_honors_tx_xfrm_embedded_pictures_and_group_scaling() -> TestResult {
    let data_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"/>"#;
    let drawing_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dsp:drawing xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram"
             xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
             xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <dsp:spTree>
    <dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/><dsp:nvPr/></dsp:nvGrpSpPr>
    <dsp:grpSpPr/>
    <dsp:sp modelId="card">
      <dsp:nvSpPr><dsp:cNvPr id="1" name="Card"/><dsp:cNvSpPr/><dsp:nvPr/></dsp:nvSpPr>
      <dsp:spPr>
        <a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm>
        <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
        <a:solidFill><a:srgbClr val="334455"/></a:solidFill>
      </dsp:spPr>
      <dsp:txXfrm><a:off x="635000" y="0"/><a:ext cx="635000" cy="635000"/></dsp:txXfrm>
      <dsp:txBody><a:bodyPr lIns="0" tIns="0" rIns="0" bIns="0"/><a:lstStyle/><a:p><a:r><a:t>Label</a:t></a:r></a:p></dsp:txBody>
    </dsp:sp>
    <dsp:pic modelId="icon">
      <dsp:nvPicPr><dsp:cNvPr id="2" name="Icon"/><dsp:cNvPicPr/><dsp:nvPr/></dsp:nvPicPr>
      <dsp:blipFill><a:blip r:embed="rImg"/></dsp:blipFill>
      <dsp:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="381000" cy="381000"/></a:xfrm></dsp:spPr>
    </dsp:pic>
  </dsp:spTree>
</dsp:drawing>"#;
    // 无 dataModelExt 时回退到 slide rels 中的 diagramDrawing；同时处于 2x 缩放的 grpSp 内
    let slide_rels = &format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rLayout" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
  <Relationship Id="rDm" Type="{DATA_REL_TYPE}" Target="../diagrams/data1.xml"/>
  <Relationship Id="rDrawFallback" Type="{DRAWING_REL_TYPE}" Target="../diagrams/drawing1.xml"/>
</Relationships>"#
    );

    let doc = build_package(data_xml, Some(drawing_xml), slide_rels, true)?;
    let page = &doc.pages[0];
    assert_eq!(page.elements.len(), 3);

    let rect = &page.elements[0];
    let text = &page.elements[1];
    let pic = &page.elements[2];
    assert!(matches!(rect.kind, ElementType::RECT));
    assert!(matches!(text.kind, ElementType::TEXT));
    assert!(matches!(pic.kind, ElementType::IMAGE));
    assert_eq!(pic.image.as_deref(), Some("ppt/media/pixel.png"));

    // grpSp (off=10pt, scale=2x) + graphicFrame (off=50pt) -> frame origin = 10 + 2*50 = 110pt
    // txXfrm off=(50pt, 0) -> text x = 110 + 2*50 = 210pt
    assert!(
        (rect.transform[4] - 110.0).abs() < 0.01,
        "rect x={}",
        rect.transform[4]
    );
    assert!(
        (text.transform[4] - 210.0).abs() < 0.01,
        "text x={}",
        text.transform[4]
    );
    Ok(())
}

#[test]
fn smartart_without_diagram_drawing_warns_omitted() -> TestResult {
    let data_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"/>"#;
    let slide_rels = &format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rLayout" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
  <Relationship Id="rDm" Type="{DATA_REL_TYPE}" Target="../diagrams/data1.xml"/>
</Relationships>"#
    );

    let doc = build_package(data_xml, None, slide_rels, false)?;
    assert!(doc.pages[0].elements.is_empty());
    assert!(doc
        .warnings
        .iter()
        .any(|w| w == "PPTX SmartArt is omitted."));
    Ok(())
}
