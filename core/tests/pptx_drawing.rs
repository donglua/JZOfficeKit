pub mod support;

use jz_office_core::parse_reader;
use std::io::Cursor;
use support::package::{archive, replace, TestResult};

fn slide(content: &str) -> TestResult<serde_json::Value> {
    let mut parts = support::pptx::parts();
    replace(
        &mut parts,
        "ppt/slides/slide2.xml",
        &format!(
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld>{content}</p:cSld></p:sld>"#
        ),
    );
    let document = parse_reader(Cursor::new(archive(&parts)?))?;
    Ok(serde_json::to_value(document)?)
}

fn near_array(actual: &serde_json::Value, expected: &[f64]) -> TestResult {
    let values = actual.as_array().ok_or("missing geometry array")?;
    assert_eq!(values.len(), expected.len());
    for (actual, expected) in values.iter().zip(expected) {
        let actual = actual.as_f64().ok_or("invalid geometry coordinate")?;
        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    }
    Ok(())
}

#[test]
fn shape_transform_includes_position_centered_rotation_and_flip() -> TestResult {
    let json = slide(
        r#"<p:spTree><p:sp><p:spPr><a:xfrm rot="5400000" flipH="1"><a:off x="127000" y="254000"/><a:ext cx="508000" cy="254000"/></a:xfrm><a:prstGeom prst="rect"/><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></p:spPr></p:sp></p:spTree>"#,
    )?;
    let element = &json["pages"][0]["elements"][0];
    near_array(&element["transform"], &[0.0, -1.0, -1.0, 0.0, 40.0, 50.0])?;
    assert_eq!(json["schemaVersion"], 9);
    for field in ["rotation", "flipH", "flipV", "imageCrop"] {
        assert!(
            element.get(field).is_none(),
            "obsolete drawing field {field}"
        );
    }
    Ok(())
}

#[test]
fn rounded_transform_over_coordinate_limit_is_omitted_with_warning() -> TestResult {
    let json = slide(
        r#"<p:spTree><p:sp><p:spPr><a:xfrm rot="1800000" flipH="1"><a:off x="1269985024" y="0"/><a:ext cx="12700" cy="12700"/></a:xfrm><a:prstGeom prst="rect"/><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></p:spPr></p:sp></p:spTree>"#,
    )?;
    assert_eq!(json["pages"][0]["elements"], serde_json::json!([]));
    assert!(json["warnings"]
        .as_array()
        .ok_or("missing warnings")?
        .iter()
        .any(|warning| warning == "PPTX elements with excessive transformed bounds were omitted."));
    Ok(())
}

#[test]
fn gradient_endpoints_account_for_aspect_ratio_and_scaled_flag() -> TestResult {
    for (scaled, expected) in [
        ("1", [0.0, 0.0, 200.0, 100.0]),
        ("0", [25.0, -25.0, 175.0, 125.0]),
    ] {
        let json = slide(&format!(
            r#"<p:spTree><p:sp><p:spPr><a:xfrm><a:off x="127000" y="254000"/><a:ext cx="2540000" cy="1270000"/></a:xfrm><a:prstGeom prst="rect"/>{}</p:spPr></p:sp></p:spTree>"#,
            gradient("2700000", scaled)
        ))?;
        let fill = &json["pages"][0]["elements"][0]["fillGradient"];
        near_array(&fill["points"], &expected)?;
        assert!(fill["transform"].is_null());
        for field in ["angle", "scaled", "inSlideSpace"] {
            assert!(fill.get(field).is_none());
        }
    }
    Ok(())
}

#[test]
fn cropped_image_has_local_destination_bounds() -> TestResult {
    let json = slide(
        r#"<p:spTree><p:pic><p:blipFill><a:blip r:embed="rImage"/><a:srcRect l="25000" t="25000"/></p:blipFill><p:spPr><a:xfrm flipV="1"><a:off x="127000" y="254000"/><a:ext cx="762000" cy="381000"/></a:xfrm></p:spPr></p:pic></p:spTree>"#,
    )?;
    let image = &json["pages"][0]["elements"][0];
    near_array(&image["imageBounds"], &[-20.0, -10.0, 60.0, 30.0])?;
    near_array(&image["transform"], &[1.0, 0.0, 0.0, -1.0, 10.0, 50.0])
}

