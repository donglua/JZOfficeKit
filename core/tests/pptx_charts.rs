pub mod support;

use jz_office_core::{
    model::{Document, ElementType, Page, PathCommand},
    parse_reader,
};
use std::io::Cursor;
use support::{
    checks,
    package::{archive, replace, TestResult},
    pptx_charts::{
        self as fixture, assert_series, data_segments, BLUE, FIRST_CHART, FRAME, RED, VALUES,
    },
};

#[test]
fn cached_series_render_inside_the_frame_in_presentation_order() -> TestResult {
    let document = parse_reader(Cursor::new(archive(&fixture::parts())?))?;

    assert_eq!(document.pages.len(), 2);
    assert!(texts(&document.pages[0]).contains(&"Category cache".to_owned()));
    assert!(texts(&document.pages[1]).contains(&"Date cache".to_owned()));
    for page in &document.pages {
        checks::near(page.width, 720.0);
        checks::near(page.height, 405.0);
    }
    assert_series(&document.pages[0], RED, [10.0, 25.0, 15.0, 30.0]);
    assert_series(&document.pages[0], BLUE, [30.0, 20.0, 25.0, 10.0]);
    Ok(())
}

#[test]
fn chart_title_legend_and_categories_come_from_indexed_caches() -> TestResult {
    let document = parse_reader(Cursor::new(archive(&fixture::parts())?))?;

    let page = &document.pages[0];
    let labels = texts(page);
    for text in ["Cached categories", "Alpha", "Beta", "Q1", "Q2", "Q3", "Q4"] {
        assert!(
            labels.iter().any(|label| label == text),
            "Missing {text}: {labels:?}"
        );
    }
    let mut categories: Vec<_> = page
        .elements
        .iter()
        .filter_map(|element| {
            let text = checks::text(&element.paragraphs);
            ["Q1", "Q2", "Q3", "Q4"]
                .contains(&text.as_str())
                .then_some((element.x, text))
        })
        .collect();
    categories.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(
        categories
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        ["Q1", "Q2", "Q3", "Q4"]
    );
    Ok(())
}

#[test]
fn date_axis_renders_cached_excel_dates_without_the_external_csv() -> TestResult {
    let parts = fixture::parts();
    assert!(parts.iter().all(|(name, _)| !name.ends_with(".csv")));

    let document = parse_reader(Cursor::new(archive(&parts)?))?;

    let page = &document.pages[1];
    assert_series(page, RED, [10.0, 25.0, 15.0, 30.0]);
    let labels = texts(page);
    for date in ["2024-01-01", "2024-01-02", "2024-01-03", "2024-01-04"] {
        assert!(
            labels.iter().any(|label| label == date),
            "Missing formatted cached date {date}: {labels:?}"
        );
    }
    assert!(labels.iter().any(|label| label == "Cached dates"));
    assert!(labels.iter().any(|label| label == "Daily"));
    Ok(())
}

