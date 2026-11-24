//! 通用日期时间令牌渲染与 ISO 8601 日期单元格解析。
//!
//! 渲染侧消费 [format](super::format) 分词后的 `Token` 流：先把令牌翻译成
//! `DatePart` 序列，再按序列号拆解出的日历与当日时间逐项拼接。`m`/`mm` 的月与分
//! 定性取决于相邻令牌，遵循 OOXML 消歧规则（见 `resolve_minutes`）。
//!
//! 解析侧实现 `t="d"` 单元格的 ISO 8601 → Excel 序列号换算，返回可复用的十进制
//! 序列号字符串与「是否含时间」标志。

use super::format::{Decimal, Token};

/// 一天的秒数，序列号小数部分的换算基准。
const SECONDS_PER_DAY: u64 = 86_400;

/// 一天对应的毫秒数，时间运算的内部单位。
const MILLIS_PER_DAY: u64 = 86_400_000;

/// 日期时间格式允许的最大小数秒位数（`.0` / `.00` / `.000`）。
const MAX_SUB_DIGITS: usize = 3;

/// 1900 体系允许的最大序列号（9999-12-31）。
const MAX_DAY_1900: u32 = 2_958_465;

/// 1904 体系允许的最大序列号（9999-12-31）。
const MAX_DAY_1904: u32 = 2_957_003;

/// ISO 8601 输入长度上限，与 `raw.len() <= 512` 的边界一致。
const MAX_ISO_LEN: usize = 512;

/// 序列号小数部分输出的最大十进制位数。
const MAX_ISO_FRACTION_DIGITS: usize = 12;

/// 英文星期名，索引 0 起为周日..周六。
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// 英文月份名，索引 0 起为一月..十二月。
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

// ---------------------------------------------------------------------------
// 对外接口
// ---------------------------------------------------------------------------

/// 把格式令牌流渲染为日期时间文本。
///
/// 令牌流不含日期时间令牌、或含无法归入日期时间的令牌时返回 `None`，
/// 交由数值渲染路径处理。
pub(super) fn display_tokens(value: &Decimal, tokens: &[Token], date1904: bool) -> Option<String> {
    let parts = parse_parts(tokens)?;
    let has_time = parts.iter().any(is_time_part);
    let precision = sub_second_precision(&parts);
    let max_day = if date1904 { MAX_DAY_1904 } else { MAX_DAY_1900 };
    let (day, millis) = clock_fields(value, precision, has_time)?;
    if day > max_day {
        return None;
    }
    render_parts(&parts, day, millis, date1904)
}

// ---------------------------------------------------------------------------
// 令牌 → DatePart
// ---------------------------------------------------------------------------

/// 日期时间格式中的一个渲染单元。
///
/// 与 `Token` 基本一一对应；`m`/`mm` 在解析阶段已定性地为月或分。
enum DatePart {
    /// 年：`four` 为真时输出 4 位，否则输出后两位。
    Year { four: bool },
    /// 月：`width` 1..=2 输出数字，3 为缩写名，4 为全称，5 为首字母。
    Month { width: usize },
    /// 日：`width` 1..=2 输出日数字，3 为星期缩写，4 为星期全称。
    Day { width: usize },
    /// 小时：`padded` 为真时补零到两位。
    Hour { padded: bool },
    /// 分钟：`padded` 为真时补零到两位。
    Minute { padded: bool },
    /// 秒：`padded` 为真时补零到两位，`sub_digits` 为小数秒位数 0..=3。
    Second { padded: bool, sub_digits: usize },
    /// 累计小时 `[h]` / `[hh]`，不按 24 取模。
    ElapsedHours { padded: bool },
    /// 累计分钟 `[m]` / `[mm]`，不按 60 取模。
    ElapsedMinutes { padded: bool },
    /// 累计秒 `[s]` / `[ss]`，不按 60 取模。
    ElapsedSeconds { padded: bool },
    /// 上下午：`short` 为 `A/P` 形态，`upper` 保留原始大小写。
    Meridiem { short: bool, upper: bool },
    /// 原样输出的字面量。
    Literal(String),
}

/// 将令牌流翻译为 `DatePart` 序列，并完成 `m`/`mm` 的分月消歧。
///
/// 令牌流里没有任何日期时间令牌时返回 `None`，纯字面量格式由调用方自行处理。
fn parse_parts(tokens: &[Token]) -> Option<Vec<DatePart>> {
    let mut parts = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens.get(index)?;
        match token {
            Token::Literal(text) => push_text(&mut parts, text),
            // 会计填充符在纯文本渲染下折叠为单个空格，相邻已有空格时不重复。
            Token::FillSpace => push_fill_space(&mut parts),
            Token::DateToken(raw) => {
                let consumed = push_date_token(&mut parts, raw, tokens, index)?;
                index += consumed;
            }
            Token::AmPm(raw) => parts.push(parse_meridiem(raw)?),
            Token::BracketElapsed(inner) => parts.push(parse_elapsed(inner)?),
            // 日期格式里的 `,` 与 `.` 是字面分隔符（如 `dddd, mmmm d, yyyy`）：
            // 分词器按数字类令牌切出它们，此处置回文本。小数秒的 `.0` 已在
            // `push_date_token` 中随 `s`/`ss` 一并消费，不会走到这里。
            Token::Comma => push_text(&mut parts, ","),
            Token::Dot => push_text(&mut parts, "."),
            // 其余数字类令牌与文本占位符不属于日期时间格式。
            _ => return None,
        }
        index += 1;
    }
    if !parts.iter().any(is_temporal) {
        return None;
    }
    resolve_minutes(&mut parts);
    Some(parts)
}

