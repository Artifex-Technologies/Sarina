//! Go: `trachtenberg`: the Trachtenberg speed system worked exactly, step by step (the single-number rules for 3-9, 11
//! and 12, the two-finger method for any other), every result checked against the true product before it is shown; the
//! `trachtenberg` tool (SARINA_MATH=on, or auto: [`math_for_prompt`]), offered per question after every other tool.
//! Proved by the CLI differential's math cases.

use crate::gotext::{quote, trim_space};
use crate::tools::registry::{Output, Tool};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

/// One move of a worked example.
#[derive(Clone, Debug, Default)]
pub struct Step {
    pub position: i64,
    pub text: String,
    pub value: i64,
    pub write: i64,
    pub carry: i64,
}

/// A worked multiplication.
#[derive(Clone, Debug, Default)]
pub struct Work {
    pub a: String,
    pub b: String,
    pub rule: String,
    pub steps: Vec<Step>,
    pub result: String,
}

/// Go's `Rules`.
fn rule(m: i64) -> Option<&'static str> {
    Some(match m {
        11 => "Add each digit to its neighbour.",
        12 => "Double each digit and add its neighbour.",
        6 => "Add to each digit half its neighbour; add 5 more if the digit is odd.",
        7 => "Double each digit and add half its neighbour; add 5 more if the digit is odd.",
        5 => "Take half the neighbour; add 5 if the digit is odd.",
        9 => "Rightmost digit: subtract it from 10. Middle digits: subtract from 9 and add the neighbour. Leftmost (the zero in front): the neighbour minus 1.",
        8 => "Rightmost digit: subtract it from 10 and double. Middle digits: subtract from 9, double, and add the neighbour. Leftmost (the zero in front): the neighbour minus 2.",
        4 => "Rightmost digit: subtract it from 10, add 5 if it is odd. Middle digits: subtract from 9, add half the neighbour, add 5 if odd. Leftmost (the zero in front): half the neighbour minus 1.",
        3 => "Rightmost digit: subtract it from 10 and double, add 5 if it is odd. Middle digits: subtract from 9, double, add half the neighbour, add 5 if odd. Leftmost (the zero in front): half the neighbour minus 2.",
        _ => return None,
    })
}

/// Go's replacer of ",", "_" and " ".
fn strip(s: &str) -> String {
    s.chars().filter(|&c| c != ',' && c != '_' && c != ' ').collect()
}

fn digits(s: &str) -> Result<Vec<i64>, String> {
    let s = strip(trim_space(s));
    if s.is_empty() || s.len() > 40 {
        return Err("give a whole number of 1 to 40 digits".into());
    }
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        if !c.is_ascii_digit() {
            return Err(format!("{} is not a whole number", quote(&s)));
        }
        out.push((c as u8 - b'0') as i64);
    }
    Ok(out)
}

fn clean(s: &str) -> String {
    strip(trim_space(s)).trim_start_matches('0').to_string()
}

fn odd(x: i64) -> bool {
    x % 2 == 1
}

/// Go's integer division and remainder (towards zero), as Rust's `/` and `%` on i64.
fn half(n: i64) -> i64 {
    n / 2
}

