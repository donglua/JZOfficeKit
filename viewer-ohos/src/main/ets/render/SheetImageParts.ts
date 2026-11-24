/**
 * XLSX 工作表浮动图片的纯几何模块（HarmonyOS）。
 *
 * 本模块只做「元素经 transform 变换后的外接矩形」与「当前帧可见图片部件」的数值计算，
 * 不依赖 ArkUI / Canvas / ImageStore，便于用 node 直接执行
 * scripts/tests/SheetFreezeChecks.ts 做纯逻辑回归。
 *
 * 坐标约定：与 `SheetFreeze` 一致，全部入参与返回值均为**文档坐标**（core 输出的
 * 行高/列宽 point 前缀和，不含行列标头、不含缩放、不含滚动）。屏幕坐标与文档坐标的
 * 换算由调用方（`SpreadsheetView` / `OfficeThumbnail`）用 `resolveQuadrantDocClip` 完成。
 *
 * 为什么单独成文件而不是塞进 `SheetFreeze.ts`：冻结窗格模块负责的是「象限切分」，
 * 本模块负责的是「元素外接框 ∩ 象限」，两者只共享「文档坐标矩形」这一个值形状；
 * 分开后 `SheetFreeze.ts` 的既有契约（以及它的回归用例）不受影响。
 *
 * 本文件刻意不 import 任何模块：仓库内其余纯 TS 模块（`PptxViewport.ts` 等）同样保持零依赖，
 * 因为 node 的 ESM 解析要求相对导入显式带扩展名，而 ArkTS 的 ets-loader 不接受 `.ts` 扩展名。
 * 与 `SheetFreeze` 的类型对接改用结构类型（形状相同即可直接赋值），与
 * `SheetSelectionModel` 对接 `OfficeModel` 的做法一致。
 */

/** 文档坐标矩形（与 `SheetFreeze.FreezeDocRect` 结构一致，可直接互传） */
export interface ElementClipRect {
  readonly left: number;
  readonly top: number;
  readonly right: number;
  readonly bottom: number;
}

/** 象限裁剪区的最小结构形状（`SheetFreeze.FreezeQuadrant` 可直接赋值进来） */
export interface QuadrantClipSource {
  readonly clip: {
    readonly x: number;
    readonly y: number;
    readonly width: number;
    readonly height: number;
  };
  /** 本象限实际使用的水平屏幕偏移（锁定轴恒为 0） */
  readonly offsetX: number;
  /** 本象限实际使用的垂直屏幕偏移（锁定轴恒为 0） */
  readonly offsetY: number;
}

/** 单部件解码像素下限：64×64，避免极小缩略图占用过大的解码预算 */
export const DEFAULT_MIN_IMAGE_PIXELS: number = 64 * 64;

/** 单部件解码像素上限：与 `RenderUtils.DEFAULT_MAX_PIXELS` / PTX 端一致的 4_000_000 */
export const DEFAULT_MAX_IMAGE_PIXELS: number = 4_000_000;

/** TEXT 元素的外扩量：开自动换行时按 16pt、否则按 256pt 外扩，纵向恒为 64pt */
const TEXT_PAD_X_WRAPPED: number = 16;
const TEXT_PAD_X_UNWRAPPED: number = 256;
const TEXT_PAD_Y: number = 64;

/**
 * 参与裁剪判定与图片收集的元素最小结构形状。
 *
 * 用结构类型而非直接引用 `OfficeModel.Element`：`OfficeModel.ets` 是 `.ets` 文件，
 * 纯 TS 模型若依赖它就无法被 node 直接执行做纯逻辑回归；结构类型同时保证
 * `model.Element` 的对象可以直接赋值进来。
 */
export interface SheetElementBox {
  /** 元素类型，仅 `IMAGE` 参与图片部件收集 */
  readonly type: string;
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
  /** 完整绘制变换 [a, b, c, d, tx, ty]；缺省表示按 (x, y) 纯平移 */
  readonly transform?: number[];
  /** ARGB 描边颜色，高 8 位为 0 表示无描边 */
  readonly stroke?: number;
  readonly strokeWidth?: number;
  /** 文本是否自动换行；缺省按 true 处理 */
  readonly textWrap?: boolean;
  /** 图片部件路径（ZIP 包内路径），仅 `IMAGE` 元素有值 */
  readonly image?: string;
}

