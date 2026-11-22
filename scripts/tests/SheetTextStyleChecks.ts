import {
  cellIndentWidth,
  fittedCellFontSize
} from '../../viewer-ohos/src/main/ets/render/SheetTextStyle.ts';

function check(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

let assertions = 0;
function expect(condition: boolean, message: string): void {
  assertions++;
  check(condition, message);
}

expect(cellIndentWidth(2, 4, 0) === 8, 'left indent');
expect(cellIndentWidth(2, 4, 2) === 8, 'right indent');
expect(cellIndentWidth(2, 4, 1) === 0, 'center ignores indent');
expect(cellIndentWidth(undefined, 4, 0) === 0, 'missing indent defaults to zero');
expect(cellIndentWidth(2, -1, 0) === 0, 'negative character width is clamped');
expect(fittedCellFontSize(13, 200, 100, true, false) === 6.5, 'shrink to available width');
expect(fittedCellFontSize(13, 50, 100, true, false) === 13, 'never enlarge short text');
expect(fittedCellFontSize(13, 200, 100, true, true) === 13, 'wrap disables shrink-to-fit');
expect(fittedCellFontSize(13, 200, 100, false, false) === 13, 'shrink-to-fit opt-in');
expect(fittedCellFontSize(13, 200, 100, undefined, false) === 13, 'missing shrink flag');
expect(fittedCellFontSize(13, 200, 0.1, true, false) === 1, 'minimum font size');
expect(fittedCellFontSize(13, 0, 100, true, false) === 13, 'empty text keeps font size');

console.log(`Sheet text style checks passed: ${assertions}`);
