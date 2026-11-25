/**
 * PPTX 表格行高下限与单元格内边距几何检查（纯 TS，Node 可直接执行）。
 *
 * 被测逻辑镜像自 `viewer-ohos/src/main/ets/render/CanvasElementDrawer.ets`：
 * - `cellPadding` 对应同名的 `CanvasElementDrawer.cellPadding`；
 * - `rowHeightAt` 对应 `CanvasElementDrawer.rowHeightAt`；
 * - `resolveCellVerticalOffset` 对应 `CanvasElementDrawer.resolveCellVerticalOffset`；
 * - `computeTableRowHeights` 对应 `CanvasElementDrawer.computeTableGeometry` 的两阶段行高结算。
 *
 * 之所以在此镜像而非直接 import：`CanvasElementDrawer.ets` 依赖 ArkUI 的 `@ohos.*` 与 Canvas
 * 上下文，纯 Node 环境无法加载。镜像版本与源文件共用同一公式，任一侧公式漂移都会使本检查失败：
 * 数值断言覆盖镜像实现，`checkProductionSource` 覆盖生产实现。
 */

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

interface CellRange {
  startRow: number;
  startColumn: number;
  endRow: number;
  endColumn: number;
}

interface TableElement {
  width?: number;
  columnWidths?: number[];
  merges?: CellRange[];
  rowHeights?: number[];
  cellPaddings?: number[][][];
  cellValigns?: number[][];
}

/** 每格内容自然高度，key 为 `${r}:${c}`；缺省视为空单元格 */
type ContentHeights = Map<string, number>;

let assertions = 0;

function expect(condition: boolean, message: string): void {
  assertions++;
  if (!condition) {
    throw new Error(message);
  }
}

function expectEqual(actual: number, expected: number, message: string): void {
  assertions++;
  if (Math.abs(actual - expected) > 1e-6) {
    throw new Error(`${message}: 期望 ${expected}，实际 ${actual}`);
  }
}

/** 镜像 CanvasElementDrawer.cellPadding：非法或缺失一律回退 [5, 5, 5, 5] */
function cellPadding(
  element: TableElement,
  r: number,
  c: number
): [number, number, number, number] {
  const fallback: [number, number, number, number] = [5, 5, 5, 5];
  const paddings = element.cellPaddings;
  if (paddings === undefined) return fallback;
  if (r < 0 || r >= paddings.length) return fallback;
  const row = paddings[r];
  if (row === undefined || c < 0 || c >= row.length) return fallback;
  const pad = row[c];
  if (pad === undefined || pad.length !== 4) return fallback;
  for (let i = 0; i < 4; i++) {
    const value = pad[i];
    if (typeof value !== 'number' || !isFinite(value) || value < 0) return fallback;
  }
  return [pad[0], pad[1], pad[2], pad[3]];
}

/** 镜像 CanvasElementDrawer.rowHeightAt：缺省或非法返回 0，有效值钳位 [0, 14400] */
function rowHeightAt(element: TableElement, r: number): number {
  const heights = element.rowHeights;
  if (heights === undefined || r < 0 || r >= heights.length) return 0;
  const value = heights[r];
  if (typeof value !== 'number' || !isFinite(value) || value <= 0) return 0;
  return Math.min(14400, value);
}

/** 镜像 CanvasElementDrawer.resolveCellVerticalOffset */
function resolveCellVerticalOffset(
  element: TableElement,
  r: number,
  c: number,
  cellH: number,
  cellContentH: number
): number {
  let valign = 1;
  const valigns = element.cellValigns;
  if (valigns !== undefined && r >= 0 && r < valigns.length) {
    const rowValigns = valigns[r];
    if (rowValigns !== undefined && c >= 0 && c < rowValigns.length) {
      valign = rowValigns[c];
    }
  }
  const pad = cellPadding(element, r, c);
  const padTop = pad[0];
  const padBottom = pad[2];
  const remaining = cellH - cellContentH;
  if (remaining <= padTop + padBottom || valign === 0) {
    return padTop;
  }
  if (valign === 2) {
    return Math.max(padTop, remaining - padBottom);
  }
  return Math.max(padTop, padTop + (remaining - padTop - padBottom) / 2);
}

function findMerge(merges: CellRange[] | undefined, r: number, c: number): CellRange | null {
  if (merges === undefined) return null;
  for (const m of merges) {
    if (r >= m.startRow && r <= m.endRow && c >= m.startColumn && c <= m.endColumn) {
      return m;
    }
  }
  return null;
}