#[test]
fn background_gradient_keeps_slide_coordinates_under_rotated_flipped_shape() -> TestResult {
    let json = slide(&format!(
        r#"<p:bg><p:bgPr>{}</p:bgPr></p:bg><p:spTree><p:sp useBgFill="1"><p:spPr><a:xfrm rot="5400000" flipH="1"><a:off x="127000" y="254000"/><a:ext cx="508000" cy="254000"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr></p:sp></p:spTree>"#,
        gradient("5400000", "0")
    ))?;
    let background = &json["pages"][0]["elements"][0]["fillGradient"];
    let fill = &json["pages"][0]["elements"][1]["fillGradient"];
    near_array(&fill["points"], &[360.0, 0.0, 360.0, 405.0])?;
    near_array(&fill["transform"], &[0.0, -1.0, -1.0, 0.0, 50.0, 40.0])?;
    assert_eq!(fill["points"], background["points"]);
    assert!(background["transform"].is_null());
    Ok(())
}

#[test]
fn alternate_content_falls_back_to_saved_picture_and_shape_in_sp_tree_and_group() -> TestResult {
    let json = slide(
        r#"<p:spTree>
          <mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
            <mc:Choice Requires="cx1"><p:graphicFrame><p:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/drawing/2014/chartex"/></a:graphic></p:graphicFrame></mc:Choice>
            <mc:Fallback><p:pic><p:blipFill><a:blip r:embed="rImage"/></p:blipFill><p:spPr><a:xfrm><a:off x="127000" y="254000"/><a:ext cx="762000" cy="381000"/></a:xfrm></p:spPr></p:pic></mc:Fallback>
          </mc:AlternateContent>
          <p:grpSp>
            <p:grpSpPr><a:xfrm><a:off x="127000" y="127000"/><a:ext cx="2540000" cy="1270000"/><a:chOff x="0" y="0"/><a:chExt cx="2540000" cy="1270000"/></a:xfrm></p:grpSpPr>
            <mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
              <mc:Choice Requires="a14"><p:sp><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>Unchosen</a:t></a:r></a:p></p:txBody></p:sp></mc:Choice>
              <mc:Fallback><p:sp><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm><a:prstGeom prst="rect"/><a:solidFill><a:srgbClr val="112233"/></a:solidFill></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>Fallback shape</a:t></a:r></a:p></p:txBody></p:sp></mc:Fallback>
            </mc:AlternateContent>
          </p:grpSp>
        </p:spTree>"#,
    )?;
    let elements = json["pages"][0]["elements"]
        .as_array()
        .ok_or("missing elements")?;
    assert_eq!(
        elements.len(),
        3,
        "Expected fallback image + fallback shape rect + fallback shape text"
    );
    assert_eq!(elements[0]["type"], "IMAGE");
    assert_eq!(elements[1]["type"], "RECT");
    assert_eq!(elements[2]["type"], "TEXT");
    let text = elements[2]["paragraphs"][0]["runs"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(text, "Fallback shape");
    let warnings = json["warnings"].as_array().ok_or("missing warnings")?;
    assert!(
        warnings.iter().any(|w| w
            .as_str()
            .is_some_and(|s| s.contains("alternate content uses one saved representation"))),
        "Expected fallback alternate content warning, got: {warnings:?}"
    );
    assert!(
        !warnings.iter().any(|w| w
            .as_str()
            .is_some_and(|s| s.contains("Unsupported PPTX shape content"))),
        "Should not report unsupported shape omission: {warnings:?}"
    );
    Ok(())
}

#[test]
fn alternate_content_selects_supported_choice_without_warning_and_unwraps_inline_runs() -> TestResult
{
    let json = slide(
        r#"<p:spTree>
          <mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
            <mc:Choice Requires="p">
              <p:sp>
                <p:spPr><a:xfrm><a:off x="127000" y="127000"/><a:ext cx="2540000" cy="635000"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr>
                <p:txBody><a:bodyPr/><a:p>
                  <a:r><a:t>Value: </a:t></a:r>
                  <mc:AlternateContent>
                    <mc:Choice Requires="a14"><a:r><a:t>Skipped</a:t></a:r></mc:Choice>
                    <mc:Fallback><a:r><a:t>E=mc2</a:t></a:r></mc:Fallback>
                  </mc:AlternateContent>
                </a:p></p:txBody>
              </p:sp>
            </mc:Choice>
            <mc:Fallback><p:sp><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>Wrong branch</a:t></a:r></a:p></p:txBody></p:sp></mc:Fallback>
          </mc:AlternateContent>
        </p:spTree>"#,
    )?;
    let elements = json["pages"][0]["elements"]
        .as_array()
        .ok_or("missing elements")?;
    assert_eq!(elements.len(), 1);
    let runs = elements[0]["paragraphs"][0]["runs"]
        .as_array()
        .ok_or("missing runs")?;
    let combined: String = runs.iter().filter_map(|r| r["text"].as_str()).collect();
    assert_eq!(combined, "Value: E=mc2");
    Ok(())
}

#[test]
fn alternate_content_without_supported_branch_or_fallback_warns_omitted() -> TestResult {
    let json = slide(
        r#"<p:spTree>
          <mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
            <mc:Choice Requires="cx1"><p:sp><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr></p:sp></mc:Choice>
          </mc:AlternateContent>
        </p:spTree>"#,
    )?;
    assert_eq!(json["pages"][0]["elements"], serde_json::json!([]));
    let warnings = json["warnings"].as_array().ok_or("missing warnings")?;
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str() == Some("PPTX alternate content is omitted.")),
        "Expected omitted alternate content warning, got: {warnings:?}"
    );
    Ok(())
}