#[test]
fn sparse_values_leave_a_gap_at_the_missing_index_by_default() -> TestResult {
    let chart = fixture::category_chart()
        .replace(r#"<c:pt idx="1"><c:v>25</c:v></c:pt>"#, "")
        .replace(r#"<c:dispBlanksAs val="gap"/>"#, "");

    let document = document(&chart)?;

    let segments = data_segments(&document.pages[0], RED);
    assert_eq!(segments.len(), 1, "Must not connect across missing point 1");
    let [(x1, y1), (x2, y2)] = segments[0];
    assert!(x1 > FRAME[0] + FRAME[2] / 2.0 && x2 > x1);
    assert!(y2 < y1, "Remaining segment must connect values 15 and 30");
    assert_series(&document.pages[0], BLUE, [30.0, 20.0, 25.0, 10.0]);
    Ok(())
}

#[test]
fn sparse_values_honor_zero_and_span_policies() -> TestResult {
    for mode in ["zero", "span"] {
        let chart = fixture::category_chart()
            .replace(r#"<c:pt idx="1"><c:v>25</c:v></c:pt>"#, "")
            .replace(
                r#"dispBlanksAs val="gap""#,
                &format!(r#"dispBlanksAs val="{mode}""#),
            );

        let document = document(&chart)?;

        let page = &document.pages[0];
        if mode == "zero" {
            assert_series(page, RED, [10.0, 0.0, 15.0, 30.0]);
        } else {
            let segments = data_segments(page, RED);
            assert_eq!(segments.len(), 2);
            let [a, b] = segments[0];
            let [c, d] = segments[1];
            checks::near(b.0, c.0);
            checks::near(b.1, c.1);
            checks::near((b.0 - a.0) / (d.0 - c.0), 2.0);
            checks::near((b.1 - a.1) / (d.1 - a.1), 0.25);
            assert!(a.1 > b.1 && b.1 > d.1);
        }
    }
    Ok(())
}

#[test]
fn missing_value_cache_warns_without_fabricating_data_lines() -> TestResult {
    let chart = fixture::category_chart().replace(
        VALUES,
        r#"<c:val><c:numRef><c:f>Missing!B2:B5</c:f></c:numRef></c:val>"#,
    );

    assert_omitted(&chart)
}

#[test]
fn malformed_value_caches_warn_without_fabricating_data_lines() -> TestResult {
    for values in [
        VALUES.replace("<c:v>15</c:v>", "<c:v>NaN</c:v>"),
        VALUES.replace(r#"idx="2""#, r#"idx="0""#),
        VALUES.replace(r#"idx="2""#, r#"idx="4""#),
        VALUES.replace(r#"idx="2""#, r#"idx="invalid""#),
    ] {
        let chart = fixture::category_chart().replace(VALUES, &values);
        assert_omitted(&chart)?;
    }
    Ok(())
}

#[test]
fn unsupported_bar_and_unknown_charts_warn_without_data_lines() -> TestResult {
    for chart in [
        fixture::category_chart().replace("lineChart", "barChart"),
        fixture::category_chart()
            .replace(
                "<c:lineChart><c:grouping val=\"standard\"/>",
                "<c:barChart><c:barDir val=\"col\"/><c:grouping val=\"unknown\"/>",
            )
            .replace("</c:lineChart>", "</c:barChart>"),
        fixture::category_chart().replace("lineChart", "radarChart"),
        fixture::category_chart()
            .replace(r#"grouping val="standard""#, r#"grouping val="unknown""#),
    ] {
        assert_omitted(&chart)?;
    }
    Ok(())
}

#[test]
fn explicit_value_axis_bounds_clip_crossing_segments() -> TestResult {
    let chart = fixture::category_chart().replace(
        r#"<c:valAx><c:axId val="200"/><c:scaling>"#,
        r#"<c:valAx><c:axId val="200"/><c:scaling><c:min val="15"/><c:max val="25"/>"#,
    ).replace("<c:majorGridlines/>", r#"<c:majorGridlines><c:spPr><a:ln><a:solidFill><a:srgbClr val="11AB22"/></a:solidFill></a:ln></c:spPr></c:majorGridlines>"#);

    let document = document(&chart)?;

    let page = &document.pages[0];
    let mut grid_y: Vec<_> = page
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::LINE) && e.stroke == 0xff11ab22)
        .map(|e| fixture::endpoints(e)[0].1)
        .collect();
    grid_y.sort_by(f32::total_cmp);
    let top = *grid_y.first().ok_or("Missing range gridlines")?;
    let bottom = *grid_y.last().ok_or("Missing range gridlines")?;
    assert!(bottom > top);
    let segments = data_segments(page, RED);
    assert_eq!(segments.len(), 3);
    let full_width = segments[1][1].0 - segments[1][0].0;
    for index in [0, 2] {
        checks::near(
            (segments[index][1].0 - segments[index][0].0) / full_width,
            2.0 / 3.0,
        );
    }
    for ((_, y), value) in segments
        .into_iter()
        .flatten()
        .zip([15.0, 25.0, 25.0, 15.0, 15.0, 25.0])
    {
        checks::near((bottom - y) / (bottom - top), (value - 15.0) / 10.0);
    }
    Ok(())
}

#[test]
fn logarithmic_and_invalid_explicit_scales_warn_without_data_lines() -> TestResult {
    for scale in [
        r#"<c:logBase val="10"/>"#,
        r#"<c:min val="40"/><c:max val="5"/>"#,
    ] {
        let chart = fixture::category_chart().replace(
            r#"<c:valAx><c:axId val="200"/><c:scaling>"#,
            &format!(r#"<c:valAx><c:axId val="200"/><c:scaling>{scale}"#),
        );
        assert_omitted(&chart)?;
    }
    Ok(())
}

#[test]
fn caches_over_point_or_series_budgets_warn_without_partial_charts() -> TestResult {
    for counts in [&[2049][..], &[2048, 2048, 2], &[2; 9]] {
        assert_omitted(&fixture::oversized_chart(counts))?;
    }
    Ok(())
}

#[test]
fn column_chart_renders_rectangles_for_each_series_bar() -> TestResult {
    let chart_xml = fixture::category_chart()
        .replace(
            "<c:lineChart><c:grouping val=\"standard\"/>",
            "<c:barChart><c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>",
        )
        .replace("</c:lineChart>", "</c:barChart>");

    let document = document(&chart_xml)?;
    let page = &document.pages[0];

    // Verify warning mentions column charts
    assert!(
        document
            .warnings
            .iter()
            .any(|w| w.contains("column charts")),
        "Expected column chart warning, got: {:?}",
        document.warnings
    );

    // Filter bars: filled RECTs with RED or BLUE fill
    let red_bars: Vec<_> = page
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::RECT) && e.fill == RED)
        .collect();
    let blue_bars: Vec<_> = page
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::RECT) && e.fill == BLUE)
        .collect();

    // 4 categories each with 1 red bar and 1 blue bar (plus legend swatches)
    assert_eq!(
        red_bars.len(),
        5,
        "Expected 4 data bars + 1 legend swatch for RED"
    );
    assert_eq!(
        blue_bars.len(),
        5,
        "Expected 4 data bars + 1 legend swatch for BLUE"
    );

    // All categories present
    let labels = texts(page);
    for cat in ["Q1", "Q2", "Q3", "Q4", "Alpha", "Beta"] {
        assert!(labels.contains(&cat.to_owned()), "Missing label {cat}");
    }

    Ok(())
}

#[test]
fn column_chart_renders_negative_values_from_zero_baseline() -> TestResult {
    let negative_values = r#"<c:val><c:numLit><c:formatCode>General</c:formatCode><c:ptCount val="2"/>
<c:pt idx="0"><c:v>-15</c:v></c:pt>
<c:pt idx="1"><c:v>25</c:v></c:pt></c:numLit></c:val>"#;
    let single_series = format!(
        r#"<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/>{}<c:axId val="100"/><c:axId val="200"/></c:barChart>"#,
        fixture::series(
            (0, "Delta", "C03050"),
            r#"<c:cat><c:strLit><c:ptCount val="2"/><c:pt idx="0"><c:v>Down</c:v></c:pt><c:pt idx="1"><c:v>Up</c:v></c:pt></c:strLit></c:cat>"#,
            negative_values
        )
    );
    let chart_xml = fixture::category_chart()
        .replace(
            "<c:lineChart><c:grouping val=\"standard\"/>",
            &single_series,
        )
        .replace("</c:lineChart>", "");

    let document = document(&chart_xml)?;
    let page = &document.pages[0];

    let bars: Vec<_> = page
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::RECT) && e.fill == RED)
        .collect();
    // 2 data bars + 1 legend swatch = 3
    assert_eq!(bars.len(), 3, "Expected 2 data bars + 1 legend swatch");

    let down_bar = bars
        .iter()
        .find(|b| b.width > 12.0 && b.x < 300.0)
        .ok_or("Missing down bar")?;
    let up_bar = bars
        .iter()
        .find(|b| b.width > 12.0 && b.x > 300.0)
        .ok_or("Missing up bar")?;

    // Negative bar starts at zero baseline and goes down (so its top is baseline)
    // Positive bar starts below and reaches zero baseline at its bottom (so its bottom is baseline)
    checks::near(down_bar.y, up_bar.y + up_bar.height);
    assert!(down_bar.height > 0.0);
    assert!(up_bar.height > 0.0);

    Ok(())
}

#[test]
fn area_chart_renders_closed_fills_and_series_outlines() -> TestResult {
    let document = document(&area_chart())?;
    let page = &document.pages[0];

    for color in [RED, BLUE] {
        let areas = area_paths(page, color);
        assert_eq!(areas.len(), 1, "Expected one filled path per series");
        let path = areas[0].paths.first().ok_or("Missing area path")?;
        assert!(path.fill);
        assert!(!path.stroke);
        assert_eq!(path_command_count(areas[0], PathCommandKind::Close), 3);
        assert_eq!(path_command_count(areas[0], PathCommandKind::Move), 3);
    }
    assert_series(page, RED, [10.0, 25.0, 15.0, 30.0]);
    assert_series(page, BLUE, [30.0, 20.0, 25.0, 10.0]);
    assert!(document
        .warnings
        .iter()
        .any(|warning| warning.contains("area charts")));
    Ok(())
}

#[test]
fn area_chart_splits_positive_and_negative_regions_at_zero() -> TestResult {
    let values = r#"<c:val><c:numLit><c:formatCode>General</c:formatCode><c:ptCount val="2"/>
<c:pt idx="0"><c:v>-15</c:v></c:pt><c:pt idx="1"><c:v>25</c:v></c:pt></c:numLit></c:val>"#;
    let series = format!(
        r#"<c:areaChart><c:grouping val="standard"/>{}<c:axId val="100"/><c:axId val="200"/></c:areaChart>"#,
        fixture::series(
            (0, "Delta", "C03050"),
            r#"<c:cat><c:strLit><c:ptCount val="2"/><c:pt idx="0"><c:v>Down</c:v></c:pt><c:pt idx="1"><c:v>Up</c:v></c:pt></c:strLit></c:cat>"#,
            values,
        )
    );
    let chart = fixture::category_chart()
        .replace("<c:lineChart><c:grouping val=\"standard\"/>", &series)
        .replace("</c:lineChart>", "");

    let document = document(&chart)?;
    let areas = area_paths(&document.pages[0], RED);
    assert_eq!(areas.len(), 1);
    assert_eq!(path_command_count(areas[0], PathCommandKind::Close), 2);
    assert_eq!(path_command_count(areas[0], PathCommandKind::Move), 2);
    Ok(())
}

#[test]
fn area_chart_honors_blank_value_policies() -> TestResult {
    for (mode, expected_regions) in [("gap", 1), ("zero", 3), ("span", 2)] {
        let chart = area_chart()
            .replace(r#"<c:pt idx="1"><c:v>25</c:v></c:pt>"#, "")
            .replace(
                r#"dispBlanksAs val="gap""#,
                &format!(r#"dispBlanksAs val="{mode}""#),
            );
        let document = document(&chart)?;
        let areas = area_paths(&document.pages[0], RED);
        assert_eq!(areas.len(), 1, "Missing area series for {mode}");
        assert_eq!(
            path_command_count(areas[0], PathCommandKind::Close),
            expected_regions,
            "Unexpected region count for {mode}"
        );
    }
    Ok(())
}

#[test]
fn stacked_horizontal_bar_chart_stacks_series_end_to_end() -> TestResult {
    let chart_xml = fixture::category_chart()
        .replace(
            "<c:lineChart><c:grouping val=\"standard\"/>",
            "<c:barChart><c:barDir val=\"bar\"/><c:grouping val=\"stacked\"/>",
        )
        .replace("</c:lineChart>", "</c:barChart>");

    let document = document(&chart_xml)?;
    let page = &document.pages[0];

    let mut red_bars: Vec<_> = page
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::RECT) && e.fill == RED && e.height > 8.0)
        .collect();
    let mut blue_bars: Vec<_> = page
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::RECT) && e.fill == BLUE && e.height > 8.0)
        .collect();
    red_bars.sort_by(|a, b| a.y.total_cmp(&b.y));
    blue_bars.sort_by(|a, b| a.y.total_cmp(&b.y));

    assert_eq!(red_bars.len(), 4, "Expected 4 stacked RED segments");
    assert_eq!(blue_bars.len(), 4, "Expected 4 stacked BLUE segments");

    // 同一分类的两段条形应共行（相同 y 与 height）且首尾衔接（blue.x == red.x + red.width）
    for (red, blue) in red_bars.iter().zip(&blue_bars) {
        checks::near(red.y, blue.y);
        checks::near(red.height, blue.height);
        checks::near(red.x + red.width, blue.x);
        assert!(red.width > 0.0 && blue.width > 0.0);
        assert!(blue.x + blue.width <= FRAME[0] + FRAME[2] + 0.5);
    }
    // 堆积总和 [40, 45, 40, 40]：第 2 组（25+20=45）应比第 1 组（10+30=40）更宽
    let total_0 = red_bars[0].width + blue_bars[0].width;
    let total_1 = red_bars[1].width + blue_bars[1].width;
    checks::near(total_1 / total_0, 45.0 / 40.0);
    // 第 1 组中 BLUE(30) 宽度应为 RED(10) 的 3 倍
    checks::near(blue_bars[0].width / red_bars[0].width, 3.0);
    assert!(document
        .warnings
        .iter()
        .any(|w| w.contains("horizontal bar charts")));
    Ok(())
}