/// 追加字面量；与已有尾部字面量合并，避免碎片化。
fn push_text(parts: &mut Vec<DatePart>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(DatePart::Literal(last)) = parts.last_mut() {
        last.push_str(text);
        return;
    }
    parts.push(DatePart::Literal(text.to_owned()));
}

/// 追加会计填充空格；上一个字面量已以空格结尾时折叠。
fn push_fill_space(parts: &mut Vec<DatePart>) {
    let ended_with_space =
        matches!(parts.last(), Some(DatePart::Literal(last)) if last.ends_with(' '));
    if !ended_with_space {
        push_text(parts, " ");
    }
}

/// 解析日期字母串（或聚合形态的小数秒），返回额外消费的令牌数。
fn push_date_token(
    parts: &mut Vec<DatePart>,
    raw: &str,
    tokens: &[Token],
    index: usize,
) -> Option<usize> {
    // 分词器若把小数秒聚合成 `.0` / `.00` / `.000`，补到前一个秒部件上。
    if let Some(digits) = raw.strip_prefix('.') {
        let width = sub_digits(digits)?;
        match parts.last_mut() {
            Some(DatePart::Second { sub_digits, .. }) => *sub_digits = width,
            _ => return None,
        }
        return Some(0);
    }
    let letter = raw.chars().next()?.to_ascii_lowercase();
    let count = raw.chars().count();
    // `s`/`ss` 需要额外识别紧随其后的 `.0` / `.00` / `.000` 并消费这些令牌。
    let (part, consumed) = match letter {
        'y' => (DatePart::Year { four: count != 2 }, 0),
        'm' => (
            DatePart::Month {
                width: count.clamp(1, 5),
            },
            0,
        ),
        'd' => (
            DatePart::Day {
                width: count.clamp(1, 4),
            },
            0,
        ),
        'h' => (DatePart::Hour { padded: count >= 2 }, 0),
        's' => {
            let (sub_digits, consumed) = trailing_sub_seconds(tokens, index)?;
            (
                DatePart::Second {
                    padded: count >= 2,
                    sub_digits,
                },
                consumed,
            )
        }
        _ => return None,
    };
    parts.push(part);
    Some(consumed)
}

/// 识别紧邻 `s` / `ss` 的小数秒：`.` 后跟 1..=3 个 `0`，返回精度与消费的令牌数。
fn trailing_sub_seconds(tokens: &[Token], index: usize) -> Option<(usize, usize)> {
    if !matches!(tokens.get(index + 1), Some(Token::Dot)) {
        return Some((0, 0));
    }
    let mut width = 0;
    while matches!(tokens.get(index + 2 + width), Some(Token::DigitZero)) {
        width += 1;
    }
    if width == 0 || width > MAX_SUB_DIGITS {
        return Some((0, 0));
    }
    Some((width, 1 + width))
}

/// 解析 `.0` / `.00` / `.000` 形态的小数秒位数。
fn sub_digits(digits: &str) -> Option<usize> {
    if digits.is_empty()
        || digits.len() > MAX_SUB_DIGITS
        || !digits.bytes().all(|byte| byte == b'0')
    {
        return None;
    }
    Some(digits.len())
}

/// 解析 `AM/PM` / `am/pm` / `A/P` / `a/p`，保留原始大小写与长短形态。
fn parse_meridiem(raw: &str) -> Option<DatePart> {
    let valid = match raw.len() {
        5 => raw.eq_ignore_ascii_case("am/pm"),
        3 => raw.eq_ignore_ascii_case("a/p"),
        _ => false,
    };
    if !valid {
        return None;
    }
    Some(DatePart::Meridiem {
        short: raw.len() == 3,
        upper: raw.starts_with(|c: char| c.is_ascii_uppercase()),
    })
}

/// 解析 `[h]` / `[hh]` / `[m]` / `[mm]` / `[s]` / `[ss]` 累计时间。
fn parse_elapsed(inner: &str) -> Option<DatePart> {
    let padded = inner.len() >= 2;
    Some(match inner.to_ascii_lowercase().as_str() {
        "h" | "hh" => DatePart::ElapsedHours { padded },
        "m" | "mm" => DatePart::ElapsedMinutes { padded },
        "s" | "ss" => DatePart::ElapsedSeconds { padded },
        _ => return None,
    })
}