/** 本帧需要加载的一个图片部件及其解码像素预算 */
export interface VisibleImagePart {
  readonly part: string;
  /** 解码像素预算（宽 × 高的上限），相同部件多次出现时取最大值 */
  desiredPixels: number;
}

/** 可见图片收集入参 */
export interface VisibleImageInput {
  /** 当前工作表的浮动元素（`Sheet.elements`） */
  readonly elements: SheetElementBox[];
  /**
   * 本帧各象限裁剪区反算出的文档坐标可见范围。
   * 由 `resolveFrameDocClips` 从 `FreezeQuadrant` 列表换算，空数组表示本帧无可见区域。
   */
  readonly clips: ElementClipRect[];
  /** 当前缩放，用于把文档尺寸换算成屏幕像素预算 */
  readonly scale: number;
  /** 每 vp 对应的物理像素数，缺省为 1；主预览传入 vp2px(1) */
  readonly pixelRatio?: number;
  /** 单部件解码像素下限，缺省 {@link DEFAULT_MIN_IMAGE_PIXELS} */
  readonly minPixels?: number;
  /** 单部件解码像素上限，缺省 {@link DEFAULT_MAX_IMAGE_PIXELS} */
  readonly maxPixels?: number;
}

/**
 * 计算元素在文档坐标下的外接矩形。
 *
 * 与改造前 `SpreadsheetView.intersectsElementClip` 的局部外框规则完全一致（描边半宽、
 * TEXT 外扩），区别只在于：这里用 `element.transform` 映射四个角点后再取包围盒，
 * 因此旋转 / 翻转后的图片与形状不会被误剔除。
 */
export function resolveElementDocBounds(element: SheetElementBox): ElementClipRect {
  const stroke = element.stroke === undefined ? 0 : element.stroke;
  const strokePad = (stroke >>> 24) === 0 ? 0 : Math.max(0.5, element.strokeWidth || 1) / 2;
  let x0 = -strokePad;
  let y0 = -strokePad;
  let x1 = Math.max(0, element.width) + strokePad;
  let y1 = Math.max(0, element.height) + strokePad;
  if (element.type === 'TEXT') {
    const extraX = element.textWrap === false ? TEXT_PAD_X_UNWRAPPED : TEXT_PAD_X_WRAPPED;
    x0 -= extraX;
    x1 += extraX;
    y0 -= TEXT_PAD_Y;
    y1 += TEXT_PAD_Y;
  }

  const m = element.transform;
  let a = 1;
  let b = 0;
  let c = 0;
  let d = 1;
  let tx = element.x || 0;
  let ty = element.y || 0;
  if (m !== undefined && m.length >= 6) {
    a = m[0];
    b = m[1];
    c = m[2];
    d = m[3];
    tx = m[4];
    ty = m[5];
  }
  const px0 = a * x0 + c * y0 + tx;
  const py0 = b * x0 + d * y0 + ty;
  const px1 = a * x1 + c * y0 + tx;
  const py1 = b * x1 + d * y0 + ty;
  const px2 = a * x0 + c * y1 + tx;
  const py2 = b * x0 + d * y1 + ty;
  const px3 = a * x1 + c * y1 + tx;
  const py3 = b * x1 + d * y1 + ty;
  return {
    left: Math.min(px0, px1, px2, px3),
    top: Math.min(py0, py1, py2, py3),
    right: Math.max(px0, px1, px2, px3),
    bottom: Math.max(py0, py1, py2, py3)
  };
}

/** 判断元素（按 transform 后的外接矩形）是否与文档坐标裁剪区相交；边贴边不算相交 */
export function intersectsElementDocClip(element: SheetElementBox, clip: ElementClipRect): boolean {
  const bounds = resolveElementDocBounds(element);
  return bounds.right > clip.left && bounds.left < clip.right
    && bounds.bottom > clip.top && bounds.top < clip.bottom;
}

