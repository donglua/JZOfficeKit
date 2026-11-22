use crate::xml::Node;
use std::collections::HashMap;

pub(super) struct Series<'a> {
    pub node: &'a Node,
    pub values: Vec<Option<f64>>,
    pub x_values: Option<Vec<Option<f64>>>,
    pub categories: Vec<Option<&'a str>>,
    pub dates: Option<Vec<Option<f64>>>,
    pub name: String,
    /// 跨子图表连续递增的配色索引，避免组合图多个子图表回退到同一主题色
    pub color_index: usize,
}

/// 数值轴刻度信息：用于主纵轴与组合图次纵轴的独立缩放和刻度绘制
#[derive(Clone, Copy)]
pub(super) struct ValueAxis<'a> {
    pub axis: &'a Node,
    pub range: (f64, f64),
    pub step: f64,
}

/// 组合图中的单个子图表切片（例如柱状图系列或折线图系列）
pub(super) struct SubChart<'a> {
    /// 具体图表类型节点（如 `lineChart`、`barChart`、`pie3DChart`），用于解析 `<c:dLbls>` 等图表级配置
    pub chart_node: &'a Node,
    /// 分类轴；饼图与环形图没有坐标轴，此时为 `None`
    pub x_axis: Option<&'a Node>,
    /// 数值轴；饼图与环形图没有坐标轴，此时为 `None`
    pub y_axis: Option<&'a Node>,
    pub series: Vec<Series<'a>>,
    pub x_range: (f64, f64),
    pub y_range: (f64, f64),
    pub chart_type: ChartType,
}

/// 折线图、面积图、柱状图与条形图的分组堆叠模式（`<c:grouping val="...">`）。
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Grouping {
    Standard,
    Stacked,
    PercentStacked,
}

/// 散点图样式（`<c:scatterStyle val="...">`）。
#[derive(Clone, Copy, PartialEq)]
pub(super) enum ScatterStyle {
    Line,
    LineMarker,
    Marker,
    None,
}

impl ScatterStyle {
    pub fn has_line(self) -> bool {
        matches!(self, Self::Line | Self::LineMarker)
    }

    pub fn has_marker(self) -> bool {
        matches!(self, Self::LineMarker | Self::Marker)
    }
}

/// 雷达图样式（`<c:radarStyle val="...">`）。
#[derive(Clone, Copy, PartialEq)]
pub(super) enum RadarStyle {
    Standard,
    Marker,
    Filled,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum ChartType {
    Line(Grouping),
    Area(Grouping),
    Column(Grouping),
    /// 横向条形图（`barChart` / `bar3DChart` 且 `barDir="bar"`）
    Bar(Grouping),
    /// XY 散点图
    Scatter(ScatterStyle),
    /// 雷达图
    Radar(RadarStyle),
    /// 饼图（`hole_ratio == 0.0`）与环形图（`hole_ratio > 0.0`）
    Pie {
        hole_ratio: f32,
    },
}

pub(super) struct Chart<'a> {
    pub node: &'a Node,
    pub plot: &'a Node,
    pub sub_charts: Vec<SubChart<'a>>,
    pub primary_x_axis: Option<&'a Node>,
    pub primary_x_step: Option<f64>,
    pub primary_y: Option<ValueAxis<'a>>,
    pub secondary_y: Option<ValueAxis<'a>>,
    pub blanks: &'a str,
    pub date1904: bool,
    /// 是否包含任一 3D 子图表
    pub is_3d: bool,
}

pub(super) fn value<'a>(node: &'a Node, name: &str) -> &'a str {
    node.child(name).map_or("", |n| n.attr("val"))
}

pub(super) fn numeric(text: &str) -> Option<f64> {
    text.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && n.abs() <= 1e100)
}