/**
 * 镜像 CanvasElementDrawer.computeTableGeometry 的行高两阶段结算（列宽固定假设等分）。
 *
 * 返回各行最终高度与行顶坐标。
 */
function computeTableRowHeights(
  element: TableElement,
  numRows: number,
  cols: number,
  contents: ContentHeights
): { rowHeights: number[]; rowTops: number[] } {
  const tableW = element.width !== undefined && element.width > 0 ? element.width : 500;
  const colW = tableW / Math.max(1, cols);

  // 第一阶段：rowHeights 下限初始化，再按非纵向跨越单元格内容高度撑高
  const rowHeights: number[] = new Array<number>(numRows).fill(20);
  for (let r = 0; r < numRows; r++) {
    const explicitMin = rowHeightAt(element, r);
    rowHeights[r] = Math.max(rowHeights[r], explicitMin);
    for (let c = 0; c < cols; c++) {
      const m = findMerge(element.merges, r, c);
      if (m && (m.startRow !== r || m.startColumn !== c)) continue;
      if (m && m.endRow > m.startRow) continue;
      const x1 = c * colW;
      const x2 = m ? Math.min(cols, m.endColumn + 1) * colW : (c + 1) * colW;
      const cellW = Math.max(1, x2 - x1);
      const contentH = contents.get(`${r}:${c}`);
      if (contentH === undefined) continue;
      const pad = cellPadding(element, r, c);
      rowHeights[r] = Math.max(rowHeights[r], contentH + pad[0] + pad[2]);
    }
  }

  // 第二阶段：纵向合并仅在跨行总高不足时把差额补到末行
  const merges = element.merges;
  if (merges !== undefined) {
    for (const m of merges) {
      if (m.endRow <= m.startRow || m.startRow < 0 || m.endRow >= numRows) continue;
      if (m.startColumn < 0 || m.startColumn >= cols) continue;
      const contentH = contents.get(`${m.startRow}:${m.startColumn}`);
      if (contentH === undefined) continue;
      const x1 = m.startColumn * colW;
      const x2 = Math.min(cols, m.endColumn + 1) * colW;
      const cellW = Math.max(1, x2 - x1);
      const pad = cellPadding(element, m.startRow, m.startColumn);
      const neededH = contentH + pad[0] + pad[2];
      let spanH = 0;
      for (let rr = m.startRow; rr <= m.endRow; rr++) {
        spanH += rowHeights[rr];
      }
      if (neededH > spanH) {
        rowHeights[m.endRow] += neededH - spanH;
      }
    }
  }

  const rowTops: number[] = new Array<number>(numRows + 1).fill(0);
  let y = 0;
  for (let r = 0; r < numRows; r++) {
    rowTops[r] = y;
    y += rowHeights[r];
  }
  rowTops[numRows] = y;
  return { rowHeights, rowTops };
}

/**
 * 防止镜像逻辑与生产实现漂移：直接读取 `CanvasElementDrawer.ets` 源码，断言关键公式片段仍在。
 *
 * 本机无 HarmonyOS SDK，无法编译 .ets，因此改用源码级断言兜底：一旦生产实现被改回写死的
 * `cellW - 10` / `x1 + 5`，或两阶段行高、内边距读取被移除，下方任一断言失败。
 */