#[test]
fn stacked_column_chart_stacks_series_vertically() -> TestResult {
    let chart_xml = fixture::category_chart()
        .replace(
            "<c:lineChart><c:grouping val=\"standard\"/>",
            "<c:barChart><c:barDir val=\"col\"/><c:grouping val=\"stacked\"/>",
        )
        .replace("</c:lineChart>", "</c:barChart>");

    let document = document(&chart_xml)?;
    let page = &document.pages[0];

    let mut red_bars: Vec<_> = page
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::RECT) && e.fill == RED && e.width > 12.0)
        .collect();
    let mut blue_bars: Vec<_> = page
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::RECT) && e.fill == BLUE && e.width > 12.0)
        .collect();
    red_bars.sort_by(|a, b| a.x.total_cmp(&b.x));
    blue_bars.sort_by(|a, b| a.x.total_cmp(&b.x));

    assert_eq!(red_bars.len(), 4, "Expected 4 stacked RED column segments");
    assert_eq!(
        blue_bars.len(),
        4,
        "Expected 4 stacked BLUE column segments"
    );

    // 同一分类的两段柱形应共列（相同 x 与 width）且 BLUE 堆叠在 RED 上方
    for (red, blue) in red_bars.iter().zip(&blue_bars) {
        checks::near(red.x, blue.x);
        checks::near(red.width, blue.width);
        checks::near(blue.y + blue.height, red.y);
        assert!(blue.y >= FRAME[1] - 0.5);
    }
    let total_0 = red_bars[0].height + blue_bars[0].height;
    let total_1 = red_bars[1].height + blue_bars[1].height;
    checks::near(total_1 / total_0, 45.0 / 40.0);
    assert!(document
        .warnings
        .iter()
        .any(|w| w.contains("column charts")));
    Ok(())
}