fn cache(parent: &Node) -> Option<Vec<Option<&str>>> {
    let node = ["numRef", "strRef"]
        .iter()
        .find_map(|name| parent.child(name))
        .and_then(|n| n.child("numCache").or_else(|| n.child("strCache")))
        .or_else(|| parent.child("numLit"))
        .or_else(|| parent.child("strLit"))?;
    let count = value(node, "ptCount").parse::<usize>().ok()?;
    if count == 0 || count > 2048 {
        return None;
    }
    let mut points = vec![None; count];
    for point in node.named("pt") {
        let index = point.attr("idx").parse::<usize>().ok()?;
        let slot = points.get_mut(index)?;
        if slot.is_some() {
            return None;
        }
        *slot = Some(point.child("v")?.text.as_str());
    }
    Some(points)
}

pub(super) fn text(node: &Node) -> String {
    if let Some(values) = cache(node) {
        return values.into_iter().flatten().collect::<Vec<_>>().join(" ");
    }
    if let Some(value) = node.child("v") {
        return value.text.clone();
    }
    fn append(node: &Node, result: &mut String) {
        if node.name == "t" {
            result.push_str(&node.text);
        }
        for child in &node.children {
            append(child, result);
        }
    }
    let mut result = String::new();
    append(node, &mut result);
    result
}

struct RawSubChart<'a> {
    chart_node: &'a Node,
    chart_type: ChartType,
    x_axis: Option<&'a Node>,
    y_axis: Option<&'a Node>,
    series: Vec<Series<'a>>,
    xs: Vec<f64>,
    ys: Vec<f64>,
}

impl<'a> Chart<'a> {
    pub fn parse(root: &'a Node) -> Option<Self> {
        if root.name != "chartSpace" {
            return None;
        }
        let node = root.child("chart")?;
        let plot = node.child("plotArea")?;
        let chart_nodes: Vec<_> = plot
            .children
            .iter()
            .filter(|n| n.name.ends_with("Chart"))
            .collect();
        if chart_nodes.is_empty() {
            return None;
        }
        let mut parsed_types = Vec::with_capacity(chart_nodes.len());
        for &chart_node in &chart_nodes {
            parsed_types.push((chart_node, parse_chart_kind(chart_node)?));
        }
        let is_combo = chart_nodes.len() > 1;
        if is_combo
            && parsed_types.iter().any(|(_, (ct, _))| {
                !matches!(
                    ct,
                    ChartType::Line(_) | ChartType::Area(_) | ChartType::Column(_)
                )
            })
        {
            return None;
        }
        let any_3d = parsed_types.iter().any(|(_, (_, is_3d))| *is_3d);
        let axes: Vec<_> = plot
            .children
            .iter()
            // 3D 图表允许附带深度系列轴 `<c:serAx>`，降维为 2D 时忽略该轴。
            .filter(|n| n.name.ends_with("Ax") && !(any_3d && n.name == "serAx"))
            .collect();
        if is_combo && axes.is_empty() {
            return None;
        }
        let mut axis_by_id = HashMap::with_capacity(axes.len());
        for &axis in &axes {
            let id = value(axis, "axId");
            if id.is_empty()
                || axis_by_id.insert(id, axis).is_some()
                || axis
                    .child("scaling")
                    .is_some_and(|n| n.child("logBase").is_some())
                || axis.child("dispUnits").is_some()
            {
                return None;
            }
        }
        let blanks = match value(node, "dispBlanksAs") {
            "" | "gap" => "gap",
            "zero" => "zero",
            "span" => "span",
            _ => return None,
        };
        let mut total_series = 0usize;
        let mut total_points = 0usize;
        let mut used_axis_ids = Vec::new();
        let mut raw_subs = Vec::with_capacity(parsed_types.len());

        for (chart_node, (chart_type, _sub_is_3d)) in parsed_types {
            let no_axes = matches!(chart_type, ChartType::Pie { .. });
            let (x_axis, y_axis) = if no_axes && axes.is_empty() {
                (None, None)
            } else {
                let (x_ax, y_ax) =
                    resolve_sub_axes(chart_node, chart_type, &axis_by_id, &mut used_axis_ids)?;
                (Some(x_ax), Some(y_ax))
            };
            let series = parse_sub_series(
                chart_node,
                chart_type,
                x_axis,
                &mut total_series,
                &mut total_points,
            )?;
            let mut xs = Vec::new();
            for s in &series {
                for i in 0..s.values.len() {
                    if let Some(x) = s.x(i) {
                        xs.push(x);
                    }
                }
            }
            let ys = collect_y_values(chart_type, &series, blanks)?;
            raw_subs.push(RawSubChart {
                chart_node,
                chart_type,
                x_axis,
                y_axis,
                series,
                xs,
                ys,
            });
        }
        // 单图表与组合图都不允许存在未被引用的孤立坐标轴
        if axes
            .iter()
            .any(|ax| !used_axis_ids.contains(&value(ax, "axId")))
        {
            return None;
        }
        build_chart(node, plot, root, raw_subs, blanks, any_3d)
    }
}