#[test]
fn ole_object_renders_fallback_preview_picture_without_unsupported_warning() -> TestResult {
    let json = slide(
        r#"<p:spTree>
          <p:graphicFrame>
            <p:nvGraphicFramePr><p:cNvPr id="7" name="Object 1"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>
            <p:xfrm><a:off x="127000" y="254000"/><a:ext cx="762000" cy="381000"/></p:xfrm>
            <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole">
              <mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
                <mc:Choice Requires="v">
                  <p:oleObj spid="_x0000_s1026" name="Worksheet" r:id="rOle"/>
                </mc:Choice>
                <mc:Fallback>
                  <p:oleObj name="Worksheet" r:id="rOle">
                    <p:embed/>
                    <p:pic>
                      <p:nvPicPr><p:cNvPr id="8" name="Preview"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr>
                      <p:blipFill><a:blip r:embed="rImage"/></p:blipFill>
                      <p:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>
                    </p:pic>
                  </p:oleObj>
                </mc:Fallback>
              </mc:AlternateContent>
            </a:graphicData></a:graphic>
          </p:graphicFrame>
        </p:spTree>"#,
    )?;
    let elements = json["pages"][0]["elements"]
        .as_array()
        .ok_or("missing elements")?;
    assert_eq!(elements.len(), 1, "Expected 1 OLE preview image");
    assert_eq!(elements[0]["type"], "IMAGE");
    assert_eq!(elements[0]["image"], "ppt/media/pixel.png");
    near_array(&elements[0]["transform"], &[1.0, 0.0, 0.0, 1.0, 10.0, 20.0])?;
    assert_eq!(elements[0]["width"], 60.0);
    assert_eq!(elements[0]["height"], 30.0);

    let warnings = json["warnings"].as_array().ok_or("missing warnings")?;
    assert!(
        warnings.iter().any(|w| w.as_str()
            == Some("PPTX embedded OLE objects display their saved preview pictures.")),
        "Expected OLE preview warning, got: {warnings:?}"
    );
    assert!(
        !warnings
            .iter()
            .any(|w| w.as_str() == Some("PPTX multimedia and embedded objects are not supported.")),
        "Should not emit unsupported OLE warning when preview renders: {warnings:?}"
    );
    Ok(())
}