#[test]
fn percent_stacked_bar_and_column_charts_normalize_categories_to_full_span() -> TestResult {
    for (dir, horizontal) in [("bar", true), ("col", false)] {
        let chart_xml = fixture::category_chart()
            .replace(
                "<c:lineChart><c:grouping val=\"standard\"/>",
                &format!(
                    "<c:barChart><c:barDir val=\"{dir}\"/><c:grouping val=\"percentStacked\"/>"
                ),
            )
            .replace("</c:lineChart>", "</c:barChart>");

        let document = document(&chart_xml)?;
        let page = &document.pages[0];

        let mut red_bars: Vec<_> = page
            .elements
            .iter()
            .filter(|e| {
                matches!(e.kind, ElementType::RECT)
                    && e.fill == RED
                    && if horizontal {
                        e.height > 8.0
                    } else {
                        e.width > 12.0
                    }
            })
            .collect();
        let mut blue_bars: Vec<_> = page
            .elements
            .iter()
            .filter(|e| {
                matches!(e.kind, ElementType::RECT)
                    && e.fill == BLUE
                    && if horizontal {
                        e.height > 8.0
                    } else {
                        e.width > 12.0
                    }
            })
            .collect();
        if horizontal {
            red_bars.sort_by(|a, b| a.y.total_cmp(&b.y));
            blue_bars.sort_by(|a, b| a.y.total_cmp(&b.y));
        } else {
            red_bars.sort_by(|a, b| a.x.total_cmp(&b.x));
            blue_bars.sort_by(|a, b| a.x.total_cmp(&b.x));
        }
        assert_eq!(red_bars.len(), 4, "{dir} percentStacked RED count");
        assert_eq!(blue_bars.len(), 4, "{dir} percentStacked BLUE count");

        // 4 个分类全为正数，归一化后总跨度应全部相等（100%）
        let spans: Vec<f32> = red_bars
            .iter()
            .zip(&blue_bars)
            .map(|(r, b)| {
                if horizontal {
                    r.width + b.width
                } else {
                    r.height + b.height
                }
            })
            .collect();
        for span in &spans[1..] {
            checks::near(*span, spans[0]);
        }
        // Q1 为 10 vs 30 => 25% vs 75%
        let ratio = if horizontal {
            red_bars[0].width / spans[0]
        } else {
            red_bars[0].height / spans[0]
        };
        checks::near(ratio, 0.25);
    }
    Ok(())
}

