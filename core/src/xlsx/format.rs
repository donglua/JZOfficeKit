/// 标准内置数字格式表：`numFmtId` 到格式串的映射。
///
/// 仅覆盖纯文本渲染可无损表达的条目；条件段、分数与 DBNum 等仍通过 `None` 交由调用方回退原值。
pub(super) const fn builtin(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 => "$#,##0;($#,##0)",
        6 => "$#,##0;[Red]($#,##0)",
        7 => "$#,##0.00;($#,##0.00)",
        8 => "$#,##0.00;[Red]($#,##0.00)",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        14 => "mm-dd-yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        37 => "#,##0;(#,##0)",
        38 => "#,##0;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        41 => "_(* #,##0_);_(* (#,##0);_(* \"-\"_);_(@_)",
        42 => "_(\"$\"* #,##0_);_(\"$\"* (#,##0);_(\"$\"* \"-\"_);_(@_)",
        43 => "_(* #,##0.00_);_(* (#,##0.00);_(* \"-\"??_);_(@_)",
        44 => "_(\"$\"* #,##0.00_);_(\"$\"* (#,##0.00);_(\"$\"* \"-\"??_);_(@_)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mm:ss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

/// 格式段词法令牌。
///
/// 由 [`tokenize_section`] 单遍扫描产生，数字骨架渲染器与日期时间引擎共享同一条令牌流。
/// 令牌只描述语法，不携带语义分类（例如 `m` 究竟是月份还是分钟由日期引擎按相邻令牌消歧）。
#[derive(Debug, Clone, PartialEq, Eq)]
// 令牌名沿用设计文档约定的 `DateToken`，与枚举名重复属于有意为之
#[allow(clippy::enum_variant_names)]
pub(super) enum Token {
    /// 引号字面量、反斜杠转义字符或未转义的普通文本字符。
    Literal(String),
    /// 会计填充符 `* c`，纯文本预览折叠为单个空格。
    FillSpace,
    /// 耗时括号 `[h]` / `[mm]` 等，保留括号内的原始内容。
    BracketElapsed(String),
    /// 原值占位符 `@`。
    TextPlaceholder,
    /// 必显数字位 `0`。
    DigitZero,
    /// 可选数字位 `#`。
    DigitHash,
    /// 小数点 `.`。
    Dot,
    /// 逗号 `,`：整数段内为千位分组，主干末尾为千分位缩放。
    Comma,
    /// 百分比符号 `%`。
    Percent,
    /// 科学计数标记 `E+` / `E-` / `e+` / `e-`。
    Exponent {
        /// 是否保留大写字母 `E`。
        uppercase: bool,
        /// 非负指数是否输出 `+`。
        plus: bool,
    },
    /// 连续日期字母令牌，保留原始大小写（如 `yyyy`、`mm`、`ddd`）。
    DateToken(String),
    /// 上下午标记 `AM/PM`、`am/pm`、`A/P`、`a/p`，保留原始写法。
    AmPm(String),
    /// 对齐问号 `?`，仅零值文本段可跳过。
    Question,
}

/// 把单元格数值按格式代码渲染为纯文本；不支持时返回 `None` 交由调用方回退原值。
pub(crate) fn display(raw: &str, code: &str, date1904: bool) -> Option<String> {
    if code.is_empty() {
        return None;
    }
    if code.eq_ignore_ascii_case("General") || code == "@" {
        return Some(raw.to_owned());
    }
    if raw.len() > 512 || code.len() > 128 {
        return None;
    }
    let value = Decimal::parse(raw)?;

    // 1. 按未转义且不在引号/中括号内的分号切分 1~4 段
    let sections = split_format_sections(code);
    if sections.is_empty() || sections.len() > 4 {
        return None;
    }
    let is_zero = value.is_zero();
    let is_negative = value.negative && !is_zero;
    let (selected, negative_section) = match sections.len() {
        1 => (sections[0], false),
        2 if is_negative => (sections[1], true),
        2 => (sections[0], false),
        _ if is_negative => (sections[1], true),
        _ if is_zero => (sections[2], false),
        _ => (sections[0], false),
    };
    if selected.is_empty() {
        return if sections.len() > 1 {
            Some(String::new())
        } else {
            None
        };
    }

    // 2. 单遍词法分词
    let tokens = tokenize_section(selected)?;
    if tokens.is_empty() {
        return if sections.len() > 1 {
            Some(String::new())
        } else {
            None
        };
    }

    // 3. 日期时间令牌优先于数字骨架；两者混排时交由日期引擎裁决
    let has_date = tokens.iter().any(|token| {
        matches!(
            token,
            Token::DateToken(_) | Token::BracketElapsed(_) | Token::AmPm(_)
        )
    });
    if has_date {
        // 日期格式中的 `,` `%` `?` 是普通字面量，不是数字骨架标记
        let normalized: Vec<Token> = tokens
            .into_iter()
            .map(|token| match token {
                Token::Comma => Token::Literal(",".to_owned()),
                Token::Percent => Token::Literal("%".to_owned()),
                Token::Question => Token::Literal("?".to_owned()),
                other => other,
            })
            .collect();
        return super::dates::display_tokens(&value, &normalized, date1904);
    }

    let has_number = tokens.iter().any(|token| {
        matches!(
            token,
            Token::DigitZero | Token::DigitHash | Token::Exponent { .. }
        )
    });
    if has_number {
        // 负数专属段以绝对值渲染，符号由该段自带字面量决定
        let render_value = if negative_section {
            value.absolute()
        } else {
            value
        };
        return render_number(&render_value, &tokens, !negative_section);
    }

    // 纯文本段（零值段 `"-"`、`"N/A"`、`_(@_)` 等）
    render_literals(&tokens, raw)
}

/// 单遍扫描选定的格式段并生成令牌流。
///
/// 遇到条件比较段、分数、DBNum 或未闭合的引号/中括号时返回 `None`。
pub(super) fn tokenize_section(code: &str) -> Option<Vec<Token>> {
    if code.len() > 128 {
        return None;
    }
    let mut tokens = Vec::new();
    let mut cursor = 0_usize;
    while cursor < code.len() {
        let rest = code.get(cursor..)?;
        let ch = rest.chars().next()?;

        // 上下午标记必须先于日期字母令牌识别，否则 `AM/PM` 会被拆成日期字母
        if let Some(pattern) = ["AM/PM", "am/pm", "A/P", "a/p"]
            .into_iter()
            .find(|pattern| rest.starts_with(pattern))
        {
            tokens.push(Token::AmPm(pattern.to_owned()));
            cursor += pattern.len();
            continue;
        }

        match ch {
            '"' => {
                let close = rest[1..].find('"').map(|offset| offset + 1)?;
                tokens.push(Token::Literal(rest[1..close].to_owned()));
                cursor += close + 1;
            }
            '\\' => {
                let escaped = rest[1..].chars().next()?;
                tokens.push(Token::Literal(escaped.to_string()));
                cursor += 1 + escaped.len_utf8();
            }
            '_' => {
                // 会计宽度留白占位符：跳过后一个字符，纯文本预览下不产生空格
                let skipped = rest[1..].chars().next()?;
                cursor += 1 + skipped.len_utf8();
            }
            '*' => {
                // 会计重复填充符：折叠为单个空格
                let skipped = rest[1..].chars().next()?;
                tokens.push(Token::FillSpace);
                cursor += 1 + skipped.len_utf8();
            }
            '[' => {
                let close = rest.find(']')?;
                let tag = rest[1..close].trim();
                let lower = tag.to_ascii_lowercase();
                if matches!(lower.as_str(), "h" | "hh" | "m" | "mm" | "s" | "ss") {
                    tokens.push(Token::BracketElapsed(tag.to_owned()));
                } else if let Some(locale) = tag.strip_prefix('$') {
                    // `[$¥-804]` 提取货币文本；`[$-409]` 为纯区域码，直接剥离
                    if !locale.starts_with('-') {
                        let symbol = locale.split('-').next().unwrap_or_default().trim();
                        if !symbol.is_empty() {
                            tokens.push(Token::Literal(symbol.to_owned()));
                        }
                    }
                } else if is_color_tag(&lower) {
                    // 颜色标签在纯文本渲染中忽略
                } else {
                    // 条件比较段（`[>0]`、`[<=100]`）与 DBNum 指令暂不支持
                    return None;
                }
                cursor += close + 1;
            }
            '@' => {
                tokens.push(Token::TextPlaceholder);
                cursor += 1;
            }
            '0' => {
                tokens.push(Token::DigitZero);
                cursor += 1;
            }
            '#' => {
                tokens.push(Token::DigitHash);
                cursor += 1;
            }
            '.' => {
                tokens.push(Token::Dot);
                cursor += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                cursor += 1;
            }
            '%' => {
                tokens.push(Token::Percent);
                cursor += 1;
            }
            '?' => {
                tokens.push(Token::Question);
                cursor += 1;
            }
            'E' | 'e'
                if rest
                    .as_bytes()
                    .get(1)
                    .is_some_and(|sign| *sign == b'+' || *sign == b'-') =>
            {
                let plus = rest.as_bytes().get(1) == Some(&b'+');
                tokens.push(Token::Exponent {
                    uppercase: ch == 'E',
                    plus,
                });
                cursor += 2;
            }
            'y' | 'Y' | 'm' | 'M' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' => {
                let target = u8::try_from(ch).ok()?;
                let mut width = 0_usize;
                while rest.as_bytes().get(width) == Some(&target) {
                    width += 1;
                }
                tokens.push(Token::DateToken(rest[..width].to_owned()));
                cursor += width;
            }
            _ => {
                tokens.push(Token::Literal(ch.to_string()));
                cursor += ch.len_utf8();
            }
        }
    }
    Some(tokens)
}

fn is_color_tag(tag: &str) -> bool {
    matches!(
        tag,
        "black" | "blue" | "cyan" | "green" | "magenta" | "red" | "white" | "yellow"
    ) || (tag.starts_with("color") && tag[5..].chars().all(|c| c.is_ascii_digit()))
}

fn split_format_sections(code: &str) -> Vec<&str> {
    let mut sections = Vec::new();
    let mut in_quotes = false;
    let mut in_brackets = false;
    let mut start = 0;
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'"' => in_quotes = !in_quotes,
            b'[' if !in_quotes => in_brackets = true,
            b']' if !in_quotes => in_brackets = false,
            b';' if !in_quotes && !in_brackets => {
                sections.push(&code[start..i]);
                start = i + 1;
            }
            _ => (),
        }
        i += 1;
    }
    sections.push(&code[start..]);
    sections
}

