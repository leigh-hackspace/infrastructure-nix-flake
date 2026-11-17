//! `calc` — one arithmetic expression in, one exact number out.
//!
//! The tool that uses it is `tools_basics::calculate`, and the reason this exists as its own
//! file is the same reason it exists as its own *job*: everything in here is a parser, and
//! everything in the tool file is a table and a sentence. They change for different reasons —
//! a new unit is a line in the table, a precedence bug is never a line in the table.
//!
//! Why not let the model do arithmetic? Because it is good enough at sums to sound right and
//! not good enough to be trusted with one, and the answers go out loud into a room with power
//! tools: `3/8` of an inch, a parallel resistor bank, watts from volts and amps. `AGENTS.md`
//! already refuses to let the model decide when a doorbell is a doorbell; a resistor value is
//! the same category of decision.
//!
//! Three rules inside the grammar are load-bearing and each has a test, because every one of
//! them is a place where a plausible reading is a *wrong answer* rather than an error:
//!
//! * `%` is **per hundred**, never a remainder. Remainders are the word `mod`. Had `%` been
//!   modulo, `12% * 340` and `50 % 7` would each have a defensible parse and one of them
//!   would be silently wrong.
//! * Two numbers side by side never multiply. `2(3+4)` and `2pi` are unambiguous; `2 3` is a
//!   typo, and multiplying typos is how a typo becomes an answer.
//! * A comma between digits outside brackets is a thousands separator (`1,000`), and inside
//!   them an argument separator (`pow(2,10)`). Bracket depth is the only way to tell them
//!   apart, so depth is tracked, and that is its sole purpose.

/// One token of an expression. Numbers arrive already parsed: a tokenizer that hands out
/// strings only to have the parser re-parse them is two chances to get `1.2.3` wrong.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Op(char),
    LP,
    RP,
    Comma,
}

