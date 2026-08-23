//! Built-in calculator: a tiny, dependency-free arithmetic expression parser
//! and evaluator powering the launcher's "type an expression, get an answer"
//! feature.
//!
//! Deliberately minimal — four binary operators with precedence, unary plus
//! and minus, power (right-associative), parentheses and decimals — because
//! the trigger only fires when the whole query is a self-contained expression.
//! It lives here (not in the app) so the parsing and formatting are pure and
//! unit-testable without any UI.

/// Whether `input` is worth showing as a calculator row, and if so the value.
///
/// Returns `None` for anything that is not a complete arithmetic expression:
/// empty input, plain words (no operator), a lone number, partial expressions
/// with trailing junk, or evaluations that do not produce a finite value
/// (division by zero, overflow).
///
/// `×`, `÷`, `−` and full-width parentheses typed on an IME are normalized to
/// their ASCII forms before parsing, so `２×３`-style math still works.
pub fn try_evaluate(input: &str) -> Option<f64> {
    let src = normalize(input);
    let src = src.trim();
    if src.is_empty() {
        return None;
    }
    // Require at least one binary operator so a bare number — or a lone sign,
    // e.g. `-5` — does not hijack ordinary search.
    if !has_binary_operator(src) {
        return None;
    }
    let mut parser = Parser::new(src);
    let value = parser.parse()?;
    if !parser.eof() {
        return None;
    }
    value.is_finite().then_some(value)
}

/// Whether `src` contains an operator that acts on two operands: strip a
/// leading sign (one or more `+`/`-`) and see if anything remains that is not
/// just sign-prefixing a single number.
fn has_binary_operator(src: &str) -> bool {
    let mut rest = src.trim();
    while rest.starts_with(['+', '-']) {
        rest = rest[1..].trim_start();
    }
    rest.chars()
        .any(|c| matches!(c, '+' | '-' | '*' | '/' | '%' | '^'))
}

