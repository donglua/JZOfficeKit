pub mod support;

use jz_office_core::{
    model::{Document, ElementType},
    parse_reader,
};
use std::io::Cursor;
use support::{
    package::{archive, replace, TestResult},
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