function checkProductionSource(): void {
  const sourcePath = resolve(
    import.meta.dirname,
    '../../viewer-ohos/src/main/ets/render/CanvasElementDrawer.ets'
  );
  const source = readFileSync(sourcePath, 'utf8');

  // 写死的 5pt 内边距必须全部改为按格查询
  expect(!source.includes('cellW - 10'), '生产实现不应再写死 cellW - 10');
  expect(!source.includes('x2 - x1 - 10'), '生产实现不应再写死 x2 - x1 - 10');
  expect(!source.includes('x1 + 5'), '生产实现不应再写死 x1 + 5');

  // cellPadding / rowHeightAt 两个查询方法必须存在
  expect(source.includes('public static cellPadding('), '生产实现必须提供 cellPadding');
  expect(source.includes('public static rowHeightAt('), '生产实现必须提供 rowHeightAt');

  // resolveCellVerticalOffset 必须按 padTop / padBottom 取值，且不再有写死的 5 / 10
  const offsetStart = source.indexOf('public static resolveCellVerticalOffset(');
  expect(offsetStart > 0, 'resolveCellVerticalOffset 必须存在');
  const offsetBody = source.slice(offsetStart, source.indexOf('\n  }', offsetStart));
  expect(offsetBody.includes('cellPadding(element, r, c)'), 'resolveCellVerticalOffset 必须查询 cellPadding');
  expect(offsetBody.includes('padTop + padBottom'), '剩余空间判定必须使用 padTop + padBottom');
  expect(offsetBody.includes('remaining - padBottom'), '底部对齐必须使用 padBottom');
  expect(offsetBody.includes('padTop + (remaining - padTop - padBottom) / 2'), '居中公式必须扣除上下内边距');

  // computeTableGeometry 必须是两阶段：先按 rowHeightAt 初始化，再对纵向合并补差额
  const geomStart = source.indexOf('public static computeTableGeometry(');
  expect(geomStart > 0, 'computeTableGeometry 必须存在');
  const geomBody = source.slice(geomStart, source.indexOf('\n  public static measureTableHeight', geomStart));
  expect(geomBody.includes('rowHeightAt(element, r)'), '第一阶段必须读取显式最小行高');
  expect(geomBody.includes('m.endRow > m.startRow'), '第一阶段必须跳过纵向合并单元格');
  expect(geomBody.includes('rowHeights[m.endRow] += neededH - spanH'), '第二阶段差额必须补到 endRow');

  // 三处调用方都必须使用 pad[3] 作为水平原点
  expect(source.includes('x1 + pad[3]'), 'drawTable 文字与图片原点必须使用 padLeft');
  expect(source.includes('cellW - pad[3] - pad[1]'), '可用宽度必须扣除左右内边距');
  expect(source.includes('const cellOriginX = x1 + pad[3];'), '选区块原点必须使用 padLeft');
}

// ---------- 1. cellPadding 缺省与非法输入回退 ----------

expectEqual(cellPadding({}, 0, 0)[0], 5, '缺失 cellPaddings 时 padTop 回退 5');
expectEqual(cellPadding({}, 0, 0)[3], 5, '缺失 cellPaddings 时 padLeft 回退 5');

const defaultTable: TableElement = { cellPaddings: [[[5, 5, 5, 5]]] };
expectEqual(cellPadding(defaultTable, 0, 0)[1], 5, '全默认内边距读取 padRight');

// 越界坐标、非 4 元、负值与非有限值一律回退
const badTable: TableElement = {
  cellPaddings: [
    [[1, 2, 3]],
    [[-1, 2, 3, 4]],
    [[1, Number.NaN, 3, 4]]
  ]
};
expectEqual(cellPadding(badTable, 0, 0)[0], 5, '长度不为 4 时回退默认');
expectEqual(cellPadding(badTable, 1, 0)[0], 5, '含负值时回退默认');
expectEqual(cellPadding(badTable, 2, 0)[0], 5, '含非有限值时回退默认');
expectEqual(cellPadding(badTable, 9, 0)[0], 5, '行越界时回退默认');

// ---------- 2. 自定义非对称内边距下的垂直偏移 ----------

// [top, right, bottom, left] = [0, 12, 8, 16]
const asymmetric: TableElement = { cellPaddings: [[[0, 12, 8, 16]]] };
const padA = cellPadding(asymmetric, 0, 0);
expectEqual(padA[0], 0, '自定义内边距 padTop');
expectEqual(padA[1], 12, '自定义内边距 padRight');
expectEqual(padA[2], 8, '自定义内边距 padBottom');
expectEqual(padA[3], 16, '自定义内边距 padLeft');

// 剩余空间充足（cellH=100, contentH=20, remaining=80 > 0+8）时按 valign 计算
expectEqual(
  resolveCellVerticalOffset(asymmetric, 0, 0, 100, 20),
  36,
  '居中：padTop + (remaining - padTop - padBottom) / 2 = 0 + (80 - 0 - 8) / 2'
);
expectEqual(
  resolveCellVerticalOffset({ ...asymmetric, cellValigns: [[2]] }, 0, 0, 100, 20),
  72,
  '底部：remaining - padBottom = 80 - 8'
);
expectEqual(
  resolveCellVerticalOffset({ ...asymmetric, cellValigns: [[0]] }, 0, 0, 100, 20),
  0,
  '顶部对齐返回 padTop'
);
// 剩余空间不足（remaining=6 <= padTop+padBottom=8）时回退 padTop
expectEqual(
  resolveCellVerticalOffset(asymmetric, 0, 0, 26, 20),
  0,
  '剩余空间不足时回退 padTop'
);