/// Go's `ByRule`: a times one of the rule numbers, digit by digit.
pub fn by_rule(a: &str, m: i64) -> Result<Work, String> {
    let Some(r) = rule(m) else {
        return Err(format!("there is no single rule for {m} (rules: 3 4 5 6 7 8 9 11 12); use the general method"));
    };
    let d = digits(a)?;
    let mut dd = vec![0];
    dd.extend(d);
    let mut w = Work { a: clean(a), b: m.to_string(), rule: r.into(), ..Work::default() };
    let mut carry = 0i64;
    let mut out: Vec<i64> = Vec::new();
    let last = dd.len() - 1;
    for k in (0..=last).rev() {
        let x = dd[k];
        let n = if k < last { dd[k + 1] } else { 0 };
        let mut v;
        let mut how;
        match m {
            11 => {
                v = x + n;
                how = format!("{x} + neighbour {n}");
            }
            12 => {
                v = 2 * x + n;
                how = format!("double {x} = {}, + neighbour {n}", 2 * x);
            }
            5..=7 => {
                let mut base = x;
                how = x.to_string();
                if m == 7 {
                    base = 2 * x;
                    how = format!("double {x} = {}", 2 * x);
                }
                if m == 5 {
                    base = 0;
                    how = String::new();
                }
                v = base + half(n);
                if how.is_empty() {
                    how = format!("half of neighbour {n} = {}", half(n));
                } else {
                    how += &format!(", + half of neighbour {n} = {}", half(n));
                }
                if odd(x) {
                    v += 5;
                    how += &format!(", + 5 because {x} is odd");
                }
            }
            _ => {
                // 9, 8, 4, 3
                let double = m == 8 || m == 3;
                let halves = m == 4 || m == 3;
                if k == last {
                    v = 10 - x;
                    how = format!("10 - {x} = {}", 10 - x);
                } else if k == 0 {
                    let sub = match m {
                        9 | 4 => 1,
                        _ => 2,
                    };
                    if halves {
                        v = half(n) - sub;
                        how = format!("half of neighbour {n} = {}, - {sub}", half(n));
                    } else {
                        v = n - sub;
                        how = format!("neighbour {n} - {sub}");
                    }
                } else {
                    v = 9 - x;
                    how = format!("9 - {x} = {}", 9 - x);
                }
                if k != 0 {
                    if double {
                        v *= 2;
                        how += &format!(", doubled = {v}");
                    }
                    if k != last {
                        if halves {
                            v += half(n);
                            how += &format!(", + half of neighbour {n} = {}", half(n));
                        } else {
                            v += n;
                            how += &format!(", + neighbour {n}");
                        }
                    }
                    if (m == 4 || m == 3) && odd(x) {
                        v += 5;
                        how += &format!(", + 5 because {x} is odd");
                    }
                }
            }
        }
        let total = v + carry;
        if carry != 0 {
            how += &format!(", + carry {carry}");
        }
        let write = ((total % 10) + 10) % 10;
        let next = (total - write) / 10;
        let what = if k == 0 { "the zero in front".to_string() } else { format!("digit {x}") };
        let shown = if how.ends_with(&format!("= {total}")) { how.clone() } else { format!("{how} = {total}") };
        w.steps.push(Step {
            position: (last - k + 1) as i64,
            text: format!("{what}: {shown}: write {write}, carry {next}"),
            value: total,
            write,
            carry: next,
        });
        out.insert(0, write);
        carry = next;
    }
    while carry > 0 {
        out.insert(0, carry % 10);
        w.steps.push(Step {
            position: out.len() as i64,
            text: format!("the carry {} left over: write it in front", carry % 10),
            write: carry % 10,
            ..Step::default()
        });
        carry /= 10;
    }
    w.result = join(&out);
    check(&w)?;
    Ok(w)
}

/// Go's `General`: the two-finger method for any two numbers.
pub fn general(a: &str, b: &str) -> Result<Work, String> {
    let x = digits(a)?;
    let y = digits(b)?;
    let ax: Vec<i64> = x.into_iter().rev().collect();
    let by: Vec<i64> = y.into_iter().rev().collect();
    let mut w = Work { a: clean(a), b: clean(b), rule: "Each digit of the answer is the units digits of the pairs that land on that place, plus the tens digits of the pairs that land one place to the right, plus the carry.".into(), ..Work::default() };
    let pairs = |p: i64| -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for i in 0..ax.len() {
            let j = p - i as i64;
            if j >= 0 && (j as usize) < by.len() {
                out.push((i, j as usize));
            }
        }
        out
    };
    let mut carry = 0i64;
    let mut out: Vec<i64> = Vec::new();
    for p in 0..(ax.len() + by.len()) as i64 {
        let mut parts = Vec::new();
        let mut v = 0;
        for (i, j) in pairs(p) {
            let prod = ax[i] * by[j];
            v += prod % 10;
            parts.push(format!("units of {}×{} = {}", ax[i], by[j], prod % 10));
        }
        for (i, j) in pairs(p - 1) {
            let prod = ax[i] * by[j];
            v += prod / 10;
            parts.push(format!("tens of {}×{} = {}", ax[i], by[j], prod / 10));
        }
        if parts.is_empty() && carry == 0 {
            continue;
        }
        if carry != 0 {
            parts.push(format!("carry {carry}"));
        }
        let total = v + carry;
        w.steps.push(Step {
            position: p + 1,
            text: format!("place {}: {} = {total}: write {}, carry {}", p + 1, parts.join(" + "), total % 10, total / 10),
            value: total,
            write: total % 10,
            carry: total / 10,
        });
        out.insert(0, total % 10);
        carry = total / 10;
    }
    while carry > 0 {
        out.insert(0, carry % 10);
        carry /= 10;
    }
    w.result = join(&out);
    check(&w)?;
    Ok(w)
}

fn join(d: &[i64]) -> String {
    let s: String = d.iter().map(|&v| (b'0'.wrapping_add(v as u8)) as char).collect();
    let s = s.trim_start_matches('0');
    if s.is_empty() {
        "0".into()
    } else {
        s.into()
    }
}

