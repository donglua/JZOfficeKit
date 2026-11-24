use super::{
    drawing, flag,
    geometry::{points, Bounds},
};
use crate::{
    model::{Element, ElementType},
    xml::{number, Node},
};

/// Maps child coordinates to slide coordinates: x' = ax + cy + e, y' = bx + dy + f.
#[derive(Clone, Copy)]
pub(crate) struct Transform(pub [f32; 6]);

impl Transform {
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    /// 将外层变换与容器框架自身的平移/旋转/翻转合并为子坐标系的仿射矩阵。
    pub(crate) fn frame(self, bounds: Bounds) -> Option<Self> {
        let mut element = bounds.element(ElementType::RECT);
        element.transform = self.0;
        let combined = drawing::element_transform(&element);
        combined
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 100_000.0)
            .then_some(Self(combined))
    }

    pub(crate) fn group(self, group: &Node) -> Option<Self> {
        let node = group.child("grpSpPr")?.child("xfrm")?;
        let off = node.child("off")?;
        let ext = node.child("ext")?;
        let (x, y) = (points(off.attr("x"))?, points(off.attr("y"))?);
        let (w, h) = (points(ext.attr("cx"))?, points(ext.attr("cy"))?);
        self.combine_group(node, x, y, w, h)
    }

    /// 顶层 XLSX `<xdr:grpSp>` 版本：锚点 `Frame` 已给出群组在数据区的最终外框，
    /// 因此用它替代 `grpSpPr/xfrm` 里的 `off`/`ext`，只保留 `chOff`/`chExt` 与 `rot`。
    pub(crate) fn group_in_frame(
        self,
        group: &Node,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Option<Self> {
        let node = group.child("grpSpPr")?.child("xfrm")?;
        self.combine_group(node, x, y, width, height)
    }

    /// 由容器外框 `(x, y, w, h)` 与 `xfrm` 中的 `chOff`/`chExt`/`rot` 构建子坐标系仿射矩阵。
    fn combine_group(self, node: &Node, x: f32, y: f32, w: f32, h: f32) -> Option<Self> {
        let child_off = node.child("chOff")?;
        let child_ext = node.child("chExt")?;
        let (cx, cy) = (points(child_off.attr("x"))?, points(child_off.attr("y"))?);
        let (cw, ch) = (points(child_ext.attr("cx"))?, points(child_ext.attr("cy"))?);
        if w <= 0.0 || h <= 0.0 || cw <= 0.0 || ch <= 0.0 {
            return None;
        }
        let (sx, sy) = (w / cw, h / ch);
        let angle = (number(node.attr("rot"), 0.0) / 60000.0) % 360.0;
        let (sin, cos) = angle.to_radians().sin_cos();
        let (dx, dy) = (-cx * sx - w / 2.0, -cy * sy - h / 2.0);
        let child = [
            cos * sx,
            sin * sx,
            -sin * sy,
            cos * sy,
            x + w / 2.0 + cos * dx - sin * dy,
            y + h / 2.0 + sin * dx + cos * dy,
        ];
        let [a, b, c, d, e, f] = self.0;
        let [g, h, i, j, k, l] = child;
        let combined = [
            a * g + c * h,
            b * g + d * h,
            a * i + c * j,
            b * i + d * j,
            a * k + c * l + e,
            b * k + d * l + f,
        ];
        combined
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 100_000.0)
            .then_some(Self(combined))
    }

    pub(crate) fn accepts(self, element: &Element) -> bool {
        let [a, b, c, d, e, f] = self.0;
        let (sin, cos) = element.rotation.to_radians().sin_cos();
        let (hw, hh) = (element.width / 2.0, element.height / 2.0);
        [(-hw, -hh), (hw, -hh), (-hw, hh), (hw, hh)]
            .iter()
            .all(|&(dx, dy)| {
                let x = element.x + hw + cos * dx - sin * dy;
                let y = element.y + hh + sin * dx + cos * dy;
                [a * x + c * y + e, b * x + d * y + f]
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= 100_000.0)
            })
    }
}

pub(crate) fn flipped(group: &Node) -> bool {
    group
        .child("grpSpPr")
        .and_then(|p| p.child("xfrm"))
        .is_some_and(|n| flag(n.attr("flipH"), false) || flag(n.attr("flipV"), false))
}