// 兼容性：padTop == padBottom == 5 时与原公式数学等价
const legacy: TableElement = {};
expectEqual(
  resolveCellVerticalOffset(legacy, 0, 0, 100, 20),
  40,
  '默认内边距居中退化为 (cellH - cellContentH) / 2'
);
expectEqual(
  resolveCellVerticalOffset({ cellValigns: [[2]] }, 0, 0, 100, 20),
  75,
  '默认内边距底部退化为 remaining - 5'
);

// ---------- 3. rowHeights 最小行高生效 ----------

const shortText: ContentHeights = new Map([
  ['0:0', 10],
  ['1:0', 10]
]);
const minHeightTable: TableElement = { width: 100, rowHeights: [44, 30] };
const minGeom = computeTableRowHeights(minHeightTable, 2, 1, shortText);
expectEqual(minGeom.rowHeights[0], 44, 'rowHeights[0] = 44 时短文本仍维持 44pt');
expectEqual(minGeom.rowHeights[1], 30, 'rowHeights[1] = 30 时短文本仍维持 30pt');
expectEqual(minGeom.rowTops[2], 74, '累计行高 = 44 + 30');

// 缺省 rowHeights 时保持 20pt 下限
const noMinGeom = computeTableRowHeights({ width: 100 }, 2, 1, shortText);
expectEqual(noMinGeom.rowHeights[0], 20, '缺失 rowHeights 时保持 20pt 下限');

// 内容高于下限时按内容撑高（padTop + padBottom = 5 + 5）
const tallContent: ContentHeights = new Map([['0:0', 50]]);
const tallGeom = computeTableRowHeights({ width: 100, rowHeights: [30] }, 1, 1, tallContent);
expectEqual(tallGeom.rowHeights[0], 60, '内容 50 + 上下内边距 10 撑高至 60');

// rowHeights 负值/非有限值不参与下限（回退 0，仍受 20pt 兜底）
const badHeightsGeom = computeTableRowHeights(
  { width: 100, rowHeights: [-5, Number.NaN] },
  2,
  1,
  shortText
);
expectEqual(badHeightsGeom.rowHeights[0], 20, '负 rowHeights 回退 20pt 下限');
expectEqual(badHeightsGeom.rowHeights[1], 20, '非有限 rowHeights 回退 20pt 下限');

// ---------- 4. rowSpan = 2 且总行高已足够时不拉高首行 ----------

// 两行显式行高合计 100，合并内容只需 40，不应改变任何行高
const spanEnoughContent: ContentHeights = new Map([['0:0', 30]]);
const spanEnoughTable: TableElement = {
  width: 100,
  rowHeights: [50, 50],
  merges: [{ startRow: 0, startColumn: 0, endRow: 1, endColumn: 0 }]
};
const spanEnoughGeom = computeTableRowHeights(spanEnoughTable, 2, 1, spanEnoughContent);
expectEqual(spanEnoughGeom.rowHeights[0], 50, '总行高已足够时首行保持 50pt 不被拉高');
expectEqual(spanEnoughGeom.rowHeights[1], 50, '总行高已足够时末行保持 50pt');
expectEqual(spanEnoughGeom.rowTops[2], 100, '总行高保持 100');

// 两行总高 40 不足以容纳合并内容 100（含上下内边距 10）时，差额补到末行
const spanShortContent: ContentHeights = new Map([['0:0', 100]]);
const spanShortTable: TableElement = {
  width: 100,
  rowHeights: [20, 20],
  merges: [{ startRow: 0, startColumn: 0, endRow: 1, endColumn: 0 }]
};
const spanShortGeom = computeTableRowHeights(spanShortTable, 2, 1, spanShortContent);
expectEqual(spanShortGeom.rowHeights[0], 20, '差额补齐不改变首行高度');
expectEqual(spanShortGeom.rowHeights[1], 90, '差额 110 - 40 = 70 补到末行：20 + 70');
expectEqual(spanShortGeom.rowTops[2], 110, '跨行总高等于合并内容所需高度');

// 自定义上下内边距参与纵向合并所需高度
const spanPadContent: ContentHeights = new Map([['0:0', 50]]);
const spanPadTable: TableElement = {
  width: 100,
  rowHeights: [20, 20],
  cellPaddings: [[[10, 5, 10, 5]]],
  merges: [{ startRow: 0, startColumn: 0, endRow: 1, endColumn: 0 }]
};
const spanPadGeom = computeTableRowHeights(spanPadTable, 2, 1, spanPadContent);
expectEqual(spanPadGeom.rowHeights[1], 50, '所需高 50 + 20 = 70，差额 30 补到末行');

checkProductionSource();

console.log(`PASS PptxTableLayoutChecks (${assertions} assertions)`);
