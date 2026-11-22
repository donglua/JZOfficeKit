use super::geometry::Bounds;
use super::parts::Part;
use super::render::Render;
use crate::model::{Element, ElementType};
use crate::xml::Node;
use crate::Result;

impl Render<'_> {
    pub fn chart(&mut self, part: &Part, reference: &Node, bounds: Bounds) -> Result<Vec<Element>> {
        let Some(path) = part
            .rels
            .get(super::relation_id(reference, "id"))
            .filter(|path| self.pkg.has(path))
        else {
            self.doc
                .warn("PPTX charts with missing or external parts were omitted.");
            return Ok(Vec::new());
        };
        let root = self.pkg.xml(path)?;
        let frame = bounds.element(ElementType::RECT);
        let Some(mut elements) = crate::chart::render(
            &root,
            frame.width,
            frame.height,
            self.theme,
            self.doc,
            &mut |bytes| self.budget.text(bytes),
        )?
        else {
            return Ok(Vec::new());
        };
        for element in &mut elements {
            place(element, &frame);
        }
        Ok(elements)
    }
}

fn place(element: &mut Element, frame: &Element) {
    let mut dx = element.x + element.width / 2.0 - frame.width / 2.0;
    let mut dy = element.y + element.height / 2.0 - frame.height / 2.0;
    if frame.flip_h {
        dx = -dx;
        element.flip_h = !element.flip_h;
    }
    if frame.flip_v {
        dy = -dy;
        element.flip_v = !element.flip_v;
    }
    let (sin, cos) = frame.rotation.to_radians().sin_cos();
    element.x = frame.x + frame.width / 2.0 + cos * dx - sin * dy - element.width / 2.0;
    element.y = frame.y + frame.height / 2.0 + sin * dx + cos * dy - element.height / 2.0;
    element.rotation = frame.rotation;
}
