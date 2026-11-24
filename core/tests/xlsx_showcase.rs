pub mod support;

use jz_office_core::parse_reader;
use serde_json::Value;
use std::io::Cursor;
use support::{
    package::{archive, Parts, TestResult},
    xlsx, xlsx_showcase,
};

fn parse(parts: &Parts) -> TestResult<Value> {
    Ok(serde_json::to_value(parse_reader(Cursor::new(archive(
        parts,
    )?))?)?)
}

fn cell(json: &Value, sheet: usize, row: u32, column: u32) -> &Value {
    match json["sheets"][sheet]["cells"].as_array().and_then(|cells| {
        cells
            .iter()
            .find(|cell| cell["row"] == row && cell["column"] == column)
    }) {
        Some(cell) => cell,
        None => panic!("Missing cell {sheet}/{row}/{column}"),
    }
}

#[test]
fn labelled_comparisons_retain_original_values_formats_and_formula_caches() -> TestResult {
    let source = parse(&xlsx::parts())?;

    let demo = parse(&xlsx_showcase::parts())?;

    assert_eq!(demo["sheets"].as_array().map(Vec::len), Some(2));
    for index in 0..2 {
        assert_eq!(
            demo["sheets"][index]["name"],
            source["sheets"][index]["name"]
        );
    }
    for row in 1..9 {
        assert_eq!(cell(&demo, 0, row, 1), cell(&source, 0, row, 1));
    }
    for key in ["text", "numeric"] {
        assert_eq!(cell(&demo, 0, 9, 1)[key], cell(&source, 0, 7, 0)[key]);
        assert_eq!(cell(&demo, 0, 10, 1)[key], cell(&source, 0, 0, 0)[key]);
    }
    for column in 0..2 {
        assert_eq!(cell(&demo, 1, 1, column), cell(&source, 1, 1, column));
    }
    for (path, source_bytes) in xlsx::parts() {
        if !path.starts_with("xl/worksheets/") {
            continue;
        }
        let demo_parts = xlsx_showcase::parts();
        let demo_bytes = &demo_parts
            .iter()
            .find(|(name, _)| *name == path)
            .ok_or("Missing worksheet")?
            .1;
        let source_xml = roxmltree::Document::parse(std::str::from_utf8(&source_bytes)?)?;
        let demo_xml = roxmltree::Document::parse(std::str::from_utf8(demo_bytes)?)?;
        let formulas = |xml: &roxmltree::Document<'_>| {
            xml.descendants()
                .filter(|node| node.has_tag_name("f"))
                .map(|node| {
                    (
                        node.parent()
                            .and_then(|parent| parent.attribute("r"))
                            .map(str::to_owned),
                        node.text().map(str::to_owned),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(formulas(&demo_xml), formulas(&source_xml));
    }
    Ok(())
}

#[test]
fn merged_style_example_retains_original_style_and_exposes_size_differences() -> TestResult {
    let source = parse(&xlsx::parts())?;

    let demo = parse(&xlsx_showcase::parts())?;

    for index in 0..7 {
        assert_eq!(demo["cellStyles"][index], source["cellStyles"][index]);
    }
    assert_eq!(cell(&demo, 0, 12, 1)["style"], 1);
    assert!(cell(&demo, 0, 12, 1)["text"]
        .as_str()
        .is_some_and(|value| value.contains('\n')));
    assert!(demo["sheets"][0]["merges"]
        .as_array()
        .is_some_and(|merges| merges.contains(
            &serde_json::json!({"startRow":12,"startColumn":1,"endRow":12,"endColumn":2})
        )));
    assert_eq!(demo["sheets"][0]["rowHeights"][12], 72.0);
    for (column, width) in [(0, 94.5), (1, 110.25), (2, 147.0)] {
        assert_eq!(demo["sheets"][0]["columnWidths"][column], width);
    }
    assert_eq!(demo["cellStyles"][7]["wrap"], true);
    assert_eq!(demo["cellStyles"][8]["wrap"], true);
    Ok(())
}

#[test]
fn overview_sheet_renders_floating_picture_shape_and_connector() -> TestResult {
    let demo = parse(&xlsx_showcase::parts())?;

    let elements = demo["sheets"][0]["elements"]
        .as_array()
        .ok_or("Overview 工作表应输出浮动元素")?;
    // 样例包含：1 张浮动图片（含外框描边）、1 个标注形状 + 其文本框、1 条虚线箭头连接线
    assert!(
        elements.iter().any(|element| element["type"] == "IMAGE"),
        "缺少浮动图片元素; kinds={:?}",
        elements
            .iter()
            .map(|element| element["type"].clone())
            .collect::<Vec<_>>()
    );
    let image = elements
        .iter()
        .find(|element| element["type"] == "IMAGE")
        .ok_or("缺少浮动图片元素")?;
    assert_eq!(image["image"], "xl/media/logo.png");
    assert!(image["width"].as_f64().unwrap_or_default() > 0.0);
    assert!(image["imageBounds"]
        .as_array()
        .is_some_and(|b| b.len() == 4));

    // 标注形状与连接线都编译为 PATH；虚线只出现在带 prstDash 的连接线上
    assert!(
        elements.iter().any(|element| {
            element["type"] == "PATH"
                && element["strokeDash"]
                    .as_array()
                    .is_some_and(|dash| !dash.is_empty())
        }),
        "缺少虚线连接线元素; kinds={:?}",
        elements
            .iter()
            .map(|element| element["type"].clone())
            .collect::<Vec<_>>()
    );

    assert!(
        elements.iter().any(|element| {
            element["type"] == "TEXT"
                && element["paragraphs"].as_array().is_some_and(|paragraphs| {
                    paragraphs.iter().any(|p| {
                        p["runs"].as_array().is_some_and(|runs| {
                            runs.iter().any(|run| run["text"] == "浮动图片与形状")
                        })
                    })
                })
        }),
        "缺少标注形状的文本框元素"
    );
    Ok(())
}

#[test]
fn sparse_scrolling_ruler_keeps_both_axes_and_regular_checkpoints() -> TestResult {
    let parts = xlsx_showcase::parts();

    let demo = parse(&parts)?;

    let sheet = &demo["sheets"][0];
    assert_eq!(sheet["rowHeights"].as_array().map(Vec::len), Some(120));
    assert_eq!(sheet["columnWidths"].as_array().map(Vec::len), Some(12));
    assert_eq!(cell(&demo, 0, 119, 11)["text"], "L120");
    let cells = sheet["cells"].as_array().ok_or("Missing sheet cells")?;
    for (row, count) in [(16, 12), (17, 1), (19, 12), (119, 12)] {
        assert_eq!(
            cells.iter().filter(|cell| cell["row"] == row).count(),
            count
        );
    }
    for row in 16..120 {
        assert_eq!(cell(&demo, 0, row, 0)["text"], format!("A{}", row + 1));
    }
    Ok(())
}
