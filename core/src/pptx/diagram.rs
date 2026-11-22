use super::geometry::Bounds;
use super::parts::Part;
use super::relation_id;
use super::render::{Render, ShapeTree};
use super::transform::Transform;
use crate::model::Page;
use crate::xml::Node;
use crate::Result;

impl Render<'_> {
    /// 读取 SmartArt 缓存的 `diagramDrawing` 部件（`<dsp:spTree>`），
    /// 按外层 `graphicFrame` 的几何框映射坐标并复用常规形状/文本/图片渲染管线。
    pub(super) fn diagram(
        &mut self,
        part: &Part,
        rel_ids: &Node,
        raw_bounds: Bounds,
        parent_transform: Transform,
        page: &mut Page,
    ) -> Result<()> {
        let Some(drawing_path) = self.resolve_diagram_drawing(part, rel_ids)? else {
            self.doc.warn("PPTX SmartArt is omitted.");
            return Ok(());
        };
        let drawing_part = Part::read(self.pkg, &drawing_path)?;
        let Some(sp_tree) = drawing_part.root.child("spTree") else {
            self.doc.warn("PPTX SmartArt is omitted.");
            return Ok(());
        };
        let Some(frame_transform) = parent_transform.frame(raw_bounds) else {
            self.doc
                .warn("PPTX elements with excessive transformed bounds were omitted.");
            return Ok(());
        };
        let tree_transform = frame_transform.group(sp_tree).unwrap_or(frame_transform);
        super::unsupported::warn(&drawing_part.root, self.doc);
        self.doc
            .warn("PPTX SmartArt uses saved diagram drawings; layout is not recalculated.");
        self.shapes(
            sp_tree,
            ShapeTree {
                part: &drawing_part,
                inherited: true,
                transform: tree_transform,
            },
            page,
        )
    }

    fn resolve_diagram_drawing(&mut self, part: &Part, rel_ids: &Node) -> Result<Option<String>> {
        let dm_id = relation_id(rel_ids, "dm");
        let data_path = (!dm_id.is_empty())
            .then(|| part.rels.get(dm_id))
            .flatten()
            .filter(|path| self.pkg.has(path))
            .cloned();

        if let Some(data_path) = data_path {
            let data_part = Part::read(self.pkg, &data_path)?;
            if let Some(rel_id) = data_part
                .root
                .descendant("dataModelExt")
                .map(|node| node.attr("relId"))
                .filter(|id| !id.is_empty())
            {
                if let Some(path) = part
                    .rels
                    .get(rel_id)
                    .or_else(|| data_part.rels.get(rel_id))
                    .filter(|path| self.pkg.has(path))
                {
                    return Ok(Some(path.clone()));
                }
            }
            if let Some(path) = self
                .pkg
                .related_by_type(&data_part.path, "diagramDrawing")?
                .filter(|path| self.pkg.has(path))
            {
                return Ok(Some(path));
            }
        }

        Ok(self
            .pkg
            .related_by_type(&part.path, "diagramDrawing")?
            .filter(|path| self.pkg.has(path)))
    }
}