/// The product of two decimal numbers (Go's `big.Int` Mul), as decimal text without leading zeros.
fn product(a: &str, b: &str) -> String {
    let a: Vec<u32> = a.bytes().rev().map(|c| (c - b'0') as u32).collect();
    let b: Vec<u32> = b.bytes().rev().map(|c| (c - b'0') as u32).collect();
    let mut r = vec![0u32; a.len() + b.len() + 1];
    for (i, &x) in a.iter().enumerate() {
        let mut carry = 0;
        for (j, &y) in b.iter().enumerate() {
            let t = r[i + j] + x * y + carry;
            r[i + j] = t % 10;
            carry = t / 10;
        }
        let mut k = i + b.len();
        while carry > 0 {
            let t = r[k] + carry;
            r[k] = t % 10;
            carry = t / 10;
            k += 1;
        }
    }
    let s: String = r.iter().rev().map(|&d| (b'0' + d as u8) as char).collect();
    let s = s.trim_start_matches('0');
    if s.is_empty() {
        "0".into()
    } else {
        s.into()
    }
}

fn or_zero(s: &str) -> &str {
    if s.is_empty() {
        "0"
    } else {
        s
    }
}

/// Go's `check`: a worked example whose answer is not the true product is refused.
fn check(w: &Work) -> Result<(), String> {
    let want = product(or_zero(&w.a), or_zero(&w.b));
    if want != w.result {
        return Err(format!("internal check failed: {} × {} worked out as {}, the true product is {want}", w.a, w.b, w.result));
    }
    Ok(())
}

/// Go's `DigitSum`: down to one digit, 9 as 0; -1 for what is not a number.
pub fn digit_sum(s: &str) -> i64 {
    let Ok(d) = digits(s) else { return -1 };
    let mut t: i64 = d.iter().sum();
    while t > 9 {
        let mut n = 0;
        while t > 0 {
            n += t % 10;
            t /= 10;
        }
        t = n;
    }
    if t == 9 {
        0
    } else {
        t
    }
}

/// Go's `Grade`: a learner's answer against the true product.
pub fn grade(w: &Work, answer: &str) -> String {
    let mut got = clean(answer);
    if got.is_empty() {
        got = "0".into();
    }
    if got == w.result {
        return format!("The answer {got} is right.");
    }
    let (g, r) = (got.as_bytes(), w.result.as_bytes());
    let mut place = 0;
    for i in 1..=g.len().max(r.len()) {
        let gc = if i <= g.len() { g[g.len() - i] } else { b'0' };
        let rc = if i <= r.len() { r[r.len() - i] } else { b'0' };
        if gc != rc {
            place = i;
            break;
        }
    }
    let mut nines = String::new();
    let (ca, cb, cg) = (digit_sum(&w.a), digit_sum(&w.b), digit_sum(&got));
    if ca >= 0 && cb >= 0 && cg >= 0 {
        let want = (ca * cb) % 9;
        nines = if want == cg {
            format!(" The nines check would not have caught it (digit sums {ca} × {cb} → {want}, and the answer's digit sum is also {cg}): it misses some slips, such as swapped digits.")
        } else {
            format!(" The nines check shows it too: digit sums {ca} × {cb} → {want}, but the answer's digit sum is {cg}.")
        };
    }
    format!("The answer {got} is not right: it is {}. The first wrong digit is place {place} from the right (the step for place {place}).{nines}", w.result)
}

/// Go's `Report`: the rule, each step, the answer, the nines check.
pub fn report(w: &Work) -> String {
    let mut b = format!("{} × {} by the Trachtenberg method.\nRule: {}\n", w.a, w.b, w.rule);
    if rule(atoi(&w.b)).is_some() && w.b.len() <= 2 {
        b += &format!(
            "Write {} with a zero in front (0{}) and work from right to left; the neighbour is the digit to the right.\n",
            w.a, w.a
        );
    }
    for (i, s) in w.steps.iter().enumerate() {
        b += &format!("Step {} ({})\n", i + 1, s.text);
    }
    let (ca, cb, cr) = (digit_sum(&w.a), digit_sum(&w.b), digit_sum(&w.result));
    b += &format!(
        "Answer: {}.\nNines check (add the digits until one is left; 9 counts as 0): {ca} × {cb} = {} → {}; the answer's digit sum is {cr}: it agrees.",
        w.result,
        ca * cb,
        (ca * cb) % 9
    );
    b
}

/// Go's `atoi`: a short whole number (-1 for anything else, and for more than nine digits, which would wrap around the
/// int and could read as a rule number: fixed in Go too).
fn atoi(s: &str) -> i64 {
    if s.len() > 9 {
        return -1;
    }
    let mut n: i64 = 0;
    for c in s.chars() {
        if !c.is_ascii_digit() {
            return -1;
        }
        n = n * 10 + (c as u8 - b'0') as i64;
    }
    n
}