#[test]
fn stacked_bar_chart_separates_positive_and_negative_stacks_around_zero() -> TestResult {
    let cat = r#"<c:cat><c:strLit><c:ptCount val="2"/><c:pt idx="0"><c:v>A</c:v></c:pt><c:pt idx="1"><c:v>B</c:v></c:pt></c:strLit></c:cat>"#;
    let val_a = r#"<c:val><c:numLit><c:ptCount val="2"/><c:pt idx="0"><c:v>10</c:v></c:pt><c:pt idx="1"><c:v>-10</c:v></c:pt></c:numLit></c:val>"#;
    let val_b = r#"<c:val><c:numLit><c:ptCount val="2"/><c:pt idx="0"><c:v>-20</c:v></c:pt><c:pt idx="1"><c:v>-10</c:v></c:pt></c:numLit></c:val>"#;
    let bar_chart = format!(
        r#"<c:barChart><c:barDir val="bar"/><c:grouping val="stacked"/>{}{}<c:axId val="100"/><c:axId val="200"/></c:barChart>"#,
        fixture::series((0, "Pos", "C03050"), cat, val_a),
        fixture::series((1, "Neg", "2070C0"), cat, val_b),
    );
    let chart_xml = fixture::category_chart()
        .replace("<c:lineChart><c:grouping val=\"standard\"/>", &bar_chart)
        .replace("</c:lineChart>", "");

    let document = document(&chart_xml)?;
    let page = &document.pages[0];
    let mut red_bars: Vec<_> = page
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::RECT) && e.fill == RED && e.height > 8.0)
        .collect();
    let mut blue_bars: Vec<_> = page
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::RECT) && e.fill == BLUE && e.height > 8.0)
        .collect();
    red_bars.sort_by(|a, b| a.y.total_cmp(&b.y));
    blue_bars.sort_by(|a, b| a.y.total_cmp(&b.y));
    assert_eq!(red_bars.len(), 2);
    assert_eq!(blue_bars.len(), 2);

    // 分类 A：RED(+10) 在零线右侧，BLUE(-20) 在零线左侧，两者在零线相接
    checks::near(blue_bars[0].x + blue_bars[0].width, red_bars[0].x);
    // 分类 B：RED(-10) 与 BLUE(-10) 均在零线左侧，BLUE 紧接在 RED 左侧
    checks::near(blue_bars[1].x + blue_bars[1].width, red_bars[1].x);
    checks::near(red_bars[1].x + red_bars[1].width, red_bars[0].x);
    Ok(())
}

