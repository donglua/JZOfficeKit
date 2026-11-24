use super::{element_transform, gradient_points, inverse, prepare, prepare_element};
use crate::model::{Document, Element, ElementType, GradientFill, ImageCrop, Kind, Page};

#[test]
fn zero_height_line_rotates_and_flips_around_its_actual_center() {
    let line = Element {
        x: 80.0,
        y: 40.0,
        width: 40.0,
        rotation: 90.0,
        flip_h: true,
        ..Element::default()
    };
    assert_eq!(
        element_transform(&line),
        [0.0, -1.0, -1.0, 0.0, 100.0, 60.0]
    );
}

#[test]
fn nearly_axis_aligned_rotation_keeps_android_snapping() {
    let element = Element {
        x: 10.0,
        y: 20.0,
        width: 100.0,
        height: 50.0,
        rotation: 0.0005,
        ..Element::default()
    };
    assert_eq!(
        element_transform(&element),
        [1.0, 0.0, 0.0, 1.0, 10.0, 20.0]
    );
}

#[test]
fn inverse_preserves_shear_and_translation() {
    assert_eq!(
        inverse([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        Some([-2.0, 1.0, 1.5, -0.5, 1.0, -2.0])
    );
}

#[test]
fn inverse_rejects_singular_and_nearly_singular_affine_matrices() {
    assert!(inverse([1.0, 1.0, 1.0, 1.0, 0.0, 0.0]).is_none());
    assert!(inverse([0.000001, 0.000001, 0.0, 0.000001, 0.0, 0.0]).is_none());
    assert!(inverse([0.0, 0.0, 0.0, 1.0, 0.0, 0.0]).is_none());
}

#[test]
fn inverse_retains_small_diagonal_scales() {
    assert_eq!(
        inverse([0.000001, 0.0, 0.0, 0.000001, 0.0, 0.0]),
        Some([1000000.0, 0.0, 0.0, 1000000.0, 0.0, 0.0])
    );
}

#[test]
fn zero_area_scaled_gradient_uses_horizontal_direction() {
    assert_eq!(gradient_points(&gradient(), [0.0, 0.0]), [0.0; 4]);
}

#[test]
fn singular_background_transform_retains_solid_fill_and_warns() {
    let mut document = Document::new(Kind::PPTX);
    let page = Page {
        width: 720.0,
        height: 405.0,
        background: 0xffffffff,
        elements: Vec::new(),
    };
    let mut fill = gradient();
    fill.in_slide_space = true;
    let mut element = Element {
        width: 40.0,
        height: 20.0,
        fill: 0xff123456,
        fill_gradient: Some(fill),
        transform: [1.0, 1.0, 1.0, 1.0, 0.0, 0.0],
        ..Element::default()
    };

    assert!(prepare(&mut element, &page, &mut document));

    assert!(element.fill_gradient.is_none());
    assert_eq!(element.fill, 0xff123456);
    assert_eq!(
        document.warnings,
        ["Slide background fill has an invalid shape transform"]
    );
}

fn gradient() -> GradientFill {
    GradientFill {
        colors: vec![0xffff0000, 0xff0000ff],
        positions: vec![0.0, 1.0],
        points: [0.0; 4],
        transform: None,
        angle: 45.0,
        scaled: true,
        in_slide_space: false,
    }
}

#[test]
fn prepare_element_computes_transform_crop_and_local_gradient() {
    let mut doc = Document::new(Kind::XLSX);
    let mut element = Element {
        kind: ElementType::IMAGE,
        x: 20.0,
        y: 30.0,
        width: 100.0,
        height: 50.0,
        rotation: 90.0,
        image_crop: Some(ImageCrop {
            left: 0.1,
            top: 0.0,
            right: 0.1,
            bottom: 0.0,
        }),
        ..Element::default()
    };
    assert!(prepare_element(&mut element, &mut doc));
    // srcRect 裁剪把图源按可见比例放大后平移到元素外框内
    let Some(bounds) = element.image_bounds else {
        panic!("crop must produce image bounds");
    };
    for (actual, expected) in bounds.iter().zip([-12.5, 0.0, 112.5, 50.0]) {
        assert!((actual - expected).abs() < 1e-4, "image_bounds {bounds:?}");
    }
    // 旋转 90° 后矩阵分量烘焙进 transform，且平移已包含元素自身 x/y
    assert!((element.transform[1] - 1.0).abs() < 1e-4);
    assert!((element.transform[2] + 1.0).abs() < 1e-4);
    assert!(doc.warnings.is_empty());
}

#[test]
fn prepare_element_drops_slide_space_gradient_without_page_size() {
    // XLSX 工作表没有幻灯片背景：in_slide_space 渐变必须被丢弃而不是产生错误端点
    let mut doc = Document::new(Kind::XLSX);
    let mut fill = gradient();
    fill.in_slide_space = true;
    let mut element = Element {
        kind: ElementType::RECT,
        width: 100.0,
        height: 40.0,
        fill_gradient: Some(fill),
        ..Element::default()
    };
    assert!(prepare_element(&mut element, &mut doc));
    assert!(element.fill_gradient.is_none());
    assert!(doc.warnings.is_empty());
}

#[test]
fn prepare_element_keeps_local_gradient_points() {
    let mut doc = Document::new(Kind::XLSX);
    let mut element = Element {
        kind: ElementType::RECT,
        width: 100.0,
        height: 40.0,
        fill_gradient: Some(gradient()),
        ..Element::default()
    };
    assert!(prepare_element(&mut element, &mut doc));
    let Some(fill) = element.fill_gradient else {
        panic!("local gradient must be preserved");
    };
    // 局部渐变以元素自身宽高为参照，端点必须落在元素中心附近
    assert!((fill.points[0] + fill.points[2]) / 2.0 > 0.0);
    assert!(doc.warnings.is_empty());
}

#[test]
fn prepare_element_rejects_out_of_range_transform() {
    let mut doc = Document::new(Kind::XLSX);
    let mut element = Element {
        kind: ElementType::RECT,
        x: 200_000.0,
        width: 10.0,
        height: 10.0,
        ..Element::default()
    };
    assert!(!prepare_element(&mut element, &mut doc));
}
