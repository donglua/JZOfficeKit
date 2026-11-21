pub mod support;

use jz_office_core::{
    model::{Document, ElementType, VertAlign},
    parse_reader,
};
use std::io::Cursor;
use support::{
    checks, docx,
    package::{archive, content_types, relationships, replace, xml, Parts, TestResult},
};

const BODY: &str = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>
<w:p><w:r><w:t>Alpha</w:t></w:r><w:r><w:footnoteReference w:id="5"/></w:r></w:p>
<w:p><w:r><w:t>Beta</w:t></w:r><w:r><w:footnoteReference w:id="9"/></w:r></w:p>
<w:p><w:r><w:t>End</w:t></w:r><w:r><w:endnoteReference w:id="20"/></w:r></w:p>
</w:body></w:document>"#;

const FOOTNOTES: &str = r#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:t>Separator</w:t></w:r></w:p></w:footnote>
<w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:t>Continuation</w:t></w:r></w:p></w:footnote>
<w:footnote w:id="9"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:tab/><w:t>Second footnote</w:t></w:r></w:p></w:footnote>
<w:footnote w:id="5"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:tab/><w:t>First footnote</w:t></w:r></w:p><w:p><w:r><w:t>More detail</w:t></w:r></w:p></w:footnote>
<w:footnote w:id="99"><w:p><w:r><w:t>Unused footnote</w:t></w:r></w:p></w:footnote>
</w:footnotes>"#;

const ENDNOTES: &str = r#"<w:endnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:endnote w:id="20"><w:p><w:r><w:endnoteRef/></w:r><w:r><w:tab/><w:t>First endnote</w:t></w:r></w:p></w:endnote>
<w:endnote w:id="30"><w:p><w:r><w:t>Unused endnote</w:t></w:r></w:p></w:endnote>
</w:endnotes>"#;

fn parts(body: &str, footnotes: &str, endnotes: &str) -> Parts {
    let mut parts = docx::parts();
    replace(&mut parts, "word/document.xml", body);
    replace(
        &mut parts,
        "[Content_Types].xml",
        &content_types(&[
            ("word/document.xml", "wordprocessingml.document.main"),
            ("word/styles.xml", "wordprocessingml.styles"),
            ("word/footnotes.xml", "wordprocessingml.footnotes"),
            ("word/endnotes.xml", "wordprocessingml.endnotes"),
        ]),
    );
    replace(
        &mut parts,
        "word/_rels/document.xml.rels",
        &relationships(&[
            ("rStyles", "styles", "styles.xml"),
            ("rFootnotes", "footnotes", "footnotes.xml"),
            ("rEndnotes", "endnotes", "endnotes.xml"),
        ]),
    );
    parts.push(xml("word/footnotes.xml", footnotes));
    parts.push(xml("word/endnotes.xml", endnotes));
    parts
}

fn parse(parts: Parts) -> TestResult<Document> {
    Ok(parse_reader(Cursor::new(archive(&parts)?))?)
}

#[test]
fn referenced_notes_keep_order_numbers_styles_and_searchable_text() -> TestResult {
    let document = parse(parts(BODY, FOOTNOTES, ENDNOTES))?;
    let texts: Vec<String> = document
        .blocks
        .iter()
        .map(|block| checks::text(&block.paragraphs))
        .collect();

    assert_eq!(
        texts,
        [
            "Alpha1",
            "Beta2",
            "End1",
            "1\tFirst footnote",
            "More detail",
            "2\tSecond footnote",
            "1\tFirst endnote",
        ]
    );
    let reference = document.blocks[0].paragraphs[0]
        .runs
        .last()
        .ok_or("Missing footnote reference")?;
    assert_eq!(reference.text, "1");
    assert_eq!(reference.vert_align, VertAlign::Superscript);
    let endnote_reference = document.blocks[2].paragraphs[0]
        .runs
        .last()
        .ok_or("Missing endnote reference")?;
    assert_eq!(endnote_reference.vert_align, VertAlign::Superscript);

    let first_footnote = document.blocks[3]
        .paragraphs
        .first()
        .ok_or("Missing note")?;
    assert!(first_footnote.before >= 12.0);
    assert_ne!(first_footnote.border_top, 0);
    let first_endnote = document.blocks[6]
        .paragraphs
        .first()
        .ok_or("Missing note")?;
    assert!(first_endnote.before >= 12.0);
    assert_ne!(first_endnote.border_top, 0);

    assert_eq!(document.search("First footnote").len(), 1);
    assert_eq!(document.search("First endnote").len(), 1);
    for omitted in [
        "Separator",
        "Continuation",
        "Unused footnote",
        "Unused endnote",
    ] {
        assert!(document.search(omitted).is_empty());
    }
    assert!(!document
        .warnings
        .iter()
        .any(|warning| warning.contains("Footnotes and endnotes are omitted")));
    Ok(())
}

#[test]
fn missing_note_targets_are_reported_without_losing_body_text() -> TestResult {
    let mut parts = parts(BODY, FOOTNOTES, ENDNOTES);
    replace(
        &mut parts,
        "word/document.xml",
        &BODY.replace("w:id=\"5\"", "w:id=\"404\""),
    );
    let document = parse(parts)?;

    assert_eq!(checks::text(&document.blocks[0].paragraphs), "Alpha");
    assert!(document
        .warnings
        .iter()
        .any(|warning| warning.contains("missing note targets")));
    assert_eq!(document.search("Second footnote").len(), 1);
    assert!(document.search("First footnote").is_empty());
    Ok(())
}

#[test]
fn note_images_resolve_against_the_note_part_relationships() -> TestResult {
    let body = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body><w:p><w:r><w:footnoteReference w:id="5"/></w:r></w:p></w:body></w:document>"#;
    let footnotes = r#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:footnote w:id="5"><w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="914400" cy="457200"/><wp:docPr id="1" name="Note image"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="pixel.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rNoteImage"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p></w:footnote></w:footnotes>"#;
    let mut parts = parts(body, footnotes, ENDNOTES);
    parts.push(xml(
        "word/_rels/footnotes.xml.rels",
        &relationships(&[("rNoteImage", "image", "media/pixel.png")]),
    ));

    let document = parse(parts)?;
    let image = document
        .blocks
        .iter()
        .find(|block| matches!(block.kind, ElementType::IMAGE))
        .ok_or("Missing note image")?;
    assert_eq!(image.image.as_deref(), Some("word/media/pixel.png"));
    Ok(())
}