/// 字符串十进制数：以 `digits` 与十进制小数点位置 `point` 精确表示，避免 `f64` 二进制误差。
pub(super) struct Decimal {
    negative: bool,
    digits: Vec<u8>,
    point: i32,
}

impl Decimal {
    /// 解析十进制字符串；非有限值、非法字面量与超界指数一律返回 `None`。
    pub(super) fn parse(raw: &str) -> Option<Self> {
        if !raw.parse::<f64>().ok()?.is_finite() {
            return None;
        }
        let negative = raw.starts_with('-');
        let unsigned = raw.strip_prefix(['-', '+']).unwrap_or(raw);
        let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
            Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
            None => (unsigned, 0),
        };
        if !(-400..=400).contains(&exponent) {
            return None;
        }
        let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let mut digits: Vec<u8> = integer.bytes().chain(fraction.bytes()).collect();
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let leading = digits.iter().take_while(|digit| **digit == b'0').count();
        let point = i32::try_from(integer.len()).ok()? + exponent - i32::try_from(leading).ok()?;
        digits.drain(..leading);
        Some(Self {
            negative,
            digits,
            point,
        })
    }

    /// 数值是否为零（含 `-0`、`0e300` 等写法）。
    fn is_zero(&self) -> bool {
        self.digits.is_empty() || self.digits.iter().all(|digit| *digit == b'0')
    }

    /// 平移十进制小数点位置，供百分比与千分位缩放使用。
    fn shift_point(&mut self, delta: i32) {
        self.point = self.point.saturating_add(delta);
    }

    /// 返回应用小数偏移后的副本。
    fn scaled(&self, delta: i32) -> Self {
        let mut copy = Self {
            negative: self.negative,
            digits: self.digits.clone(),
            point: self.point,
        };
        copy.shift_point(delta);
        copy
    }

    /// 返回去掉符号的副本，供负数专属段按绝对值渲染。
    fn absolute(&self) -> Self {
        Self {
            negative: false,
            digits: self.digits.clone(),
            point: self.point,
        }
    }

    fn quantize(&self, places: i32, round: bool) -> Option<Vec<u8>> {
        let end = self.point + places;
        if end > 512 {
            return None;
        }
        if end < 0 || self.digits.is_empty() {
            return Some(vec![b'0']);
        }
        let end = usize::try_from(end).ok()?;
        let mut result = self.digits[..end.min(self.digits.len())].to_vec();
        result.resize(end, b'0');
        if round && self.digits.get(end).is_some_and(|digit| *digit >= b'5') {
            carry_into(&mut result);
        }
        if result.is_empty() {
            result.push(b'0');
        }
        Some(result)
    }

    pub(super) fn units(&self, multiplier: u32, round: bool) -> Option<u64> {
        if self.negative && !self.digits.is_empty() {
            return None;
        }
        let mut scaled = Self {
            negative: false,
            digits: self.digits.clone(),
            point: self.point,
        };
        let mut carry = 0;
        for digit in scaled.digits.iter_mut().rev() {
            let product = u32::from(*digit - b'0')
                .checked_mul(multiplier)?
                .checked_add(carry)?;
            *digit = u8::try_from(product % 10).ok()? + b'0';
            carry = product / 10;
        }
        while carry > 0 {
            scaled
                .digits
                .insert(0, u8::try_from(carry % 10).ok()? + b'0');
            scaled.point += 1;
            carry /= 10;
        }
        scaled
            .quantize(0, round)?
            .into_iter()
            .try_fold(0_u64, |value, digit| {
                value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
            })
    }
}