/// Parse and evaluate one expression. Public only to the tests.
pub(crate) fn eval(src: &str) -> Result<f64, String> {
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return Err("the expression was empty".into());
    }
    let mut p = Parser { toks, at: 0 };
    let v = p.expr()?;
    if let Some(t) = p.peek() {
        return Err(format!("unexpected {} after the end of the expression", describe(t)));
    }
    Ok(v)
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = src.chars().collect();
    let mut out: Vec<Tok> = Vec::new();
    let mut i = 0;
    let mut depth = 0usize;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() || c == '_' {
            i += 1;
            continue;
        }
        match c {
            '×' | '✕' | '∗' => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '−' | '–' => {
                out.push(Tok::Op('-'));
                i += 1;
            }
            // `**` is how a programmer writes a power; `^` is how everyone else does.
            '*' if cs.get(i + 1) == Some(&'*') => {
                out.push(Tok::Op('^'));
                i += 2;
            }
            '(' => {
                depth += 1;
                out.push(Tok::LP);
                i += 1;
            }
            ')' => {
                depth = depth.saturating_sub(1);
                out.push(Tok::RP);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '+' | '-' | '*' | '/' | '%' | '^' => {
                out.push(Tok::Op(c));
                i += 1;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let mut s = String::new();
                while i < cs.len()
                    && (cs[i].is_ascii_digit()
                        || cs[i] == '.'
                        || cs[i] == 'e'
                        || cs[i] == 'E'
                        // …and the sign of an exponent, which belongs to the number and not
                        // to the expression: `2e+1` is twenty, `2 e+1` is two times e plus one.
                        || (matches!(cs[i], '+' | '-') && s.ends_with(['e', 'E']))
                        // A comma between digits at depth 0 is a thousands separator, because
                        // models type `1,000`; inside parentheses a comma is an argument
                        // separator, because `pow(2,10)` has no other reading. Depth is the only
                        // way to tell them apart, and it is the only reason depth exists.
                        || (cs[i] == ',' && depth == 0 && matches!(cs.get(i + 1), Some(&c) if c.is_ascii_digit())))
                {
                    // `1e3` and `2e+1` are numbers; `e` as Euler's constant is only a name
                    // when it is not stuck to a digit, which is what this look-ahead is for.
                    if cs[i] == 'e' || cs[i] == 'E' {
                        let signed = matches!(cs.get(i + 1), Some(&'+') | Some(&'-'));
                        let next = if signed { i + 2 } else { i + 1 };
                        if !matches!(cs.get(next), Some(&c) if c.is_ascii_digit()) {
                            break;
                        }
                    }
                    if cs[i] != ',' {
                        s.push(cs[i]);
                    }
                    i += 1;
                }
                let v = s
                    .parse::<f64>()
                    .map_err(|_| format!("\"{s}\" is not a number I can read"))?;
                out.push(Tok::Num(v));
            }
            c if c.is_alphabetic() => {
                let mut s = String::new();
                while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                    s.push(cs[i]);
                    i += 1;
                }
                out.push(Tok::Name(s.to_lowercase()));
            }
            c => return Err(format!("I do not understand the character \"{c}\"")),
        }
    }
    Ok(out)
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Num(v) => format!("number {v}"),
        Tok::Name(n) => format!("\"{n}\""),
        Tok::Op(c) => format!("\"{c}\""),
        Tok::LP => "\"(\"".into(),
        Tok::RP => "\")\"".into(),
        Tok::Comma => "\",\"".into(),
    }
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn eat_op(&mut self, ops: &[char]) -> Option<char> {
        if let Some(Tok::Op(c)) = self.peek() {
            let c = *c;
            if ops.contains(&c) {
                self.at += 1;
                return Some(c);
            }
        }
        None
    }

    /// expr := term (('+' | '-') term)*
    fn expr(&mut self) -> Result<f64, String> {
        let mut v = self.term()?;
        while let Some(op) = self.eat_op(&['+', '-']) {
            let rhs = self.term()?;
            v = if op == '+' { v + rhs } else { v - rhs };
        }
        Ok(v)
    }

    /// term := postfix (('*' | '/' | mod) postfix)*  — and implicit `2pi` / `2(3)` / `(1)(2)`,
    /// but never `2 3`: two numbers side by side is a typo, and multiplying them silently is
    /// how a typo becomes an answer.
    fn term(&mut self) -> Result<f64, String> {
        let mut v = self.postfix()?;
        loop {
            if let Some(op) = self.eat_op(&['*', '/']) {
                let rhs = self.postfix()?;
                if op == '/' && rhs == 0.0 {
                    return Err("division by zero".into());
                }
                v = if op == '*' { v * rhs } else { v / rhs };
                continue;
            }
            if let Some(Tok::Name(n)) = self.peek() {
                if n == "mod" {
                    self.at += 1;
                    let rhs = self.postfix()?;
                    v %= rhs;
                    continue;
                }
            }
            let implicit = matches!(self.peek(), Some(Tok::LP) | Some(Tok::Name(_)));
            if implicit {
                let rhs = self.postfix()?;
                v *= rhs;
                continue;
            }
            return Ok(v);
        }
    }

    /// postfix := unary ('%')* — a trailing percent is "per hundred", which is what a person
    /// means by 12%. Remainders are the word `mod`, because `%` cannot be both and guessing
    /// which one was meant is the failure mode this tool exists to remove.
    fn postfix(&mut self) -> Result<f64, String> {
        let mut v = self.unary()?;
        while self.eat_op(&['%']).is_some() {
            v /= 100.0;
        }
        Ok(v)
    }

    fn unary(&mut self) -> Result<f64, String> {
        if self.eat_op(&['-']).is_some() {
            return Ok(-self.unary()?);
        }
        if self.eat_op(&['+']).is_some() {
            return self.unary();
        }
        self.power()
    }

    /// power := atom ('^' unary)? — right associative, and the exponent may be negative.
    fn power(&mut self) -> Result<f64, String> {
        let base = self.atom()?;
        if self.eat_op(&['^']).is_some() {
            let e = self.unary()?;
            return Ok(base.powf(e));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<f64, String> {
        match self.peek().cloned() {
            Some(Tok::Num(v)) => {
                self.at += 1;
                Ok(v)
            }
            Some(Tok::LP) => {
                self.at += 1;
                let v = self.expr()?;
                if !matches!(self.peek(), Some(Tok::RP)) {
                    return Err("a bracket was left open".into());
                }
                self.at += 1;
                Ok(v)
            }
            Some(Tok::Name(name)) => {
                self.at += 1;
                if !matches!(self.peek(), Some(Tok::LP)) {
                    return constant(&name);
                }
                self.at += 1;
                let mut a = Vec::new();
                if !matches!(self.peek(), Some(Tok::RP)) {
                    loop {
                        a.push(self.expr()?);
                        if matches!(self.peek(), Some(Tok::Comma)) {
                            self.at += 1;
                            continue;
                        }
                        break;
                    }
                }
                if !matches!(self.peek(), Some(Tok::RP)) {
                    return Err(format!("{}( … is missing its closing bracket", name));
                }
                self.at += 1;
                call(&name, &a)
            }
            Some(t) => Err(format!("expected a number, found {}", describe(&t))),
            None => Err("the expression ends too early".into()),
        }
    }
}

fn constant(name: &str) -> Result<f64, String> {
    match name {
        "pi" => Ok(std::f64::consts::PI),
        "e" => Ok(std::f64::consts::E),
        "tau" => Ok(std::f64::consts::TAU),
        other => Err(format!(
            "I do not know \"{other}\". This calculator knows the numbers pi, e and tau and \
             the functions {KNOWN}. Units are not numbers: drop them (230 volts × 2 amps is just \
             230*2) and use the convert tool for the units.",
            KNOWN = KNOWN_FUNCTIONS
        )),
    }
}

const KNOWN_FUNCTIONS: &str = "sqrt, abs, round, floor, ceil, min, max, pow, hypot, ln, log, exp, sin, cos, tan, asin, acos, atan, atan2";

fn call(name: &str, a: &[f64]) -> Result<f64, String> {
    let want = match name {
        "min" | "max" | "pow" | "hypot" | "atan2" => 2,
        "sqrt" | "abs" | "round" | "floor" | "ceil" | "ln" | "log" | "exp" | "sin" | "cos"
        | "tan" | "asin" | "acos" | "atan" => 1,
        other => return Err(format!("I do not know a function called \"{other}\" — {KNOWN} are what I have", KNOWN = KNOWN_FUNCTIONS)),
    };
    if a.len() != want {
        return Err(format!("{name}( ) takes {want} number(s), you gave {}", a.len()));
    }
    let one = |f: fn(f64) -> f64| Ok(f(a[0]));
    let two = |f: fn(f64, f64) -> f64| Ok(f(a[0], a[1]));
    match name {
        "sqrt" => {
            if a[0] < 0.0 {
                return Err("sqrt of a negative number".into());
            }
            one(f64::sqrt)
        }
        "abs" => one(f64::abs),
        "round" => one(f64::round),
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        "exp" => one(f64::exp),
        "ln" => one(f64::ln),
        "log" => one(f64::log10),
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "asin" => one(f64::asin),
        "acos" => one(f64::acos),
        "atan" => one(f64::atan),
        "min" => two(f64::min),
        "max" => two(f64::max),
        "pow" => two(f64::powf),
        "hypot" => two(f64::hypot),
        "atan2" => two(f64::atan2),
        _ => unreachable!("the arity match above is the only path here"),
    }
}


/// Six significant digits, no trailing zeros, and an integer printed as one — because
/// `0.375*25.4 = 9.525000000000001` is a true statement about binary floating point and a
/// useless answer to a person holding a ruler.
pub(crate) fn fmt(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{v:.0}");
    }
    let mag = v.abs().log10().floor() as i32;
    if !(-6..12).contains(&mag) {
        return format!("{v:e}");
    }
    let decimals = (5 - mag).max(0) as usize;
    let text = format!("{v:.decimals$}");
    let text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_precedence_is_the_one_a_calculator_uses() {
        assert_eq!(eval("2+3*4").unwrap(), 14.0);
        assert_eq!(eval("(2+3)*4").unwrap(), 20.0);
        assert_eq!(eval("2^3^2").unwrap(), 512.0, "powers are right associative");
        assert_eq!(eval("8/4/2").unwrap(), 1.0, "division is left associative");
        assert_eq!(eval("-2^2").unwrap(), -4.0, "unary minus binds looser than a power");
        assert_eq!(eval("(-2)^2").unwrap(), 4.0);
        assert_eq!(eval("2^-3").unwrap(), 0.125);
        assert!((eval("100/3/3").unwrap() - 11.1111111111).abs() < 1e-9);
    }
    #[test]
    fn a_workshop_expression_evaluates_as_the_calculator_on_the_bench_would() {
        assert!((eval("3/8*25.4").unwrap() - 9.525).abs() < 1e-9);
        assert_eq!(eval("230*2").unwrap(), 460.0);
        assert!((eval("1/(1/470+1/1000)").unwrap() - 319.7278911564626).abs() < 1e-6);
        assert!((eval("hypot(600,400)").unwrap() - 721.1102550927978).abs() < 1e-9);
        assert!((eval("100/80*10").unwrap() - 12.5).abs() < 1e-9, "percent of, by division");
    }
    #[test]
    fn percent_is_per_hundred_and_mod_is_the_remainder() {
        assert!((eval("12% * 340").unwrap() - 40.8).abs() < 1e-9);
        assert_eq!(eval("17 mod 5").unwrap(), 2.0);
        // `%` never means remainder, so the ambiguous reading cannot happen quietly: this is a
        // loud error rather than 3.5 or 1.
        assert!(eval("50 % 7").is_err(), "juxtaposed numbers must not implicitly multiply");
    }
    #[test]
    fn things_a_model_actually_types_still_parse() {
        assert_eq!(eval("1,000/8").unwrap(), 125.0, "thousands separator outside brackets");
        assert_eq!(eval("pow(2,10)").unwrap(), 1024.0, "comma inside brackets is an argument");
        assert_eq!(eval("2(3+4)").unwrap(), 14.0, "implicit multiplication after a bracket");
        assert_eq!(eval("2pi").unwrap(), (2.0 * std::f64::consts::PI));
        assert_eq!(eval("2 × 3").unwrap(), 6.0);
        assert_eq!(eval("6 ÷ 2").unwrap(), 3.0);
        assert_eq!(eval("2 ** 8").unwrap(), 256.0);
        assert_eq!(eval("1e3+1").unwrap(), 1001.0);
        assert_eq!(eval("2e+1").unwrap(), 20.0, "scientific notation with an explicit sign");
        assert_eq!(eval("e+1").unwrap(), (std::f64::consts::E + 1.0), "e alone is Euler's");
    }
    #[test]
    fn nonsense_is_refused_loudly_and_educates() {
        assert!(eval("2+").is_err());
        assert!(eval("(2+3").is_err());
        assert!(eval("sqrt(-1)").is_err());
        assert!(eval("1/0").is_err());
        assert!(eval("").is_err());
        // The error for a unit is the interesting one: it has to send the model to the tool
        // that can answer, rather than let it invent a number.
        let e = eval("230V*2A").unwrap_err();
        assert!(e.contains("230 volts × 2 amps is just 230*2"), "{e}");
        let e = eval("sqrt").unwrap_err();
        assert!(e.contains("sqrt, abs, round"), "{e}");
    }
    #[test]
    fn an_answer_prints_the_way_a_person_reads_one() {
        assert_eq!(fmt(9.525), "9.525");
        assert_eq!(fmt(460.0), "460");
        assert_eq!(fmt(0.125), "0.125");
        assert_eq!(fmt(1.0 / 3.0), "0.333333");
        assert_eq!(fmt(0.375 * 25.4), "9.525", "not 9.525000000000001");
        assert_eq!(fmt(1e13), "10000000000000");
        assert!(fmt(1e20).contains('e'));
    }
}