/// Go's `WorkOut`: the single rule when b (or a) is a rule number, else the general method.
pub fn work_out(a: &str, b: &str) -> Result<Work, String> {
    let m = atoi(&clean(b));
    if m >= 0 && rule(m).is_some() {
        return by_rule(a, m);
    }
    let m = atoi(&clean(a));
    if m >= 0 && rule(m).is_some() {
        return by_rule(b, m);
    }
    general(a, b)
}

/// Go's `Tools`: the trachtenberg tool when SARINA_MATH=on (read when the registry is built).
pub fn tools() -> Vec<Tool> {
    if std::env::var("SARINA_MATH").unwrap_or_default() != "on" {
        return Vec::new();
    }
    vec![crate::tools::defs::tool(
        "trachtenberg",
        Arc::new(|args, _| {
            let s = |k: &str| match args.get(k) {
                Some(crate::goany::Any::String(v)) => v.clone(),
                _ => String::new(),
            };
            let w = match work_out(&s("a"), &s("b")) {
                Ok(w) => w,
                Err(e) => return crate::tools::files::fail(e),
            };
            let mut out = report(&w);
            let ans = s("answer");
            if !trim_space(&ans).is_empty() {
                out += &format!("\n{}", grade(&w, trim_space(&ans)));
            }
            out += "\n[Computed exactly. Teach from these steps; do not do the arithmetic yourself.]";
            let md = BTreeMap::from([("action".to_string(), crate::goany::Any::String("trachtenberg".into()))]);
            Output { content: out, metadata: md, error: None }
        }),
    )]
}

static MATH_ASK_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    crate::goregexp::compile(r"(?i)\btrachtenberg\b|\b(mental|speed) (math|arithmetic|multiplication)\b|\bmultipl(y|ying|ication)\b|\b\d[\d,]*\s*(x|×|\*|times)\s*\d").expect("mathAskRE")
});

static CRYPTO_ASK_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    crate::goregexp::compile(concat!(
        r"(?i)\b(base ?64|base ?32|hex(adecimal)?|hash(es|ed)?|sha-?\d+|md5|bcrypt|argon2|hmac|cipher(text)?|caesar|vigen[eè]re|rot ?13|xor|",
        r"encrypt\w*|decrypt\w*|encod\w*|decod\w*|crypto\w*|jwt|tokens?|certificate|x\.?509|pem|pgp|gpg|ssh key|public key|private key|",
        r"steg\w*|hidden (message|data|text|characters?)|lsb|exif|metadata|watermark\w*|invisible (characters?|text|marks?)|homoglyphs?|zero[- ]width|smuggl\w*)\b|[A-Za-z0-9+]{24,}={0,2}|\b[0-9a-fA-F]{32,}\b"
    ))
    .expect("cryptoAskRE")
});

/// Go's `cli.Main` deciding SARINA_CRYPTO=auto and SARINA_MATH=auto from the command line's words (every argument after
/// the command that starts with neither "-" nor "/"): "on" when the words ask for the set, else "off". What the process's
/// environment should hold for each (None: left as it is). The caller sets it before anything else runs, as Go does
/// with `os.Setenv`, so the commands' children see it too.
pub fn auto_decisions(getenv: &dyn Fn(&str) -> String, args: &[String]) -> Vec<(&'static str, &'static str)> {
    let words: Vec<&str> = args.iter().skip(1).filter(|a| !a.starts_with('-') && !a.starts_with('/')).map(String::as_str).collect();
    let prompt = words.join(" ");
    let mut out = Vec::new();
    for (k, re) in [("SARINA_CRYPTO", &CRYPTO_ASK_RE), ("SARINA_MATH", &MATH_ASK_RE)] {
        if getenv(k) == "auto" {
            out.push((k, if re.is_match(&prompt) { "on" } else { "off" }));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn products_are_exact() {
        assert_eq!(product("0", "0"), "0");
        assert_eq!(product("123", "456"), "56088");
        assert_eq!(product(&"9".repeat(40), &"9".repeat(40)), format!("{}8{}1", "9".repeat(39), "0".repeat(39)));
    }

    #[test]
    fn long_numbers_are_no_rule() {
        assert_eq!(atoi("1844674407370955161600000000000000000009"), -1);
        assert_eq!(atoi("123456789"), 123456789);
        assert_eq!(atoi("12"), 12);
        assert_eq!(atoi("1a"), -1);
        let w = work_out("12", "1844674407370955161600000000000000000009").unwrap();
        assert_eq!(w.result, "22136092888451461939200000000000000000108");
    }
}