/// 就地对十进制数字串加一，溢出时在最高位插入进位位。
fn carry_into(digits: &mut Vec<u8>) {
    let mut carry = true;
    for digit in digits.iter_mut().rev() {
        if *digit < b'9' {
            *digit += 1;
            carry = false;
            break;
        }
        *digit = b'0';
    }
    if carry {
        digits.insert(0, b'1');
    }
}

/// 数字骨架：整数/小数占位掩码、千位分组、缩放逗号与科学计数规格。
struct Skeleton {
    /// 整数段必显位数（`0` 的个数）。
    int_required: usize,
    /// 整数段占位总数（`0` 与 `#`），决定工程计数的指数步长。
    int_slots: usize,
    /// 小数段掩码：`true` 为必显 `0`，`false` 为可选 `#`。
    fraction: Vec<bool>,
    /// 整数段是否含千位分组逗号。
    grouped: bool,
    /// 主干末尾的千分位缩放逗号个数。
    scale_commas: i32,
    /// 科学计数规格；`None` 表示常规数值。
    exponent: Option<ExponentSpec>,
}

struct ExponentSpec {
    uppercase: bool,
    plus: bool,
    digits: usize,
}

/// 解析数字主干令牌，遇到非法序列（多段骨架、先 `#` 后 `0`、裸小数点等）返回 `None`。
fn parse_skeleton(body: &[Token]) -> Option<Skeleton> {
    let at = body
        .iter()
        .position(|token| matches!(token, Token::Exponent { .. }));
    let (mantissa, exponent) = match at {
        Some(at) => (&body[..at], Some(&body[at..])),
        None => (body, None),
    };
    let mut int_required = 0_usize;
    let mut int_slots = 0_usize;
    let mut fraction: Vec<bool> = Vec::new();
    let mut grouped = false;
    let mut in_fraction = false;
    let mut dot_seen = false;
    let mut commas_after_digits = 0_usize;
    for token in mantissa {
        match token {
            Token::DigitZero | Token::DigitHash => {
                let required = matches!(token, Token::DigitZero);
                if in_fraction {
                    // 小数段内部不允许千位分组
                    if commas_after_digits > 0 {
                        return None;
                    }
                    fraction.push(required);
                } else {
                    if commas_after_digits > 0 {
                        grouped = true;
                    }
                    int_slots += 1;
                    if required {
                        int_required += 1;
                    }
                }
                commas_after_digits = 0;
            }
            Token::Dot => {
                if dot_seen || commas_after_digits > 0 {
                    return None;
                }
                dot_seen = true;
                in_fraction = true;
            }
            Token::Comma => {
                if int_slots == 0 && fraction.is_empty() {
                    return None;
                }
                commas_after_digits += 1;
                if commas_after_digits > 10 {
                    return None;
                }
            }
            _ => return None,
        }
    }
    if int_slots == 0 || (dot_seen && fraction.is_empty()) {
        return None;
    }
    if fraction.len() > 15 {
        return None;
    }
    // 小数段不得先出现可选 `#` 再出现必显 `0`
    let mut optional_seen = false;
    for required in &fraction {
        if *required {
            if optional_seen {
                return None;
            }
        } else {
            optional_seen = true;
        }
    }
    let exponent = match exponent {
        Some(rest) => {
            let (uppercase, plus) = match rest.first() {
                Some(Token::Exponent { uppercase, plus }) => (*uppercase, *plus),
                _ => return None,
            };
            let digits = rest.len() - 1;
            if !(1..=4).contains(&digits)
                || !rest[1..]
                    .iter()
                    .all(|token| matches!(token, Token::DigitZero))
            {
                return None;
            }
            Some(ExponentSpec {
                uppercase,
                plus,
                digits,
            })
        }
        None => None,
    };
    Some(Skeleton {
        int_required,
        int_slots,
        fraction,
        grouped,
        scale_commas: i32::try_from(commas_after_digits).ok()?,
        exponent,
    })
}