#[test]
fn direct_ole_object_with_and_without_preview_picture() -> TestResult {
    let with_pic = slide(
        r#"<p:spTree>
          <p:graphicFrame>
            <p:xfrm><a:off x="254000" y="127000"/><a:ext cx="635000" cy="635000"/></p:xfrm>
            <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole">
              <p:oleObj name="Equation">
                <p:embed/>
                <p:pic>
                  <p:blipFill><a:blip r:embed="rImage"/></p:blipFill>
                  <p:spPr/>
                </p:pic>
              </p:oleObj>
            </a:graphicData></a:graphic>
          </p:graphicFrame>
        </p:spTree>"#,
    )?;
    let elements = with_pic["pages"][0]["elements"]
        .as_array()
        .ok_or("missing elements")?;
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0]["type"], "IMAGE");
    near_array(&elements[0]["transform"], &[1.0, 0.0, 0.0, 1.0, 20.0, 10.0])?;

    let without_pic = slide(
        r#"<p:spTree>
          <p:graphicFrame>
            <p:xfrm><a:off x="0" y="0"/><a:ext cx="635000" cy="635000"/></p:xfrm>
            <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole">
              <p:oleObj name="Equation"><p:embed/></p:oleObj>
            </a:graphicData></a:graphic>
          </p:graphicFrame>
        </p:spTree>"#,
    )?;
    assert_eq!(without_pic["pages"][0]["elements"], serde_json::json!([]));
    let warnings = without_pic["warnings"]
        .as_array()
        .ok_or("missing warnings")?;
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str() == Some("PPTX multimedia and embedded objects are not supported.")),
        "Expected unsupported warning when preview pic is absent: {warnings:?}"
    );
    Ok(())
}

fn gradient(angle: &str, scaled: &str) -> String {
    format!(
        r#"<a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs></a:gsLst><a:lin ang="{angle}" scaled="{scaled}"/></a:gradFill>"#
    )
}

#[test]
fn picture_supports_ellipse_round_rect_and_preset_masks_with_stroke() -> TestResult {
    let json = slide(
        r#"<p:spTree>
          <p:pic>
            <p:blipFill><a:blip r:embed="rImage"/></p:blipFill>
            <p:spPr>
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="1270000"/></a:xfrm>
              <a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom>
              <a:ln w="25400"><a:solidFill><a:srgbClr val="1A73E8"/></a:solidFill></a:ln>
            </p:spPr>
          </p:pic>
          <p:pic>
            <p:blipFill><a:blip r:embed="rImage"/></p:blipFill>
            <p:spPr>
              <a:xfrm><a:off x="1270000" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm>
              <a:prstGeom prst="roundRect"><a:avLst><a:gd name="adj" fmla="val 25000"/></a:avLst></a:prstGeom>
            </p:spPr>
          </p:pic>
          <p:pic>
            <p:blipFill><a:blip r:embed="rImage"/></p:blipFill>
            <p:spPr>
              <a:xfrm><a:off x="2540000" y="0"/><a:ext cx="1270000" cy="1270000"/></a:xfrm>
              <a:prstGeom prst="star5"><a:avLst/></a:prstGeom>
            </p:spPr>
          </p:pic>
        </p:spTree>"#,
    )?;
    let elements = json["pages"][0]["elements"]
        .as_array()
        .ok_or("missing elements")?;
    assert_eq!(elements.len(), 3);
    for el in elements {
        assert_eq!(el["type"], "IMAGE");
        assert!(
            el["paths"]
                .as_array()
                .is_some_and(|paths| !paths.is_empty()),
            "expected non-rectangular mask paths: {el}"
        );
    }
    assert_eq!(elements[0]["stroke"], 0xff1a_73e8_u32);
    assert_eq!(elements[0]["strokeWidth"], 2.0);
    let warnings = json["warnings"].as_array().ok_or("missing warnings")?;
    assert!(
        !warnings
            .iter()
            .any(|w| w.as_str() == Some("PPTX picture masks use rectangular bounds.")),
        "supported picture masks must not emit fallback warning: {warnings:?}"
    );
    Ok(())
}

#[test]
fn picture_unsupported_mask_warns_and_falls_back_to_rect() -> TestResult {
    let json = slide(
        r#"<p:spTree>
          <p:pic>
            <p:blipFill><a:blip r:embed="rImage"/></p:blipFill>
            <p:spPr>
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm>
              <a:prstGeom prst="cloud"><a:avLst/></a:prstGeom>
            </p:spPr>
          </p:pic>
        </p:spTree>"#,
    )?;
    let elements = json["pages"][0]["elements"]
        .as_array()
        .ok_or("missing elements")?;
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0]["type"], "IMAGE");
    assert!(elements[0].get("paths").is_none());
    let warnings = json["warnings"].as_array().ok_or("missing warnings")?;
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str() == Some("PPTX picture masks use rectangular bounds.")),
        "expected rectangular fallback warning: {warnings:?}"
    );
    Ok(())
}
