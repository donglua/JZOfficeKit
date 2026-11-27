pub mod support;

use jz_office_core::parse_reader;
use std::io::Cursor;
use support::{
    checks, docx,
    package::{archive, replace, TestResult},
};

#[test]
fn docx_emits_page_height_and_explicit_page_breaks() -> TestResult {
    let mut parts = docx::parts();
    replace(
        &mut parts,
        "word/document.xml",
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
          <w:p><w:r><w:t>第一页内容</w:t></w:r></w:p>
          <w:p><w:pPr><w:pageBreakBefore/></w:pPr><w:r><w:t>第二页（段前分页）</w:t></w:r></w:p>
          <w:p><w:r><w:t>第二页尾部</w:t><w:br w:type="page"/><w:t>第三页（段中分页）</w:t></w:r></w:p>
          <w:p><w:r><w:t>第三页末</w:t><w:br w:type="page"/></w:r></w:p>
          <w:p><w:r><w:t>第四页（尾分页传递）</w:t></w:r></w:p>
          <w:sectPr><w:pgSz w:w="11906" w:h="16838"/></w:sectPr>
        </w:body></w:document>"#,
    );

    let doc = parse_reader(Cursor::new(archive(&parts)?))?;
    assert!((doc.width - 595.3).abs() < 0.1);
    assert!((doc.page_height - 841.9).abs() < 0.1);
    assert_eq!(doc.blocks.len(), 6);
    assert!(!doc.blocks[0].page_break_before);
    assert_eq!(checks::text(&doc.blocks[0].paragraphs), "第一页内容");

    assert!(doc.blocks[1].page_break_before);
    assert_eq!(
        checks::text(&doc.blocks[1].paragraphs),
        "第二页（段前分页）"
    );

    assert!(!doc.blocks[2].page_break_before);
    assert_eq!(checks::text(&doc.blocks[2].paragraphs), "第二页尾部");

    assert!(doc.blocks[3].page_break_before);
    assert_eq!(
        checks::text(&doc.blocks[3].paragraphs),
        "第三页（段中分页）"
    );

    assert!(!doc.blocks[4].page_break_before);
    assert_eq!(checks::text(&doc.blocks[4].paragraphs), "第三页末");

    assert!(doc.blocks[5].page_break_before);
    assert_eq!(
        checks::text(&doc.blocks[5].paragraphs),
        "第四页（尾分页传递）"
    );

    assert!(!doc
        .warnings
        .iter()
        .any(|w| w.contains("Page and column breaks") || w.contains("Paragraph page breaks")));
    Ok(())
}