fn is_numeric_token(token: &Token) -> bool {
    matches!(
        token,
        Token::DigitZero | Token::DigitHash | Token::Dot | Token::Comma | Token::Exponent { .. }
    )
}

/// 定位唯一的连续数字主干；出现两段骨架（如 `000-0000`）时返回 `None`。
fn numeric_span(tokens: &[Token]) -> Option<(usize, usize)> {
    let first = tokens.iter().position(is_numeric_token)?;
    let last = tokens.iter().rposition(is_numeric_token)?;
    if !tokens.get(first..=last)?.iter().all(is_numeric_token) {
        return None;
    }
    Some((first, last))
}

fn render_affix(tokens: &[Token]) -> Option<String> {
    let mut text = String::new();
    for token in tokens {
        match token {
            Token::Literal(inner) => text.push_str(inner),
            Token::FillSpace => {
                if !text.ends_with(' ') {
                    text.push(' ');
                }
            }
            Token::Percent => text.push('%'),
            Token::Comma => text.push(','),
            _ => return None,
        }
    }
    Some(text)
}

fn render_literals(tokens: &[Token], raw: &str) -> Option<String> {
    let mut text = String::new();
    for token in tokens {
        match token {
            Token::Literal(inner) => text.push_str(inner),
            Token::FillSpace => {
                if !text.ends_with(' ') {
                    text.push(' ');
                }
            }
            Token::Percent => text.push('%'),
            Token::Comma => text.push(','),
            Token::TextPlaceholder => text.push_str(raw),
            Token::Question => (),
            _ => return None,
        }
    }
    Some(text)
}