fn parse_chart_kind(chart_node: &Node) -> Option<(ChartType, bool)> {
    match chart_node.name.as_str() {
        "lineChart" | "line3DChart" => {
            let grouping = match value(chart_node, "grouping") {
                "" | "standard" => Grouping::Standard,
                "stacked" => Grouping::Stacked,
                "percentStacked" => Grouping::PercentStacked,
                _ => return None,
            };
            Some((ChartType::Line(grouping), chart_node.name == "line3DChart"))
        }
        "areaChart" | "area3DChart" => {
            let grouping = match value(chart_node, "grouping") {
                "" | "standard" => Grouping::Standard,
                "stacked" => Grouping::Stacked,
                "percentStacked" => Grouping::PercentStacked,
                _ => return None,
            };
            Some((ChartType::Area(grouping), chart_node.name == "area3DChart"))
        }
        "barChart" | "bar3DChart" => {
            let grouping = match value(chart_node, "grouping") {
                "" | "clustered" | "standard" => Grouping::Standard,
                "stacked" => Grouping::Stacked,
                "percentStacked" => Grouping::PercentStacked,
                _ => return None,
            };
            let is_3d = chart_node.name == "bar3DChart";
            match value(chart_node, "barDir") {
                "col" => Some((ChartType::Column(grouping), is_3d)),
                "bar" => Some((ChartType::Bar(grouping), is_3d)),
                _ => None,
            }
        }
        "pieChart" | "pie3DChart" => Some((
            ChartType::Pie { hole_ratio: 0.0 },
            chart_node.name == "pie3DChart",
        )),
        "doughnutChart" => Some((
            ChartType::Pie {
                hole_ratio: hole_ratio(chart_node),
            },
            false,
        )),
        "scatterChart" => {
            let style = match value(chart_node, "scatterStyle") {
                "" | "lineMarker" | "smoothMarker" => ScatterStyle::LineMarker,
                "line" | "smooth" => ScatterStyle::Line,
                "marker" => ScatterStyle::Marker,
                "none" => ScatterStyle::None,
                _ => return None,
            };
            Some((ChartType::Scatter(style), false))
        }
        "radarChart" => {
            let style_node = chart_node.child("radarStyle")?;
            let style = match style_node.attr("val") {
                "" | "standard" => RadarStyle::Standard,
                "marker" => RadarStyle::Marker,
                "filled" => RadarStyle::Filled,
                _ => return None,
            };
            Some((ChartType::Radar(style), false))
        }
        _ => None,
    }
}

