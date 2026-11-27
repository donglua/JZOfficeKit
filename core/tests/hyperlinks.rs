pub mod support;

use jz_office_core::parse_reader;
use std::io::Cursor;
use support::{
    docx,
    package::{archive, content_types, relationships, replace, xml, TestResult},
    pptx,
};

#[test]
fn docx_resolves_external_links_bookmarks_and_blocks_unsafe_schemes() -> TestResult {
    let mut parts = docx::parts();
    replace(
        &mut parts,
        "word/_rels/document.xml.rels",
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Id="rStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
          <Relationship Id="rExtOk" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/docs" TargetMode="External"/>
          <Relationship Id="rExtBad" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="javascript:alert(1)" TargetMode="External"/>
          <Relationship Id="rExtFile" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="file:///etc/passwd" TargetMode="External"/>
        </Relationships>"#,
    );
    replace(
        &mut parts,
        "word/document.xml",
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>
          <w:p>
            <w:hyperlink r:id="rExtOk"><w:r><w:t>官方网站</w:t></w:r></w:hyperlink>
            <w:hyperlink r:id="rExtBad"><w:r><w:t>恶意脚本</w:t></w:r></w:hyperlink>
            <w:hyperlink r:id="rExtFile"><w:r><w:t>本地路径</w:t></w:r></w:hyperlink>
            <w:hyperlink w:anchor="sec2"><w:r><w:t>跳到第二节</w:t></w:r></w:hyperlink>
          </w:p>
          <w:p><w:r><w:t>中间段落</w:t></w:r></w:p>
          <w:p><w:bookmarkStart w:id="0" w:name="sec2"/><w:r><w:t>第二节标题</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>
        </w:body></w:document>"#,
    );

    let doc = parse_reader(Cursor::new(archive(&parts)?))?;
    let runs = &doc.blocks[0].paragraphs[0].runs;
    assert_eq!(runs[0].link.as_deref(), Some("https://example.com/docs"));
    assert_eq!(runs[1].link, None);
    assert_eq!(runs[2].link, None);
    assert_eq!(runs[3].link.as_deref(), Some("#block:2"));
    Ok(())
}

#[test]
fn pptx_resolves_text_and_shape_external_and_slide_jump_links() -> TestResult {
    let mut parts = pptx::parts();
    replace(
        &mut parts,
        "ppt/slides/_rels/slide2.xml.rels",
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Id="rLayout" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
          <Relationship Id="rWeb" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://rust-lang.org" TargetMode="External"/>
          <Relationship Id="rSlideJump" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slide2.xml"/>
        </Relationships>"#,
    );
    replace(
        &mut parts,
        "ppt/slides/slide2.xml",
        r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree>
          <p:sp>
            <p:nvSpPr><p:cNvPr id="2" name="Card"><a:hlinkClick r:id="rSlideJump" action="ppaction://hlinksldjump"/></p:cNvPr><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
            <p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2540000" cy="635000"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr>
            <p:txBody><a:bodyPr/><a:p><a:r><a:rPr><a:hlinkClick r:id="rWeb"/></a:rPr><a:t>Rust</a:t></a:r></a:p></p:txBody>
          </p:sp>
        </p:spTree></p:cSld></p:sld>"#,
    );

    let doc = parse_reader(Cursor::new(archive(&parts)?))?;
    let el = &doc.pages[0].elements[0];
    assert_eq!(el.link.as_deref(), Some("#slide:1"));
    assert_eq!(
        el.paragraphs[0].runs[0].link.as_deref(),
        Some("https://rust-lang.org")
    );
    assert!(
        !doc.warnings
            .iter()
            .any(|w| w == "PPTX text hyperlinks are not interactive."),
        "resolved hyperlinks must not emit non-interactive warning: {:?}",
        doc.warnings
    );
    Ok(())
}

#[test]
fn xlsx_resolves_external_and_internal_cell_hyperlinks() -> TestResult {
    let parts = vec![
        xml(
            "[Content_Types].xml",
            &content_types(&[
                ("xl/workbook.xml", "spreadsheetml.sheet.main"),
                ("xl/worksheets/sheet1.xml", "spreadsheetml.worksheet"),
                ("xl/styles.xml", "spreadsheetml.styles"),
            ]),
        ),
        xml(
            "_rels/.rels",
            &relationships(&[("rDoc", "officeDocument", "xl/workbook.xml")]),
        ),
        xml(
            "xl/workbook.xml",
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rSheet1"/></sheets></workbook>"#,
        ),
        xml(
            "xl/_rels/workbook.xml.rels",
            &relationships(&[
                ("rSheet1", "worksheet", "worksheets/sheet1.xml"),
                ("rStyles", "styles", "styles.xml"),
            ]),
        ),
        xml(
            "xl/styles.xml",
            r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="1"><xf/></cellXfs></styleSheet>"#,
        ),
        xml(
            "xl/worksheets/_rels/sheet1.xml.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
              <Relationship Id="rLink1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="mailto:hi@example.com" TargetMode="External"/>
              <Relationship Id="rBad" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="data:text/html,boom" TargetMode="External"/>
            </Relationships>"#,
        ),
        xml(
            "xl/worksheets/sheet1.xml",
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
              <sheetData>
                <row r="1">
                  <c r="A1" t="inlineStr"><is><t>Email</t></is></c>
                  <c r="B1" t="inlineStr"><is><t>Jump</t></is></c>
                  <c r="C1" t="inlineStr"><is><t>Blocked</t></is></c>
                </row>
              </sheetData>
              <hyperlinks>
                <hyperlink ref="A1" r:id="rLink1"/>
                <hyperlink ref="B1" location="Sheet2!C5"/>
                <hyperlink ref="C1" r:id="rBad"/>
              </hyperlinks>
            </worksheet>"#,
        ),
    ];

    let doc = parse_reader(Cursor::new(archive(&parts)?))?;
    let cells = &doc.sheets[0].cells;
    assert_eq!(cells[0].link.as_deref(), Some("mailto:hi@example.com"));
    assert_eq!(cells[1].link.as_deref(), Some("#sheet:Sheet2!C5"));
    assert_eq!(cells[2].link, None);
    Ok(())
}