fn render_number(value: &Decimal, tokens: &[Token], allow_auto_minus: bool) -> Option<String> {
    // 对齐问号只出现在会计零值文本段，数字骨架内一律视为分数格式
    if tokens.iter().any(|token| matches!(token, Token::Question)) {
        return None;
    }
    let (first, last) = numeric_span(tokens)?;
    let skeleton = parse_skeleton(tokens.get(first..=last)?)?;
    let percent = tokens
        .iter()
        .filter(|token| matches!(token, Token::Percent))
        .count();
    // 百分比放大两位，千分位缩放除以 10³
    let shift = i32::try_from(percent).ok()?.checked_mul(2)? - skeleton.scale_commas * 3;
    let (body, negative) = if skeleton.exponent.is_some() {
        render_scientific(value, &skeleton, shift)?
    } else {
        render_plain(value, &skeleton, shift)?
    };
    let prefix = render_affix(tokens.get(..first)?)?;
    let suffix = render_affix(tokens.get(last + 1..)?)?;
    let mut result = String::with_capacity(prefix.len() + body.len() + suffix.len() + 1);
    // 单段格式下负号置于所有前缀字面量之前；段内已有 `-` 或 `(` 时交由格式自身表达符号
    if allow_auto_minus && negative && !prefix.contains('-') && !prefix.contains('(') {
        result.push('-');
    }
    result.push_str(&prefix);
    result.push_str(&body);
    result.push_str(&suffix);
    Some(result)
}

/// 常规数值渲染：按 `shift` 缩放后精确四舍五入，再拼接整数分组与修剪后的小数。
fn render_plain(value: &Decimal, skeleton: &Skeleton, shift: i32) -> Option<(String, bool)> {
    let places = i32::try_from(skeleton.fraction.len()).ok()?;
    let required_places = skeleton.fraction.iter().take_while(|flag| **flag).count();
    let digits = value.scaled(shift).quantize(places, true)?;
    let width = skeleton.fraction.len();
    // 左侧补齐到「整数 1 位 + 小数位」，随后按同一分割点拆分整数与小数
    let mut padded = vec![b'0'; (width + 1).saturating_sub(digits.len())];
    padded.extend_from_slice(&digits);
    let split = padded.len() - width;
    let mut int_start = 0_usize;
    while int_start + skeleton.int_required < split && padded[int_start] == b'0' {
        int_start += 1;
    }
    let mut frac_end = padded.len();
    while frac_end > split + required_places && padded[frac_end - 1] == b'0' {
        frac_end -= 1;
    }
    let negative = value.negative
        && padded[int_start..frac_end]
            .iter()
            .any(|digit| *digit != b'0');
    let width_int = split - int_start;
    let pad = skeleton.int_required.saturating_sub(width_int);
    let mut text = String::with_capacity(padded.len() + 4);
    for _ in 0..pad {
        text.push('0');
    }
    for (offset, digit) in padded[int_start..split].iter().enumerate() {
        if skeleton.grouped && offset > 0 && (width_int - offset) % 3 == 0 {
            text.push(',');
        }
        text.push(char::from(*digit));
    }
    if frac_end > split {
        text.push('.');
        for digit in &padded[split..frac_end] {
            text.push(char::from(*digit));
        }
    }
    if text.is_empty() {
        text.push('0');
    }
    Some((text, negative))
}