fn resolve_sub_axes<'a>(
    chart_node: &Node,
    chart_type: ChartType,
    axis_by_id: &HashMap<&'a str, &'a Node>,
    used_axis_ids: &mut Vec<&'a str>,
) -> Option<(&'a Node, &'a Node)> {
    if matches!(chart_type, ChartType::Scatter(_)) {
        let mut matched = Vec::new();
        for id_node in chart_node.named("axId") {
            let id = id_node.attr("val");
            let Some(&axis) = axis_by_id.get(id) else {
                continue;
            };
            if !matches!(axis.name.as_str(), "valAx" | "dateAx") {
                return None;
            }
            let canonical_id = value(axis, "axId");
            if !used_axis_ids.contains(&canonical_id) {
                used_axis_ids.push(canonical_id);
            }
            matched.push(axis);
        }
        if matched.len() != 2 {
            return None;
        }
        let (a, b) = (matched[0], matched[1]);
        let pos_a = value(a, "axPos");
        let pos_b = value(b, "axPos");
        return if matches!(pos_a, "l" | "r") && matches!(pos_b, "b" | "t") {
            Some((b, a))
        } else {
            Some((a, b))
        };
    }

    let mut x_axis = None;
    let mut y_axis = None;
    for id_node in chart_node.named("axId") {
        let id = id_node.attr("val");
        let Some(&axis) = axis_by_id.get(id) else {
            // 忽略已被过滤的 serAx 引用（仅在 3D 图表中被过滤）
            continue;
        };
        let canonical_id = value(axis, "axId");
        if !used_axis_ids.contains(&canonical_id) {
            used_axis_ids.push(canonical_id);
        }
        match axis.name.as_str() {
            "catAx" | "dateAx" => {
                if x_axis.is_some() {
                    return None;
                }
                x_axis = Some(axis);
            }
            "valAx" => {
                if y_axis.is_some() {
                    return None;
                }
                y_axis = Some(axis);
            }
            _ => return None,
        }
    }
    Some((x_axis?, y_axis?))
}

fn parse_numeric_cache(parent: &Node) -> Option<Vec<Option<f64>>> {
    let raw_values = cache(parent)?;
    let mut values = Vec::with_capacity(raw_values.len());
    for raw in raw_values {
        values.push(match raw {
            None | Some("" | "#N/A") => None,
            Some(raw) => Some(numeric(raw)?),
        });
    }
    Some(values)
}

fn parse_sub_series<'a>(
    chart_node: &'a Node,
    chart_type: ChartType,
    x_axis: Option<&'a Node>,
    total_series: &mut usize,
    total_points: &mut usize,
) -> Option<Vec<Series<'a>>> {
    let mut series = Vec::new();
    let is_scatter = matches!(chart_type, ChartType::Scatter(_));
    for source in chart_node.named("ser") {
        if *total_series + series.len() >= 8 {
            return None;
        }
        let (values, x_values, categories, dates) = if is_scatter {
            let x_vals = parse_numeric_cache(source.child("xVal")?)?;
            let y_vals = parse_numeric_cache(source.child("yVal")?)?;
            if x_vals.len() != y_vals.len()
                || !x_vals
                    .iter()
                    .zip(&y_vals)
                    .any(|(x, y)| x.is_some() && y.is_some())
            {
                return None;
            }
            *total_points += y_vals.len();
            if *total_points > 4096 {
                return None;
            }
            let cats = vec![None; y_vals.len()];
            (y_vals, Some(x_vals), cats, None)
        } else {
            let values = parse_numeric_cache(source.child("val")?)?;
            *total_points += values.len();
            if *total_points > 4096 || !values.iter().any(Option::is_some) {
                return None;
            }
            let categories = match source.child("cat") {
                Some(cat) => cache(cat)?,
                None => (0..values.len()).map(|_| None).collect(),
            };
            if categories.len() != values.len() {
                return None;
            }
            let dates = if x_axis.is_some_and(|n| n.name == "dateAx") {
                Some(
                    categories
                        .iter()
                        .zip(&values)
                        .map(|(raw, y)| match raw {
                            Some(raw) => numeric(raw).map(Some),
                            None if y.is_none() => Some(None),
                            None => None,
                        })
                        .collect::<Option<Vec<_>>>()?,
                )
            } else {
                None
            };
            (values, None, categories, dates)
        };
        series.push(Series {
            node: source,
            values,
            x_values,
            categories,
            dates,
            name: source.child("tx").map(text).unwrap_or_default(),
            color_index: 0,
        });
    }
    if series.is_empty() {
        return None;
    }
    series.sort_by_key(|s| value(s.node, "order").parse::<usize>().unwrap_or(0));
    for item in &mut series {
        item.color_index = *total_series;
        *total_series += 1;
    }
    Some(series)
}

