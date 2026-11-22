/** XLSX 单元格缩进与自动缩字的纯逻辑，与 Android / macOS 语义保持一致。 */
export function cellIndentWidth(
  indent: number | undefined,
  characterWidth: number,
  resolvedAlignment: number
): number {
  if (!indent || indent <= 0 || (resolvedAlignment !== 0 && resolvedAlignment !== 2)) return 0;
  return indent * Math.max(0, characterWidth);
}

export function fittedCellFontSize(
  baseSize: number,
  desiredWidth: number,
  availableWidth: number,
  shrinkToFit: boolean | undefined,
  wrap: boolean
): number {
  if (!shrinkToFit || wrap || desiredWidth <= availableWidth || desiredWidth <= 0) return baseSize;
  const ratio = Math.max(0, availableWidth) / desiredWidth;
  return Math.max(1, Math.min(baseSize, baseSize * ratio));
}