/// 按 OOXML 规则定性 `m` / `mm`：之前最近的日期时间令牌是小时，或之后最近的是秒，
/// 即为分钟；否则为月份。`mmm` 及以上恒为月份名，不参与消歧。
fn resolve_minutes(parts: &mut [DatePart]) {
    for index in 0..parts.len() {
        let width = match parts.get(index) {
            Some(DatePart::Month { width }) if *width == 1 || *width == 2 => *width,
            _ => continue,
        };
        let preceded_by_hour = parts
            .get(..index)
            .and_then(|head| head.iter().rev().find(|part| is_temporal(part)))
            .is_some_and(is_hour_part);
        let followed_by_second = parts
            .get(index + 1..)
            .and_then(|tail| tail.iter().find(|part| is_temporal(part)))
            .is_some_and(is_second_part);
        if !(preceded_by_hour || followed_by_second) {
            continue;
        }
        if let Some(slot) = parts.get_mut(index) {
            *slot = DatePart::Minute { padded: width >= 2 };
        }
    }
}

/// 该部件是否为日期时间令牌（字面量除外，消歧时跳过字面量）。
fn is_temporal(part: &DatePart) -> bool {
    !matches!(part, DatePart::Literal(_))
}

/// 该部件是否为小时类令牌。
fn is_hour_part(part: &DatePart) -> bool {
    matches!(part, DatePart::Hour { .. } | DatePart::ElapsedHours { .. })
}

/// 该部件是否为秒类令牌。
fn is_second_part(part: &DatePart) -> bool {
    matches!(
        part,
        DatePart::Second { .. } | DatePart::ElapsedSeconds { .. }
    )
}

/// 该部件是否参与日历日期渲染。
fn is_date_part(part: &DatePart) -> bool {
    matches!(
        part,
        DatePart::Year { .. } | DatePart::Month { .. } | DatePart::Day { .. }
    )
}

/// 该部件是否参与当日时间渲染。
fn is_time_part(part: &DatePart) -> bool {
    matches!(
        part,
        DatePart::Hour { .. }
            | DatePart::Minute { .. }
            | DatePart::Second { .. }
            | DatePart::ElapsedHours { .. }
            | DatePart::ElapsedMinutes { .. }
            | DatePart::ElapsedSeconds { .. }
            | DatePart::Meridiem { .. }
    )
}