fn collect_y_values(
    chart_type: ChartType,
    series: &[Series<'_>],
    blanks: &str,
) -> Option<Vec<f64>> {
    let mut ys = Vec::new();
    match chart_type {
        ChartType::Line(Grouping::Stacked) => {
            // 堆积折线图按系列顺序累加每个分类的累计值作为数值轴范围。
            let max_len = series.iter().map(|s| s.values.len()).max().unwrap_or(0);
            for i in 0..max_len {
                let mut acc = 0.0;
                let mut has_value = false;
                for s in series {
                    if let Some(v) = s
                        .values
                        .get(i)
                        .copied()
                        .flatten()
                        .or((blanks == "zero").then_some(0.0))
                    {
                        has_value = true;
                        acc += v;
                        if !acc.is_finite() {
                            return None;
                        }
                        ys.push(acc);
                    }
                }
                if has_value && ys.iter().all(|&y| y >= 0.0) {
                    ys.push(0.0);
                }
            }
        }
        ChartType::Line(Grouping::PercentStacked) => {
            // 百分比堆积折线图按分类绝对值总和归一化累计序列。
            let max_len = series.iter().map(|s| s.values.len()).max().unwrap_or(0);
            for i in 0..max_len {
                let total: f64 = series
                    .iter()
                    .filter_map(|s| {
                        s.values
                            .get(i)
                            .copied()
                            .flatten()
                            .or((blanks == "zero").then_some(0.0))
                    })
                    .map(f64::abs)
                    .sum();
                if !total.is_finite() {
                    return None;
                }
                if total > 0.0 {
                    let mut acc = 0.0;
                    ys.push(0.0);
                    for s in series {
                        if let Some(v) = s
                            .values
                            .get(i)
                            .copied()
                            .flatten()
                            .or((blanks == "zero").then_some(0.0))
                        {
                            acc += v / total;
                            ys.push(acc);
                        }
                    }
                } else if series.iter().any(|s| {
                    s.values
                        .get(i)
                        .copied()
                        .flatten()
                        .or((blanks == "zero").then_some(0.0))
                        .is_some()
                }) {
                    ys.push(0.0);
                }
            }
        }
        ChartType::Area(Grouping::Stacked)
        | ChartType::Column(Grouping::Stacked)
        | ChartType::Bar(Grouping::Stacked) => {
            // 堆积面积/柱状/条形图按分类分别累加正负值，以堆积总和作为数值轴范围。
            let max_len = series.iter().map(|s| s.values.len()).max().unwrap_or(0);
            for i in 0..max_len {
                let mut pos = 0.0;
                let mut neg = 0.0;
                let mut has_value = false;
                for s in series {
                    if let Some(v) = s
                        .values
                        .get(i)
                        .copied()
                        .flatten()
                        .or((blanks == "zero").then_some(0.0))
                    {
                        has_value = true;
                        if v >= 0.0 {
                            pos += v;
                        } else {
                            neg += v;
                        }
                    }
                }
                if !pos.is_finite() || !neg.is_finite() {
                    return None;
                }
                if has_value {
                    ys.push(pos);
                    ys.push(neg);
                }
            }
        }
        ChartType::Area(Grouping::PercentStacked)
        | ChartType::Column(Grouping::PercentStacked)
        | ChartType::Bar(Grouping::PercentStacked) => {
            // 百分比堆积图按各分类绝对值总和归一化到 [-1.0, 1.0]。
            let max_len = series.iter().map(|s| s.values.len()).max().unwrap_or(0);
            for i in 0..max_len {
                let mut pos = 0.0;
                let mut neg = 0.0;
                let mut has_value = false;
                for s in series {
                    if let Some(v) = s
                        .values
                        .get(i)
                        .copied()
                        .flatten()
                        .or((blanks == "zero").then_some(0.0))
                    {
                        has_value = true;
                        if v >= 0.0 {
                            pos += v;
                        } else {
                            neg += v;
                        }
                    }
                }
                let total = pos - neg;
                if !total.is_finite() {
                    return None;
                }
                if total > 0.0 {
                    ys.push(pos / total);
                    ys.push(neg / total);
                } else if has_value {
                    ys.push(0.0);
                }
            }
        }
        _ => {
            for s in series {
                for y in &s.values {
                    if let Some(y) = y.or((blanks == "zero").then_some(0.0)) {
                        ys.push(y);
                    }
                }
            }
            if matches!(
                chart_type,
                ChartType::Column(_) | ChartType::Bar(_) | ChartType::Radar(_)
            ) && !ys.is_empty()
                && ys.iter().all(|&y| y >= 0.0)
            {
                ys.push(0.0);
            }
            // 面积区域始终相对零轴闭合；全负值系列也需要把零纳入自动数值范围。
            if matches!(chart_type, ChartType::Area(_)) && !ys.is_empty() {
                ys.push(0.0);
            }
        }
    }
    Some(ys)
}

fn build_chart<'a>(
    node: &'a Node,
    plot: &'a Node,
    root: &'a Node,
    raw_subs: Vec<RawSubChart<'a>>,
    blanks: &'a str,
    is_3d: bool,
) -> Option<Chart<'a>> {
    let combo_has_column = raw_subs.len() > 1
        && raw_subs
            .iter()
            .any(|s| matches!(s.chart_type, ChartType::Column(_)));
    let mut x_pool: HashMap<&'a str, Vec<f64>> = HashMap::new();
    let mut y_pool: HashMap<&'a str, Vec<f64>> = HashMap::new();
    let mut y_order: Vec<&'a str> = Vec::new();
    let mut x_max_len: HashMap<&'a str, usize> = HashMap::new();

    for sub in &raw_subs {
        if let Some(ax) = sub.x_axis {
            let id = value(ax, "axId");
            x_pool.entry(id).or_default().extend_from_slice(&sub.xs);
            let max_cat = sub.series.iter().map(|s| s.values.len()).max().unwrap_or(0);
            x_max_len
                .entry(id)
                .and_modify(|cur| *cur = (*cur).max(max_cat))
                .or_insert(max_cat);
        }
        if let Some(ax) = sub.y_axis {
            let id = value(ax, "axId");
            if !y_pool.contains_key(id) {
                y_order.push(id);
            }
            y_pool.entry(id).or_default().extend_from_slice(&sub.ys);
        }
    }
    if y_order.len() > 2 {
        return None;
    }

    let mut x_ranges: HashMap<&'a str, ((f64, f64), f64)> = HashMap::new();
    let mut y_ranges: HashMap<&'a str, ValueAxis<'a>> = HashMap::new();
    for sub in &raw_subs {
        if let Some(ax) = sub.x_axis {
            let id = value(ax, "axId");
            if !x_ranges.contains_key(id) {
                let is_scatter = matches!(sub.chart_type, ChartType::Scatter(_));
                let (mut xr, x_step) = range(x_pool.get(id)?, ax, is_scatter)?;
                let has_explicit = ax
                    .child("scaling")
                    .is_some_and(|s| s.child("min").is_some() || s.child("max").is_some());
                // 柱线/柱面组合图中无显式边界的非日期分类轴扩展半格，使折线与面积拐点对准柱槽中心
                if combo_has_column && ax.name != "dateAx" && !has_explicit {
                    let count = x_max_len.get(id).copied().unwrap_or(0);
                    if count > 0 {
                        xr = (-0.5, count as f64 - 0.5);
                    }
                }
                x_ranges.insert(id, (xr, x_step));
            }
        }
        if let Some(ax) = sub.y_axis {
            let id = value(ax, "axId");
            if !y_ranges.contains_key(id) {
                let (yr, step) = range(y_pool.get(id)?, ax, true)?;
                y_ranges.insert(
                    id,
                    ValueAxis {
                        axis: ax,
                        range: yr,
                        step,
                    },
                );
            }
        }
    }

    let primary_x_axis = raw_subs.first().and_then(|s| s.x_axis);
    let primary_x_step =
        primary_x_axis.and_then(|ax| x_ranges.get(value(ax, "axId")).map(|(_, step)| *step));
    let primary_y = y_order.first().and_then(|id| y_ranges.get(id).copied());
    let secondary_y = y_order.get(1).and_then(|id| y_ranges.get(id).copied());

    let sub_charts = raw_subs
        .into_iter()
        .map(|sub| {
            let x_range = sub
                .x_axis
                .and_then(|ax| x_ranges.get(value(ax, "axId")).map(|(r, _)| *r))
                .unwrap_or((0.0, 1.0));
            let y_range = sub
                .y_axis
                .and_then(|ax| y_ranges.get(value(ax, "axId")).copied())
                .map_or((0.0, 1.0), |va| va.range);
            SubChart {
                chart_node: sub.chart_node,
                x_axis: sub.x_axis,
                y_axis: sub.y_axis,
                series: sub.series,
                x_range,
                y_range,
                chart_type: sub.chart_type,
            }
        })
        .collect();

    Some(Chart {
        node,
        plot,
        sub_charts,
        primary_x_axis,
        primary_x_step,
        primary_y,
        secondary_y,
        blanks,
        date1904: crate::pptx::flag(value(root, "date1904"), false),
        is_3d,
    })
}