#[test]
fn stacked_and_percent_stacked_line_charts_accumulate_series_values() -> TestResult {
    // Stacked line: RED=[10,25,15,30], BLUE=[10+30=40, 25+20=45, 15+25=40, 30+10=40]
    let stacked = fixture::category_chart()
        .replace(r#"grouping val="standard""#, r#"grouping val="stacked""#);
    let doc_stacked = document(&stacked)?;
    let page = &doc_stacked.pages[0];
    let red_segs = data_segments(page, RED);
    assert_eq!(red_segs.len(), 3, "RED stacked line should have 3 segments");
    // BLUE 在 4 个分类处堆积值为 [40, 45, 40, 40]，第 1 段上升、第 2 段下降、第 3 段水平
    let blue_all: Vec<_> = page
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::LINE) && e.stroke == BLUE && e.width > 20.0)
        .map(fixture::endpoints)
        .collect();
    assert_eq!(
        blue_all.len(),
        3,
        "BLUE stacked line should have 3 segments"
    );
    // 第 1 分类处 BLUE(40) 必须在 RED(10) 上方（更小的 y）
    let mut blue_sorted = blue_all;
    blue_sorted.sort_by(|a, b| a[0].0.total_cmp(&b[0].0));
    assert!(blue_sorted[0][0].1 < red_segs[0][0].1);
    assert!(blue_sorted[0][1].1 < blue_sorted[0][0].1, "40 -> 45 rises");
    assert!(blue_sorted[1][1].1 > blue_sorted[1][0].1, "45 -> 40 falls");
    checks::near(blue_sorted[2][0].1, blue_sorted[2][1].1);

    // Percent stacked line: BLUE 始终位于 100% 顶线（水平），RED 为 [0.25, 25/45, 0.375, 0.75]
    let pct = fixture::category_chart().replace(
        r#"grouping val="standard""#,
        r#"grouping val="percentStacked""#,
    );
    let doc_pct = document(&pct)?;
    let page_pct = &doc_pct.pages[0];
    let red_pct = data_segments(page_pct, RED);
    assert_eq!(red_pct.len(), 3);
    let mut blue_pct: Vec<_> = page_pct
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::LINE) && e.stroke == BLUE && e.width > 20.0)
        .map(fixture::endpoints)
        .collect();
    blue_pct.sort_by(|a, b| a[0].0.total_cmp(&b[0].0));
    assert_eq!(blue_pct.len(), 3);
    for seg in &blue_pct {
        checks::near(seg[0].1, seg[1].1);
    }
    assert!(blue_pct[0][0].1 < red_pct[0][0].1);
    Ok(())
}