/** 计算单张图片在当前缩放与像素密度下的解码预算，并钳位到 [minPixels, maxPixels] */
export function resolveImagePixels(
  element: SheetElementBox,
  scale: number,
  pixelRatio: number = 1,
  minPixels: number = DEFAULT_MIN_IMAGE_PIXELS,
  maxPixels: number = DEFAULT_MAX_IMAGE_PIXELS
): number {
  const s = Number.isFinite(scale) && scale > 0 ? scale : 1;
  const density = Number.isFinite(pixelRatio) && pixelRatio > 0 ? pixelRatio : 1;
  const low = Math.max(1, Math.floor(minPixels));
  const high = Math.max(low, Math.floor(maxPixels));
  // 屏幕投影面积的 4 倍（宽高各留 2 倍余量），为旋转、裁剪与一次缩放升级预留清晰度。
  // 宽、高分别乘像素密度，因此总预算包含 density²，避免高 DPI 设备按 vp 面积解码而模糊。
  const budget = Math.ceil(Math.abs(element.width) * s * density * 2
    * Math.abs(element.height) * s * density * 2);
  if (!Number.isFinite(budget) || budget <= 0) return low;
  return Math.max(low, Math.min(high, budget));
}

/**
 * 把象限裁剪区反算成一组文档坐标可见范围（供 {@link collectVisibleImageParts} 复用）。
 *
 * 公式与 `SheetFreeze.resolveQuadrantDocClip` 完全一致；此处复写一份是为了让本模块
 * 保持零依赖（见文件头说明），两者由 `SheetFreezeChecks` 的一致性断言锁定。
 */
export function resolveFrameDocClips(
  quadrants: QuadrantClipSource[],
  headerW: number,
  headerH: number,
  scale: number
): ElementClipRect[] {
  const s = scale > 0 ? scale : 1;
  const clips: ElementClipRect[] = [];
  for (const quadrant of quadrants) {
    const clipW = quadrant.clip.width;
    const clipH = quadrant.clip.height;
    if (clipW <= 0 || clipH <= 0) continue;
    clips.push({
      left: (quadrant.clip.x - headerW - quadrant.offsetX) / s,
      right: (quadrant.clip.x + clipW - headerW - quadrant.offsetX) / s,
      top: (quadrant.clip.y - headerH - quadrant.offsetY) / s,
      bottom: (quadrant.clip.y + clipH - headerH - quadrant.offsetY) / s
    });
  }
  return clips;
}

/**
 * 收集当前帧需要加载的图片部件。
 *
 * 只处理 `type === 'IMAGE'` 且带 `image` 的元素；同一部件在一帧内多次出现（例如图片
 * 跨冻结边界被相邻象限各画一次，或同一图片被多张锚点引用）按部件路径保序去重，
 * 解码预算取各次出现的最大值，避免高清需求被小图覆盖。
 */
export function collectVisibleImageParts(input: VisibleImageInput): VisibleImagePart[] {
  const parts: VisibleImagePart[] = [];
  const clips = input.clips;
  if (clips.length === 0) return parts;
  const pixelRatio = input.pixelRatio === undefined ? 1 : input.pixelRatio;
  const minPixels = input.minPixels === undefined ? DEFAULT_MIN_IMAGE_PIXELS : input.minPixels;
  const maxPixels = input.maxPixels === undefined ? DEFAULT_MAX_IMAGE_PIXELS : input.maxPixels;
  const indexOf: Map<string, number> = new Map<string, number>();
  for (const element of input.elements) {
    if (element.type !== 'IMAGE') continue;
    const part = element.image;
    if (part === undefined || part.length === 0) continue;
    let visible = false;
    for (const clip of clips) {
      if (intersectsElementDocClip(element, clip)) {
        visible = true;
        break;
      }
    }
    if (!visible) continue;
    const desiredPixels = resolveImagePixels(element, input.scale, pixelRatio, minPixels, maxPixels);
    const existing = indexOf.get(part);
    if (existing === undefined) {
      indexOf.set(part, parts.length);
      parts.push({ part: part, desiredPixels: desiredPixels });
    } else if (desiredPixels > parts[existing].desiredPixels) {
      parts[existing].desiredPixels = desiredPixels;
    }
  }
  return parts;
}