/// Format a computed value for display: integers without a trailing decimal
/// point, and floats rounded to at most 10 decimal places with trailing zeros
/// trimmed (so `0.1 + 0.2` shows `0.3`, not the raw f64 noise).
pub fn format_value(value: f64) -> String {
    if !value.is_finite() {
        return "NaN".into();
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    let mut s = format!("{value:.10}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s
    }
}

/// Map a handful of common math symbols typed via an IME or keypad onto their
/// ASCII operators so the parser sees a single canonical alphabet.
fn normalize(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            '×' | '·' => '*',
            '÷' => '/',
            '−' | '–' | '—' => '-',
            '（' => '(',
            '）' => ')',
            _ => c,
        })
        .collect()
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    /// Parse and evaluate the whole expression. `None` on any syntax error.
    fn parse(&mut self) -> Option<f64> {
        let value = self.expr()?;
        // `eof` is checked by the caller; parse() tolerates trailing input so
        // callers can treat a partial parse as "not a calculator input".
        Some(value)
    }

    /// Whether the parser consumed the entire input (whitespace allowed).
    fn eof(&mut self) -> bool {
        self.skip_ws();
        self.pos >= self.src.len()
    }

    /// Advance past any ASCII whitespace between tokens.
    fn skip_ws(&mut self) {
        while self
            .src
            .as_bytes()
            .get(self.pos)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            self.pos += 1;
        }
    }

    /// expr := term (('+' | '-') term)*
    fn expr(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        loop {
            self.skip_ws();
            match self.peek_byte() {
                Some(b'+') => {
                    self.bump();
                    value += self.term()?;
                }
                Some(b'-') => {
                    self.bump();
                    value -= self.term()?;
                }
                _ => return Some(value),
            }
        }
    }

    /// term := factor (('*' | '/' | '%') factor)*
    fn term(&mut self) -> Option<f64> {
        let mut value = self.factor()?;
        loop {
            self.skip_ws();
            match self.peek_byte() {
                Some(b'*') => {
                    self.bump();
                    value *= self.factor()?;
                }
                Some(b'/') => {
                    self.bump();
                    value /= self.factor()?;
                }
                Some(b'%') => {
                    self.bump();
                    value %= self.factor()?;
                }
                _ => return Some(value),
            }
        }
    }

    /// factor := ('+' | '-') factor | power
    fn factor(&mut self) -> Option<f64> {
        self.skip_ws();
        match self.peek_byte() {
            Some(b'+') => {
                self.bump();
                self.factor()
            }
            Some(b'-') => {
                self.bump();
                self.factor().map(|v| -v)
            }
            _ => self.power(),
        }
    }

    /// power := atom ('^' factor)?   — right-associative: 2^3^2 = 2^(3^2)
    fn power(&mut self) -> Option<f64> {
        let base = self.atom()?;
        self.skip_ws();
        if self.peek_byte() == Some(b'^') {
            self.bump();
            let exponent = self.factor()?;
            Some(base.powf(exponent))
        } else {
            Some(base)
        }
    }

    /// atom := number | '(' expr ')'
    fn atom(&mut self) -> Option<f64> {
        self.skip_ws();
        match self.peek_byte() {
            Some(b'(') => {
                self.bump();
                let value = self.expr()?;
                if self.peek_byte() == Some(b')') {
                    self.bump();
                    Some(value)
                } else {
                    None
                }
            }
            Some(b'0'..=b'9' | b'.') => self.number(),
            _ => None,
        }
    }

    /// number := digits ['.' digits] [('e' | 'E') ['+' | '-'] digits]
    fn number(&mut self) -> Option<f64> {
        let start = self.pos;
        while matches!(self.peek_byte(), Some(b'0'..=b'9')) {
            self.bump();
        }
        if self.peek_byte() == Some(b'.') {
            self.bump();
            while matches!(self.peek_byte(), Some(b'0'..=b'9')) {
                self.bump();
            }
        }
        if matches!(self.peek_byte(), Some(b'e' | b'E')) {
            self.bump();
            if matches!(self.peek_byte(), Some(b'+' | b'-')) {
                self.bump();
            }
            while matches!(self.peek_byte(), Some(b'0'..=b'9')) {
                self.bump();
            }
        }
        if self.pos == start {
            return None;
        }
        self.src[start..self.pos].parse::<f64>().ok()
    }

    /// Next byte (whitespace not skipped; call `skip_ws` first for tokens).
    fn peek_byte(&self) -> Option<u8> {
        self.src.as_bytes().get(self.pos).copied()
    }

    fn bump(&mut self) {
        self.pos += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(input: &str) -> Option<f64> {
        try_evaluate(input)
    }

    fn assert_close(input: &str, expected: f64) {
        let got = eval(input).unwrap_or_else(|| panic!("`{input}` did not evaluate"));
        assert!(
            (got - expected).abs() < 1e-9,
            "`{input}` = {got}, expected {expected}"
        );
    }

    #[test]
    fn basic_arithmetic() {
        assert_close("1+2", 3.0);
        assert_close("10-4", 6.0);
        assert_close("3*4", 12.0);
        assert_close("10/4", 2.5);
    }

    #[test]
    fn precedence_and_parens() {
        assert_close("2+3*4", 14.0);
        assert_close("(2+3)*4", 20.0);
        assert_close("2*3+4*5", 26.0);
        assert_close("10-2-3", 5.0); // left-associative
        assert_close("8/2*2", 8.0); // left-associative
    }

    #[test]
    fn unary_minus() {
        assert_close("-5+3", -2.0);
        assert_close("5--3", 8.0);
        assert_close("5 - -3", 8.0);
        assert_close("2*-3", -6.0);
        assert_close("(-2)^2", 4.0);
        assert_close("-2^2", -4.0); // power binds tighter than unary minus
    }

    #[test]
    fn power_is_right_associative() {
        assert_close("2^3", 8.0);
        assert_close("2^3^2", 512.0); // 2^(3^2), not (2^3)^2
        assert_close("2^0.5", std::f64::consts::SQRT_2);
        assert_close("2^-1", 0.5);
    }

    #[test]
    fn modulo_and_decimals() {
        assert_close("10%3", 1.0);
        assert_close("5.5+4.5", 10.0);
        assert_close(".5*4", 2.0);
        assert_close("1e3+1", 1001.0);
        assert_close("1.5e-2*100", 1.5);
    }

    #[test]
    fn whitespace_is_ignored() {
        assert_close(" 2 +  2 ", 4.0);
        assert_close("2\t+\t2", 4.0);
    }

    #[test]
    fn unicode_symbols_are_normalized() {
        assert_close("2×3", 6.0);
        assert_close("10÷2", 5.0);
        assert_close("5−2", 3.0);
        assert_close("（1+2）×2", 6.0);
    }

    #[test]
    fn non_expressions_return_none() {
        assert_eq!(eval(""), None);
        assert_eq!(eval("   "), None);
        assert_eq!(eval("calc"), None);
        assert_eq!(eval("firefox"), None);
        assert_eq!(eval("42"), None); // bare number: keep ordinary search
        assert_eq!(eval("-5"), None); // lone sign: not a binary expression
        assert_eq!(eval("--5"), None);
        assert_eq!(eval("2+"), None); // dangling operator
        assert_eq!(eval("(2+3"), None); // unclosed paren
        assert_eq!(eval("2+3)"), None); // stray paren
        assert_eq!(eval("2+abc"), None);
        assert_eq!(eval("1/0"), None); // division by zero
        assert_eq!(eval("0/0"), None);
        assert_eq!(eval("5%0"), None);
        assert_eq!(eval("2 2"), None); // implicit multiplication not supported
    }

    #[test]
    fn format_integer_results_without_fraction() {
        assert_eq!(format_value(4.0), "4");
        assert_eq!(format_value(-0.0), "0");
        assert_eq!(format_value(1e15), "1000000000000000");
    }

    #[test]
    fn format_floats_trim_trailing_zeros() {
        assert_eq!(format_value(0.1 + 0.2), "0.3");
        assert_eq!(format_value(10.0 / 3.0), "3.3333333333");
        assert_eq!(format_value(2.5), "2.5");
        assert_eq!(format_value(1.5e-2), "0.015");
    }
}