#[test]
fn stacked_and_percent_stacked_area_charts_render_non_overlapping_bands() -> TestResult {
    for grouping in ["stacked", "percentStacked"] {
        let chart = area_chart().replace(
            r#"grouping val="standard""#,
            &format!(r#"grouping val="{grouping}""#),
        );
        let doc = document(&chart)?;
        let page = &doc.pages[0];
        let red_areas = area_paths(page, RED);
        let blue_areas = area_paths(page, BLUE);
        assert_eq!(red_areas.len(), 1, "{grouping} expected 1 RED area path");
        assert_eq!(blue_areas.len(), 1, "{grouping} expected 1 BLUE area path");
        assert_eq!(path_command_count(red_areas[0], PathCommandKind::Close), 3);
        assert_eq!(path_command_count(blue_areas[0], PathCommandKind::Close), 3);

        // BLUE 堆积在 RED 之上：BLUE 的最小 y（顶部）应严格小于 RED 的最小 y
        let min_y = |el: &jz_office_core::model::Element| -> f32 {
            el.paths
                .iter()
                .flat_map(|p| &p.commands)
                .filter_map(|c| match *c {
                    PathCommand::Move([_, y]) | PathCommand::Line([_, y]) => Some(el.y + y),
                    _ => None,
                })
                .fold(f32::MAX, f32::min)
        };
        assert!(
            min_y(blue_areas[0]) < min_y(red_areas[0]),
            "{grouping}: BLUE area band should sit above RED"
        );
        assert!(doc.warnings.iter().any(|w| w.contains("area charts")));
    }
    Ok(())
}

#[test]
fn charts_3d_flatten_to_2d_with_optional_series_axis_and_warning() -> TestResult {
    // 1. pie3DChart（无坐标轴）
    let pie3d = r#"<?xml version="1.0" encoding="UTF-8"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"
xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<c:chart><c:plotArea><c:layout/>
<c:pie3DChart><c:ser><c:idx val="0"/><c:order val="0"/>
<c:dPt><c:idx val="0"/><c:spPr><a:solidFill><a:srgbClr val="C03050"/></a:solidFill></c:spPr></c:dPt>
<c:cat><c:strLit><c:ptCount val="2"/><c:pt idx="0"><c:v>A</c:v></c:pt><c:pt idx="1"><c:v>B</c:v></c:pt></c:strLit></c:cat>
<c:val><c:numLit><c:ptCount val="2"/><c:pt idx="0"><c:v>70</c:v></c:pt><c:pt idx="1"><c:v>30</c:v></c:pt></c:numLit></c:val>
</c:ser></c:pie3DChart></c:plotArea></c:chart></c:chartSpace>"#;
    let doc_pie = document(pie3d)?;
    let sectors = doc_pie.pages[0]
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::PATH))
        .count();
    assert_eq!(sectors, 2, "pie3DChart should emit 2 flattened sectors");
    assert!(doc_pie
        .warnings
        .iter()
        .any(|w| w.contains("3D charts are flattened to 2D")));

    // 2. bar3DChart（含可选 serAx 深度轴）
    let bar3d = fixture::category_chart()
        .replace(
            "<c:lineChart><c:grouping val=\"standard\"/>",
            "<c:bar3DChart><c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>",
        )
        .replace(
            "<c:axId val=\"200\"/></c:lineChart>",
            "<c:axId val=\"200\"/><c:axId val=\"300\"/></c:bar3DChart>",
        )
        .replace(
            "</c:valAx>",
            "</c:valAx><c:serAx><c:axId val=\"300\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:crossAx val=\"200\"/></c:serAx>",
        );
    let doc_bar = document(&bar3d)?;
    let bar_count = doc_bar.pages[0]
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementType::RECT) && e.fill == RED && e.width > 12.0)
        .count();
    assert_eq!(bar_count, 4, "bar3DChart with serAx should emit 4 RED bars");
    assert!(doc_bar
        .warnings
        .iter()
        .any(|w| w.contains("3D charts are flattened to 2D")));

    // 3. line3DChart 与 area3DChart（含可选 serAx）
    for (tag, is_area) in [("line3DChart", false), ("area3DChart", true)] {
        let xml = fixture::category_chart()
            .replace("<c:lineChart>", &format!("<c:{tag}>"))
            .replace(
                "<c:axId val=\"200\"/></c:lineChart>",
                &format!("<c:axId val=\"200\"/><c:axId val=\"300\"/></c:{tag}>"),
            )
            .replace(
                "</c:valAx>",
                "</c:valAx><c:serAx><c:axId val=\"300\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:crossAx val=\"200\"/></c:serAx>",
            );
        let doc = document(&xml)?;
        let page = &doc.pages[0];
        assert_eq!(data_segments(page, RED).len(), 3, "{tag} missing RED line");
        if is_area {
            assert_eq!(area_paths(page, RED).len(), 1, "{tag} missing RED area");
        }
        assert!(doc
            .warnings
            .iter()
            .any(|w| w.contains("3D charts are flattened to 2D")));
    }
    Ok(())
}