/// 科学计数渲染：支持 `E`/`e` 大小写、`+`/`-` 指数符号、标准步长 1 与工程步长 3。
fn render_scientific(value: &Decimal, skeleton: &Skeleton, shift: i32) -> Option<(String, bool)> {
    let spec = skeleton.exponent.as_ref()?;
    let step = i32::try_from(skeleton.int_slots.max(1)).ok()?;
    let decimals = skeleton.fraction.len();
    let required_places = skeleton.fraction.iter().take_while(|flag| **flag).count();
    let plain_zero = value.is_zero();
    if plain_zero {
        // 零值的尾数按必显位渲染为 0，指数固定为零
        let mut text = "0".repeat(skeleton.int_required.max(1));
        if required_places > 0 {
            text.push('.');
            text.push_str(&"0".repeat(required_places));
        }
        text.push_str(&exponent_text(spec, 0));
        return Some((text, false));
    }
    // 原始最高位十进制指数
    let mut exponent_value = value.point.checked_add(shift)?.checked_sub(1)?;
    let mut carried = false;
    let mut resolved: Option<(Vec<u8>, usize, i32)> = None;
    // 跨位进位最多上移一位，四次迭代足以收敛
    for _ in 0..4 {
        let bucket = exponent_value.div_euclid(step) * step;
        let int_digits = usize::try_from(exponent_value - bucket + 1).ok()?;
        if int_digits == 0 || i32::try_from(int_digits).ok()? > step {
            return None;
        }
        let significant = int_digits.checked_add(decimals)?;
        if carried {
            // 进位后数值恰为 10^exponent_value，其余位全为零
            let mut digits = vec![b'0'; significant];
            if let Some(first) = digits.first_mut() {
                *first = b'1';
            }
            resolved = Some((digits, int_digits, bucket));
            break;
        }
        let mut rounded = value
            .digits
            .get(..significant.min(value.digits.len()))?
            .to_vec();
        rounded.resize(significant, b'0');
        if value
            .digits
            .get(significant)
            .is_some_and(|digit| *digit >= b'5')
        {
            carry_into(&mut rounded);
        }
        if rounded.len() > significant {
            exponent_value = exponent_value.checked_add(1)?;
            carried = true;
            continue;
        }
        resolved = Some((rounded, int_digits, bucket));
        break;
    }
    let (digits, int_digits, bucket) = resolved?;
    let mut int_text: String = digits
        .get(..int_digits)?
        .iter()
        .map(|digit| char::from(*digit))
        .collect();
    while int_text.len() < skeleton.int_required {
        int_text.insert(0, '0');
    }
    let mut frac_end = digits.len();
    while frac_end > int_digits + required_places && digits[frac_end - 1] == b'0' {
        frac_end -= 1;
    }
    if frac_end > int_digits {
        int_text.push('.');
        for digit in digits.get(int_digits..frac_end)? {
            int_text.push(char::from(*digit));
        }
    }
    int_text.push_str(&exponent_text(spec, bucket));
    let negative = value.negative && digits.iter().any(|digit| *digit != b'0');
    Some((int_text, negative))
}

fn exponent_text(spec: &ExponentSpec, exponent: i32) -> String {
    let mut text = String::with_capacity(spec.digits + 3);
    text.push(if spec.uppercase { 'E' } else { 'e' });
    if exponent < 0 {
        text.push('-');
    } else if spec.plus {
        text.push('+');
    }
    let magnitude = exponent.unsigned_abs().to_string();
    for _ in magnitude.len()..spec.digits {
        text.push('0');
    }
    text.push_str(&magnitude);
    text
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