/// 解析环形图 `<c:holeSize val="...">`，合法范围 1..=90（默认 50），
/// 换算为内半径占比并钳位到 `0.1..=0.9`，避免退化或自交的环。
fn hole_ratio(chart_node: &Node) -> f32 {
    let parsed = chart_node
        .child("holeSize")
        .and_then(|n| n.attr("val").parse::<i32>().ok())
        .filter(|value| (1..=90).contains(value))
        .unwrap_or(50);
    (parsed as f32 / 100.0).clamp(0.1, 0.9)
}

impl Series<'_> {
    pub fn x(&self, index: usize) -> Option<f64> {
        if let Some(xs) = &self.x_values {
            return xs.get(index).copied().flatten();
        }
        self.dates
            .as_ref()
            .map_or(Some(index as f64), |dates| dates[index])
    }
}

fn range(values: &[f64], axis: &Node, round: bool) -> Option<((f64, f64), f64)> {
    let mut min = values.iter().copied().reduce(f64::min)?;
    let mut max = values.iter().copied().reduce(f64::max)?;
    if min == max {
        let extra = (min.abs() * 0.1).max(1.0);
        min -= extra;
        max += extra;
    }
    let raw_step = (max - min) / 4.0;
    let power = 10.0_f64.powf(raw_step.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .find(|n| raw_step <= n * power)?
        * power;
    if round {
        min = (min / step).floor() * step;
        max = (max / step).ceil() * step;
    }
    if let Some(scaling) = axis.child("scaling") {
        if let Some(n) = scaling.child("min") {
            min = numeric(n.attr("val"))?;
        }
        if let Some(n) = scaling.child("max") {
            max = numeric(n.attr("val"))?;
        }
    }
    let step = match axis.child("majorUnit") {
        Some(n) if round => numeric(n.attr("val"))?,
        _ => step,
    };
    if max <= min || step <= 0.0 || !step.is_finite() || (max - min) / step > 100.0 {
        return None;
    }
    Some(((min, max), step))
}