fn area_chart() -> String {
    fixture::category_chart().replace("lineChart", "areaChart")
}

fn area_paths(page: &Page, color: u32) -> Vec<&jz_office_core::model::Element> {
    page.elements
        .iter()
        .filter(|element| matches!(element.kind, ElementType::PATH) && element.fill == color)
        .collect()
}

#[derive(Clone, Copy)]
enum PathCommandKind {
    Move,
    Close,
}

fn path_command_count(element: &jz_office_core::model::Element, kind: PathCommandKind) -> usize {
    element
        .paths
        .iter()
        .flat_map(|path| &path.commands)
        .filter(|command| {
            matches!(
                (kind, command),
                (PathCommandKind::Move, PathCommand::Move(_))
                    | (PathCommandKind::Close, PathCommand::Close)
            )
        })
        .count()
}

fn document(chart: &str) -> TestResult<Document> {
    let mut parts = fixture::parts();
    replace(&mut parts, FIRST_CHART, chart);
    replace(
        &mut parts,
        "ppt/slides/slide1.xml",
        r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree/></p:cSld></p:sld>"#,
    );
    Ok(parse_reader(Cursor::new(archive(&parts)?))?)
}

fn assert_omitted(chart: &str) -> TestResult {
    let document = document(chart)?;
    assert!(
        document.pages[0].elements.iter().all(|element| {
            !matches!(element.kind, ElementType::LINE) || ![RED, BLUE].contains(&element.stroke)
        }),
        "Rejected chart emitted series lines"
    );
    assert!(
        document.pages[0].elements.iter().all(|element| {
            !matches!(element.kind, ElementType::RECT) || ![RED, BLUE].contains(&element.fill)
        }),
        "Rejected chart emitted series bars"
    );
    assert!(document.pages[0].elements.iter().all(|element| {
        !matches!(element.kind, ElementType::PATH) || ![RED, BLUE].contains(&element.fill)
    }));
    assert!(
        document
            .warnings
            .iter()
            .any(|warning| warning.to_lowercase().contains("chart")),
        "Missing chart warning: {:?}",
        document.warnings
    );
    Ok(())
}

fn texts(page: &Page) -> Vec<String> {
    page.elements
        .iter()
        .filter(|element| matches!(element.kind, ElementType::TEXT))
        .map(|element| checks::text(&element.paragraphs))
        .collect()
}