/// 格式中出现的最大小数秒精度（0..=3）。
fn sub_second_precision(parts: &[DatePart]) -> usize {
    parts
        .iter()
        .filter_map(|part| match part {
            DatePart::Second { sub_digits, .. } => Some(*sub_digits),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .min(MAX_SUB_DIGITS)
}

// ---------------------------------------------------------------------------
// 序列号 → 日历与当日时间
// ---------------------------------------------------------------------------

/// 按格式所需精度换算序列号，返回整数天与当日毫秒数。
///
/// 含时间部分时按 `1 / (86_400 * 10^precision)` 刻度四舍五入并自动进位；
/// 纯日期格式沿用整数天截断，避免小数日被进位到下一天。
fn clock_fields(value: &Decimal, precision: usize, has_time: bool) -> Option<(u32, u64)> {
    if !has_time {
        return Some((u32::try_from(value.units(1, false)?).ok()?, 0));
    }
    let scale = 10_u32.checked_pow(u32::try_from(precision).ok()?)?;
    let per_day = SECONDS_PER_DAY.checked_mul(u64::from(scale))?;
    let total = value.units(u32::try_from(per_day).ok()?, true)?;
    let day = u32::try_from(total / per_day).ok()?;
    let tick = total % per_day;
    // 统一折算到毫秒，供各时间部件按需取位。
    let to_millis = 10_u64.checked_pow(u32::try_from(MAX_SUB_DIGITS - precision).ok()?)?;
    Some((day, tick.checked_mul(to_millis)?))
}

/// 日历日期：1900 体系的 0 值以月 1、日 0 表示。
#[derive(Clone, Copy)]
struct Calendar {
    year: u32,
    month: u32,
    day: u32,
}

const fn leap(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

const fn days_before_year(year: u32) -> u32 {
    let preceding = year - 1;
    365 * preceding + preceding / 4 - preceding / 100 + preceding / 400
}

/// 把序列号还原为日历日期，保留 1900 体系假闰年 60 = 1900-02-29。
fn calendar(serial: u32, date1904: bool) -> Option<Calendar> {
    if !date1904 && serial == 60 {
        return Some(Calendar {
            year: 1900,
            month: 2,
            day: 29,
        });
    }
    let epoch = if date1904 { 1904 } else { 1900 };
    let ordinal = if date1904 {
        serial
    } else {
        serial.checked_sub(1 + u32::from(serial > 60))?
    };
    let mut year = epoch + ordinal / 366;
    let mut remaining = ordinal - (days_before_year(year) - days_before_year(epoch));
    while remaining >= 365 + u32::from(leap(year)) {
        remaining -= 365 + u32::from(leap(year));
        year += 1;
    }
    let lengths = [
        31,
        28 + u32::from(leap(year)),
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1;
    for days in lengths {
        if remaining < days {
            return Some(Calendar {
                year,
                month,
                day: remaining + 1,
            });
        }
        remaining -= days;
        month += 1;
    }
    None
}

/// 星期索引（0 = 周日）。1900 体系序列 1 记为周日，1904 体系序列 0 为周五。
fn weekday(serial: u32, date1904: bool) -> u32 {
    if date1904 {
        (serial + 5) % 7
    } else if serial == 0 {
        6
    } else {
        (serial + 6) % 7
    }
}

// ---------------------------------------------------------------------------
// DatePart → 文本
// ---------------------------------------------------------------------------

/// 渲染上下文：日历、星期、当日毫秒数与格式级开关。
struct Render<'a> {
    parts: &'a [DatePart],
    calendar: Calendar,
    weekday: u32,
    millis: u64,
    /// 序列号整数天，累计时间需要它参与。
    day: u32,
    /// 格式含上下午令牌时小时按 12 小时制输出。
    twelve_hour: bool,
}

/// 渲染 `DatePart` 序列。
fn render_parts(parts: &[DatePart], day: u32, millis: u64, date1904: bool) -> Option<String> {
    let has_date = parts.iter().any(is_date_part);
    let has_time = parts.iter().any(is_time_part);
    let calendar = if day == 0 && !date1904 {
        // 纯时间格式允许序列号 0；仅日期格式的 0 不是合法日历日。
        if has_date && !has_time {
            return None;
        }
        Calendar {
            year: 1900,
            month: 1,
            day: 0,
        }
    } else {
        calendar(day, date1904)?
    };
    let context = Render {
        parts,
        calendar,
        weekday: weekday(day, date1904),
        millis,
        day,
        twelve_hour: parts
            .iter()
            .any(|part| matches!(part, DatePart::Meridiem { .. })),
    };
    let mut result = String::new();
    for part in context.parts {
        render_part(&mut result, part, &context)?;
    }
    Some(result)
}

/// 渲染单个部件。
fn render_part(result: &mut String, part: &DatePart, context: &Render<'_>) -> Option<()> {
    let millis = context.millis;
    match part {
        DatePart::Literal(text) => result.push_str(text),
        DatePart::Year { four } => {
            let year = context.calendar.year;
            if *four {
                result.push_str(&format!("{year:04}"));
            } else {
                result.push_str(&format!("{:02}", year % 100));
            }
        }
        DatePart::Month { width } => render_month(result, context.calendar.month, *width)?,
        DatePart::Day { width } => {
            render_day(result, context.calendar.day, context.weekday, *width)?
        }
        DatePart::Hour { padded } => {
            let hour = millis / 3_600_000 % 24;
            let shown = if context.twelve_hour {
                (hour + 11) % 12 + 1
            } else {
                hour
            };
            push_number(result, shown, usize::from(*padded) + 1);
        }
        DatePart::Minute { padded } => {
            push_number(result, millis / 60_000 % 60, usize::from(*padded) + 1);
        }
        DatePart::Second { padded, sub_digits } => {
            push_number(result, millis / 1_000 % 60, usize::from(*padded) + 1);
            if *sub_digits > 0 {
                let fraction = format!("{:03}", millis % 1_000);
                result.push('.');
                result.push_str(fraction.get(..*sub_digits)?);
            }
        }
        DatePart::ElapsedHours { padded } => {
            let hours = u64::from(context.day) * 24 + millis / 3_600_000;
            push_number(result, hours, usize::from(*padded) + 1);
        }
        DatePart::ElapsedMinutes { padded } => {
            let minutes = u64::from(context.day) * 1_440 + millis / 60_000;
            push_number(result, minutes, usize::from(*padded) + 1);
        }
        DatePart::ElapsedSeconds { padded } => {
            let seconds = u64::from(context.day) * SECONDS_PER_DAY + millis / 1_000;
            push_number(result, seconds, usize::from(*padded) + 1);
        }
        DatePart::Meridiem { short, upper } => {
            let pm = millis / 3_600_000 % 24 >= 12;
            let text = match (*short, pm) {
                (true, false) => "A",
                (true, true) => "P",
                (false, false) => "AM",
                (false, true) => "PM",
            };
            if *upper {
                result.push_str(text);
            } else {
                result.push_str(&text.to_ascii_lowercase());
            }
        }
    }
    Some(())
}

/// 追加十进制数字，按最小位数补零。
fn push_number(result: &mut String, value: u64, min_width: usize) {
    let text = value.to_string();
    for _ in text.len()..min_width {
        result.push('0');
    }
    result.push_str(&text);
}

/// 渲染月份：数字、缩写名、全称或首字母。
fn render_month(result: &mut String, month: u32, width: usize) -> Option<()> {
    let name = MONTHS.get(usize::try_from(month.checked_sub(1)?).ok()?)?;
    match width {
        1 => result.push_str(&month.to_string()),
        2 => push_number(result, u64::from(month), 2),
        3 => result.push_str(name.get(..3)?),
        4 => result.push_str(name),
        _ => result.push_str(name.get(..1)?),
    }
    Some(())
}

/// 渲染日数字或星期名。
fn render_day(result: &mut String, day: u32, weekday: u32, width: usize) -> Option<()> {
    match width {
        1 => result.push_str(&day.to_string()),
        2 => push_number(result, u64::from(day), 2),
        _ => {
            let name = WEEKDAYS.get(usize::try_from(weekday).ok()?)?;
            result.push_str(if width == 3 { name.get(..3)? } else { name });
        }
    }
    Some(())
}

// ---------------------------------------------------------------------------
// ISO 8601 → 序列号
// ---------------------------------------------------------------------------

/// ISO 8601 文本的解析结果。
struct IsoDateTime {
    year: u32,
    month: u32,
    day: u32,
    /// 当日时刻折算的秒数，已扣除时区偏移，可为负。
    seconds: i64,
    /// 小数秒折算的毫秒数（0..=999）。
    millis: i64,
    /// 是否含时间部分。
    has_time: bool,
}

/// 把 `t="d"` 单元格的 ISO 8601 文本换算为 Excel 序列号十进制字符串。
///
/// 返回值第二项表示是否含时间部分。`Z` 直接取墙上时间、不做本地时区换算；
/// `±HH:MM` 偏移按分钟调整当日时刻，跨日时相应调整整数天。非法或越界输入返回
/// `None`，由调用方保留原值。
pub(super) fn iso_to_serial(raw: &str, date1904: bool) -> Option<(String, bool)> {
    let text = raw.trim();
    if text.is_empty() || text.len() > MAX_ISO_LEN {
        return None;
    }
    let parsed = parse_iso(text)?;
    let min_year = if date1904 { 1904 } else { 1900 };
    if parsed.year < min_year || parsed.year > 9999 {
        return None;
    }
    let max_day = if date1904 { MAX_DAY_1904 } else { MAX_DAY_1900 };
    // 偏移可能使当日时刻跨日，先把当日毫秒归一化到 [0, 1) 天。
    let mut sub_day = parsed
        .seconds
        .checked_mul(1_000)?
        .checked_add(parsed.millis)?;
    let mut adjust = sub_day.div_euclid(i64::try_from(MILLIS_PER_DAY).ok()?);
    sub_day -= adjust.checked_mul(i64::try_from(MILLIS_PER_DAY).ok()?)?;
    // 归一化后余数必定落在 [0, 1) 天内；此处再兜底一次以防御极端输入。
    if sub_day < 0 {
        adjust = adjust.checked_sub(1)?;
        sub_day += i64::try_from(MILLIS_PER_DAY).ok()?;
    }
    // 先在真实天数上跨日，再统一映射为序列号，避免在假闰年边界上做特殊判断。
    let ordinal = ordinal_days(&parsed, date1904)?;
    let shifted = i64::from(ordinal).checked_add(adjust)?;
    let day = i64::from(serial_from_ordinal(u32::try_from(shifted).ok()?, date1904)?);
    if day > i64::from(max_day) {
        return None;
    }
    if !parsed.has_time {
        return Some((day.to_string(), false));
    }
    let fraction = fraction_digits(u64::try_from(sub_day).ok()?);
    let serial = if fraction.is_empty() {
        day.to_string()
    } else {
        format!("{day}.{fraction}")
    };
    Some((serial, true))
}

/// 当日毫秒数换算为 12 位以内、去掉尾零的小数串。
///
/// 用整数长除逐步取位：12:00 这类有限小数自然终止为 `5`，
/// 14:30 这类循环小数按 12 位截断。
fn fraction_digits(millis: u64) -> String {
    if millis == 0 {
        return String::new();
    }
    let mut digits = String::with_capacity(MAX_ISO_FRACTION_DIGITS);
    let mut remainder = millis;
    for _ in 0..MAX_ISO_FRACTION_DIGITS {
        remainder *= 10;
        let digit = remainder / MILLIS_PER_DAY;
        remainder %= MILLIS_PER_DAY;
        digits.push(char::from(u8::try_from(digit.min(9)).unwrap_or(0) + b'0'));
        if remainder == 0 {
            break;
        }
    }
    while digits.ends_with('0') {
        digits.pop();
    }
    digits
}

/// 解析 ISO 8601 文本为日期时间字段。
fn parse_iso(text: &str) -> Option<IsoDateTime> {
    let (date, time) = match text.find(['T', 't', ' ']) {
        Some(index) => (&text[..index], Some(text.get(index + 1..)?.trim())),
        None => (text, None),
    };
    let mut fields = date.split('-');
    let year = parse_number(fields.next()?, 4)?;
    let month = parse_number(fields.next()?, 2)?;
    let day = parse_number(fields.next()?, 2)?;
    if fields.next().is_some() {
        return None;
    }
    let year = u32::try_from(year).ok()?;
    let month = u32::try_from(month).ok()?;
    let day = u32::try_from(day).ok()?;
    if day < 1 || day > month_length(year, month)? {
        return None;
    }
    let mut result = IsoDateTime {
        year,
        month,
        day,
        seconds: 0,
        millis: 0,
        has_time: false,
    };
    let time = match time {
        Some(time) if !time.is_empty() => time,
        Some(_) => return None,
        None => return Some(result),
    };
    let (clock, offset_minutes) = split_offset(time)?;
    let (hour, minute, second, millis) = parse_clock(clock)?;
    result.seconds = hour
        .checked_mul(3_600)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)?
        .checked_sub(offset_minutes.checked_mul(60)?)?;
    result.millis = millis;
    result.has_time = true;
    Some(result)
}

/// 拆分时区后缀，返回墙上时间与偏移分钟数（本地时间 = UTC + 偏移）。
fn split_offset(time: &str) -> Option<(&str, i64)> {
    if let Some(clock) = time.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    let bytes = time.as_bytes();
    let mut index = bytes.len();
    while index > 0 {
        index -= 1;
        let sign = match bytes.get(index) {
            Some(b'+') => 1,
            Some(b'-') => -1,
            _ => continue,
        };
        let clock = time.get(..index)?;
        let offset = parse_offset(time.get(index + 1..)?)?;
        return Some((clock, sign * offset));
    }
    Some((time, 0))
}

/// 解析 `HH:MM` 形式的时区偏移为分钟数。
fn parse_offset(text: &str) -> Option<i64> {
    let mut fields = text.split(':');
    let hour = parse_number(fields.next()?, 2)?;
    let minute = match fields.next() {
        Some(minute) => parse_number(minute, 2)?,
        None => 0,
    };
    if fields.next().is_some() || hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

/// 解析 `HH:MM[:SS[.fff]]` 形式的当日时刻。
fn parse_clock(clock: &str) -> Option<(i64, i64, i64, i64)> {
    let mut fields = clock.split(':');
    let hour = parse_number(fields.next()?, 2)?;
    let minute = parse_number(fields.next()?, 2)?;
    let (second, millis) = match fields.next() {
        Some(rest) => {
            let (whole, fraction) = rest.split_once('.').unwrap_or((rest, ""));
            (parse_number(whole, 2)?, parse_millis(fraction)?)
        }
        None => (0, 0),
    };
    if fields.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some((hour, minute, second, millis))
}

/// 解析小数秒为毫秒；超过 3 位或含非数字时返回 `None`。
fn parse_millis(fraction: &str) -> Option<i64> {
    if fraction.is_empty() {
        return Some(0);
    }
    if fraction.len() > MAX_SUB_DIGITS || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut millis = 0_i64;
    for byte in fraction.bytes() {
        millis = millis * 10 + i64::from(byte - b'0');
    }
    for _ in fraction.len()..MAX_SUB_DIGITS {
        millis *= 10;
    }
    Some(millis)
}

/// 解析定长纯数字字段，长度上限为 `width`。
fn parse_number(text: &str, width: usize) -> Option<i64> {
    if text.is_empty() || text.len() > width || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>().ok()
}

/// 当月天数，含闰年判断。
fn month_length(year: u32, month: u32) -> Option<u32> {
    Some(match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 28 + u32::from(leap(year)),
        _ => return None,
    })
}

/// 计算相对工作簿基准日、以 0 起算的真实天数偏移。
///
/// 返回值仍是真实格里高利历的天数差；与假闰年 60 的对齐交给
/// `serial_from_ordinal` 统一处理，因此跨日调整不必再判断边界。
fn ordinal_days(value: &IsoDateTime, date1904: bool) -> Option<u32> {
    let epoch = if date1904 { 1904 } else { 1900 };
    let year_days = days_before_year(value.year).checked_sub(days_before_year(epoch))?;
    let month_days = cumulative_month_days(value.year, value.month)?;
    (year_days + month_days).checked_add(value.day.checked_sub(1)?)
}

/// 把真实天数偏移换算为序列号：1900 体系在 1900-03-01 起跳过假闰年 60。
fn serial_from_ordinal(ordinal: u32, date1904: bool) -> Option<u32> {
    if date1904 {
        return Some(ordinal);
    }
    // 1900-01-01..=1900-02-28 对应序列 1..=59，之后每天再偏移 1。
    if ordinal <= 58 {
        ordinal.checked_add(1)
    } else {
        ordinal.checked_add(2)
    }
}

/// 当月之前（不含当月）的累计天数。
fn cumulative_month_days(year: u32, month: u32) -> Option<u32> {
    let mut total = 0_u32;
    for current in 1..month {
        total = total.checked_add(month_length(year, current)?)?;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xlsx::format::tokenize_section;

    /// 端到端渲染：真实分词 → `display_tokens`，与 format.rs 的调用路径一致。
    fn display(raw: &str, code: &str, date1904: bool) -> Option<String> {
        let value = Decimal::parse(raw)?;
        let tokens = tokenize_section(code)?;
        display_tokens(&value, &tokens, date1904)
    }

    #[test]
    fn chinese_literal_date() {
        assert_eq!(
            display("45292", "yyyy\"年\"m\"月\"d\"日\"", false).as_deref(),
            Some("2024年1月1日")
        );
    }

    #[test]
    fn slash_date_without_padding() {
        assert_eq!(
            display("45292", "yyyy/m/d", false).as_deref(),
            Some("2024/1/1")
        );
        assert_eq!(
            display("45292", "mm/dd/yy", false).as_deref(),
            Some("01/01/24")
        );
    }

    #[test]
    fn weekday_and_full_month_name() {
        assert_eq!(
            display("45292", "dddd, mmmm d, yyyy", false).as_deref(),
            Some("Monday, January 1, 2024")
        );
    }

    #[test]
    fn short_weekday_and_initial_month() {
        assert_eq!(
            display("45292", "ddd mmmmm-yy", false).as_deref(),
            Some("Mon J-24")
        );
        assert_eq!(display("45292", "mmm d", false).as_deref(), Some("Jan 1"));
    }

    #[test]
    fn minute_second_with_one_sub_second_digit() {
        // 0.0011574074 天 ≈ 100 秒，即 01:40.0。
        assert_eq!(
            display("0.0011574074", "mm:ss.0", false).as_deref(),
            Some("01:40.0")
        );
        // 三档小数秒精度分别取到十分位、百分位与毫秒。
        assert_eq!(
            display("0.0011574074", "ss.00", false).as_deref(),
            Some("40.00")
        );
    }

    #[test]
    fn meridiem_markers_keep_case_and_width() {
        assert_eq!(
            display("0.75", "h:mm A/P", false).as_deref(),
            Some("6:00 P")
        );
        assert_eq!(
            display("0.25", "h:mm A/P", false).as_deref(),
            Some("6:00 A")
        );
        assert_eq!(
            display("0.75", "h:mm am/pm", false).as_deref(),
            Some("6:00 pm")
        );
        assert_eq!(
            display("0.5", "h:mm AM/PM", false).as_deref(),
            Some("12:00 PM")
        );
        // 午夜按 12 小时制显示为 12 AM。
        assert_eq!(
            display("0", "h:mm AM/PM", false).as_deref(),
            Some("12:00 AM")
        );
    }

    #[test]
    fn minute_before_second_stays_minute() {
        assert_eq!(display("0.0625", "m:ss", false).as_deref(), Some("30:00"));
        // 单字母 `m` 作为分钟时不得强制补零，仅 `mm` 补零到两位。
        assert_eq!(display("0.003472222", "h:m", false).as_deref(), Some("0:5"));
        assert_eq!(
            display("0.003472222", "h:mm", false).as_deref(),
            Some("0:05")
        );
        // 无小时且无秒相邻时仍是月份。
        assert_eq!(display("45292", "m/d", false).as_deref(), Some("1/1"));
    }

    #[test]
    fn mixed_case_date_tokens_merge() {
        assert_eq!(
            display("45292", "Yyyy-Mm-Dd", false).as_deref(),
            Some("2024-01-01")
        );
    }

    #[test]
    fn elapsed_units_are_cumulative() {
        assert_eq!(
            display("2.5", "[h]:mm:ss", false).as_deref(),
            Some("60:00:00")
        );
        // 0.0625 天 = 5 400 秒 = 90 累计分钟。
        assert_eq!(display("0.0625", "[m]:ss", false).as_deref(), Some("90:00"));
        assert_eq!(display("0.0625", "[s]", false).as_deref(), Some("5400"));
    }

    #[test]
    fn fake_leap_day_and_epochs_render() {
        for (raw, expected) in [
            ("59", "1900-02-28"),
            ("60", "1900-02-29"),
            ("61", "1900-03-01"),
            ("1", "1900-01-01"),
        ] {
            assert_eq!(
                display(raw, "yyyy-mm-dd", false).as_deref(),
                Some(expected),
                "serial {raw}"
            );
        }
        assert_eq!(
            display("0", "yyyy-mm-dd", true).as_deref(),
            Some("1904-01-01")
        );
        // 纯日期格式的序列号 0 不是合法日历日。
        assert_eq!(display("0", "yyyy-mm-dd", false), None);
    }

    #[test]
    fn weekday_follows_serial_epoch() {
        // 1900 体系序列 1 记为周日；1904 体系序列 0 为周五。
        assert_eq!(weekday(1, false), 0);
        assert_eq!(weekday(0, false), 6);
        assert_eq!(weekday(61, false), 4);
        assert_eq!(weekday(0, true), 5);
        assert_eq!(weekday(45292, false), 1);
        assert_eq!(display("1", "dddd", false).as_deref(), Some("Sunday"));
        assert_eq!(display("61", "dddd", false).as_deref(), Some("Thursday"));
    }

    #[test]
    fn pure_time_and_mixed_formats() {
        // 纯时间允许序列号 0；混合格式在 1900 体系下按 1 月 0 日渲染。
        assert_eq!(display("0.5", "h:mm", false).as_deref(), Some("12:00"));
        assert_eq!(
            display("0.5", "m/d/yy h:mm", false).as_deref(),
            Some("1/0/00 12:00")
        );
        assert_eq!(
            display("45292.5", "yyyy\"年\"m\"月\"d\"日\" h\"时\"", false).as_deref(),
            Some("2024年1月1日 12时")
        );
    }

    #[test]
    fn rejects_out_of_range_and_non_date_tokens() {
        assert_eq!(display("2958466", "yyyy-mm-dd", false), None);
        assert_eq!(display("2957004", "yyyy-mm-dd", true), None);
        assert_eq!(display("-1", "yyyy-mm-dd", false), None);
        // 文本占位符、必显数字位不属于日期时间格式。
        assert_eq!(display("45292", "yyyy-mm-dd@", false), None);
        assert_eq!(display("45292", "yyyy-mm-dd 0", false), None);
    }

    #[test]
    fn iso_to_serial_plain_date() {
        assert_eq!(
            iso_to_serial("2024-01-01", false),
            Some(("45292".to_owned(), false))
        );
        // 前后空白被 trim。
        assert_eq!(
            iso_to_serial("  2024-01-01  ", false),
            Some(("45292".to_owned(), false))
        );
    }

    #[test]
    fn iso_to_serial_date_time_uses_exact_fraction() {
        assert_eq!(
            iso_to_serial("2024-01-01T18:00:00Z", false),
            Some(("45292.75".to_owned(), true))
        );
        assert_eq!(
            iso_to_serial("2024-01-01 06:00", false),
            Some(("45292.25".to_owned(), true))
        );
        assert_eq!(
            iso_to_serial("2024-01-01T12:00:00Z", false),
            Some(("45292.5".to_owned(), true))
        );
        // 小写 t / z 同样接受。
        assert_eq!(
            iso_to_serial("2024-01-01t18:00:00z", false),
            Some(("45292.75".to_owned(), true))
        );
    }

    #[test]
    fn iso_to_serial_pre_fake_leap_day() {
        assert_eq!(
            iso_to_serial("1900-02-28", false),
            Some(("59".to_owned(), false))
        );
        assert_eq!(
            iso_to_serial("1900-03-01", false),
            Some(("61".to_owned(), false))
        );
        assert_eq!(
            iso_to_serial("1900-01-01", false),
            Some(("1".to_owned(), false))
        );
    }

    #[test]
    fn iso_to_serial_rejects_invalid_input() {
        assert_eq!(iso_to_serial("2024-02-30", false), None);
        assert_eq!(iso_to_serial("2024-13-01", false), None);
        assert_eq!(iso_to_serial("2024-01-00", false), None);
        assert_eq!(iso_to_serial("1899-12-31", false), None);
        assert_eq!(iso_to_serial("1903-12-31", true), None);
        assert_eq!(iso_to_serial("2024-01-01T24:00:00Z", false), None);
        assert_eq!(iso_to_serial("2024-01-01T12:60:00Z", false), None);
        assert_eq!(iso_to_serial("2024-01-01T12:00:60Z", false), None);
        assert_eq!(iso_to_serial("", false), None);
        assert_eq!(iso_to_serial("not-a-date", false), None);
    }

    #[test]
    fn iso_to_serial_handles_1904_epoch_and_offsets() {
        assert_eq!(
            iso_to_serial("1904-01-01", true),
            Some(("0".to_owned(), false))
        );
        assert_eq!(
            iso_to_serial("2024-01-01", true),
            Some(("43830".to_owned(), false))
        );
        // 本地时间 01:00 带 +02:00 偏移，折算为前一日 23:00。
        assert_eq!(
            iso_to_serial("2024-01-02T01:00:00+02:00", false),
            Some(("45292.958333333333".to_owned(), true))
        );
        // 带 -05:00 偏移的晚间时刻跨入次日。
        assert_eq!(
            iso_to_serial("2024-01-01T22:00:00-05:00", false),
            Some(("45293.125".to_owned(), true))
        );
    }

    #[test]
    fn iso_to_serial_truncates_fraction_to_twelve_digits() {
        // 14:30 的十进制展开按 12 位截断。
        assert_eq!(
            iso_to_serial("2024-01-01T14:30:00", false),
            Some(("45292.604166666666".to_owned(), true))
        );
        // 毫秒级输入保留可终止的十进制展开。
        assert_eq!(
            iso_to_serial("2024-01-01T00:00:00.500Z", false),
            Some(("45292.000005787037".to_owned(), true))
        );
        // 跨入假闰年边界后仍保持合法序列号。
        assert_eq!(
            iso_to_serial("1900-03-01T00:30:00", false),
            Some(("61.020833333333".to_owned(), true))
        );
    }

    #[test]
    fn iso_serial_round_trips_through_display() {
        // ISO 文本换算出序列号后，应能被默认日期格式原样渲染回来。
        let (serial, has_time) = match iso_to_serial("2024-01-15T14:30:00Z", false) {
            Some(value) => value,
            None => panic!("合法 ISO 日期时间应可换算"),
        };
        assert!(has_time);
        assert_eq!(
            display(&serial, "yyyy-mm-dd hh:mm:ss", false).as_deref(),
            Some("2024-01-15 14:30:00")
        );
        let (date_only, has_time) = match iso_to_serial("2024-01-15", false) {
            Some(value) => value,
            None => panic!("合法 ISO 日期应可换算"),
        };
        assert!(!has_time);
        assert_eq!(
            display(&date_only, "yyyy\"年\"m\"月\"d\"日\"", false).as_deref(),
            Some("2024年1月15日")
        );
    }
}
