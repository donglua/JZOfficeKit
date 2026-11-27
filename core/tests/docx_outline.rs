pub mod support;

use jz_office_core::{model::Document, parse_reader};
use std::io::Cursor;
use support::{
    docx,
    package::{archive, replace, TestResult},
};

fn parse_docx(body: &str, styles: Option<&str>) -> TestResult<Document> {
    let mut parts = docx::parts();
    replace(&mut parts, "word/document.xml", body);
    if let Some(styles_xml) = styles {
        replace(&mut parts, "word/styles.xml", styles_xml);
    }
    Ok(parse_reader(Cursor::new(archive(&parts)?))?)
}

#[test]
fn extracts_outline_from_styles_and_direct_outline_level() -> TestResult {
    let styles = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
      <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>
      <w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style>
      <w:style w:type="paragraph" w:styleId="ZhH3"><w:name w:val="标题 3"/></w:style>
    </w:styles>"#;
    let body = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
      <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>第一章   总览</w:t></w:r></w:p>
      <w:p><w:r><w:t>普通正文段落</w:t></w:r></w:p>
      <w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>1.1 背景</w:t></w:r></w:p>
      <w:p><w:pPr><w:pStyle w:val="Heading1"/><w:outlineLvl w:val="9"/></w:pPr><w:r><w:t>被降级为正文</w:t></w:r></w:p>
      <w:p><w:pPr><w:pStyle w:val="ZhH3"/></w:pPr><w:r><w:t>中文标题样式</w:t></w:r></w:p>
      <w:p><w:pPr><w:outlineLvl w:val="3"/></w:pPr><w:r><w:t>直接大纲级别</w:t></w:r></w:p>
      <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr></w:p>
      <w:tbl><w:tr><w:tc><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>表格内标题不收入</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
    </w:body></w:document>"#;

    let doc = parse_docx(body, Some(styles))?;
    assert_eq!(doc.outline.len(), 4);
    assert_eq!(doc.outline[0].level, 1);
    assert_eq!(doc.outline[0].title, "第一章 总览");
    assert_eq!(doc.outline[0].block_index, 0);

    assert_eq!(doc.outline[1].level, 2);
    assert_eq!(doc.outline[1].title, "1.1 背景");
    assert_eq!(doc.outline[1].block_index, 2);

    assert_eq!(doc.outline[2].level, 3);
    assert_eq!(doc.outline[2].title, "中文标题样式");
    assert_eq!(doc.outline[2].block_index, 4);

    assert_eq!(doc.outline[3].level, 4);
    assert_eq!(doc.outline[3].title, "直接大纲级别");
    assert_eq!(doc.outline[3].block_index, 5);
    Ok(())
}

#[test]
fn caps_outline_items_and_truncates_long_titles() -> TestResult {
    let long_title = "甲".repeat(300);
    let mut paragraphs = format!(
        r#"<w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>{long_title}</w:t></w:r></w:p>"#
    );
    for i in 1..=520 {
        paragraphs.push_str(&format!(
            r#"<w:p><w:pPr><w:outlineLvl w:val="1"/></w:pPr><w:r><w:t>Heading {i}</w:t></w:r></w:p>"#
        ));
    }
    let body = format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{paragraphs}</w:body></w:document>"#
    );
    let doc = parse_docx(&body, None)?;
    assert_eq!(doc.outline.len(), 500);
    assert_eq!(doc.outline[0].title.encode_utf16().count(), 256);
    Ok(())
}
