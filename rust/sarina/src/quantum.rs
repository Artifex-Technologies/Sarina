//! Go: `go/quantum`: a small exact simulator of quantum circuits, so that when Sarina teaches quantum computing she can
//! check a claim ("this circuit gives 50/50") instead of trusting her memory. The whole state vector (complex amplitudes)
//! up to [`MAX_QUBITS`] qubits, gates applied one by one: plain arithmetic, nothing random unless shots are asked for, no
//! network, no files. Qubit order follows Qiskit: qubit 0 is the rightmost bit of a basis state.
//!
//! Byte for byte Go's: the gates' entries come from Go's own `math.Sin`, `Cos` and `Sincos` ([`crate::gomath`]), complex
//! products are Go's (`(ac - bd) + (ad + bc)i`, nothing fused), the shots are drawn by Go's `math/rand`
//! (testdata/parity/quantum.json; rust/parity/cli-differential.py's quantum cases).

use crate::goany::Any;
use crate::gotext::{equal_fold, fields_bytes, quote, quote_bytes, to_lower, to_lower_bytes, trim_space, trim_space_bytes};
use crate::tools::registry::{Output, Tool};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The most qubits (2^12 amplitudes): plenty for teaching, never a burden on the computer.
pub const MAX_QUBITS: i64 = 12;
/// The most gates in a circuit.
pub const MAX_GATES: usize = 500;

/// A complex128 with Go's arithmetic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct C(pub f64, pub f64);

impl C {
    fn mul(self, b: C) -> C {
        C(self.0 * b.0 - self.1 * b.1, self.0 * b.1 + self.1 * b.0)
    }
    fn add(self, b: C) -> C {
        C(self.0 + b.0, self.1 + b.1)
    }
    fn neg(self) -> C {
        C(-self.0, -self.1)
    }
}

/// Go's `cmplx.Exp` of a number whose real part is +0 (the only kind the gates take).
fn exp_i(im: f64) -> C {
    let r = crate::gomath::exp(0.0);
    let (s, c) = crate::gomath::sincos(im);
    C(r * c, r * s)
}

type Mat2 = [[C; 2]; 2];

const ZERO: C = C(0.0, 0.0);
const ONE: C = C(1.0, 0.0);

fn gate1(name: &str) -> Mat2 {
    let r2 = C(std::f64::consts::FRAC_1_SQRT_2, 0.0);
    let i = C(0.0, 1.0);
    let mi = C(0.0, -1.0);
    match name {
        "x" => [[ZERO, ONE], [ONE, ZERO]],
        "y" => [[ZERO, mi], [i, ZERO]],
        "z" => [[ONE, ZERO], [ZERO, C(-1.0, 0.0)]],
        "h" => [[r2, r2], [r2, r2.neg()]],
        "s" => [[ONE, ZERO], [ZERO, i]],
        "sdg" => [[ONE, ZERO], [ZERO, mi]],
        "t" => [[ONE, ZERO], [ZERO, exp_i(std::f64::consts::FRAC_PI_4)]],
        "tdg" => [[ONE, ZERO], [ZERO, exp_i(-std::f64::consts::FRAC_PI_4)]],
        _ => [[ONE, ZERO], [ZERO, ONE]], // "id"
    }
}

fn rot(name: &str, th: f64) -> Mat2 {
    let (c, s) = (C(crate::gomath::cos(th / 2.0), 0.0), C(crate::gomath::sin(th / 2.0), 0.0));
    let mi = C(0.0, -1.0);
    match name {
        "rx" => [[c, mi.mul(s)], [mi.mul(s), c]],
        "ry" => [[c, s.neg()], [s, c]],
        "rz" => [[exp_i(-th / 2.0), ZERO], [ZERO, exp_i(th / 2.0)]],
        _ => [[ONE, ZERO], [ZERO, exp_i(th)]], // "p": phase
    }
}

/// A register of n qubits.
pub struct State {
    pub n: i64,
    pub amp: Vec<C>,
}

/// One parsed instruction.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub gate: String,
    pub qubits: Vec<i64>,
    pub theta: f64,
}

fn arity(name: &str) -> Option<usize> {
    Some(match name {
        "x" | "y" | "z" | "h" | "s" | "sdg" | "t" | "tdg" | "id" | "rx" | "ry" | "rz" | "p" => 1,
        "cx" | "cnot" | "cy" | "cz" | "ch" | "swap" | "cp" => 2,
        "ccx" | "toffoli" | "cswap" => 3,
        _ => return None,
    })
}

impl State {
    /// Go's `New`: n qubits all in |0>.
    pub fn new(n: i64) -> Result<State, String> {
        if !(1..=MAX_QUBITS).contains(&n) {
            return Err(format!("between 1 and {MAX_QUBITS} qubits"));
        }
        let mut amp = vec![ZERO; 1 << n];
        amp[0] = ONE;
        Ok(State { n, amp })
    }

    /// A one-qubit gate on qubit q, only where every control qubit is 1.
    fn apply1(&mut self, m: &Mat2, q: i64, controls: &[i64]) {
        let bit = 1usize << q;
        let cmask = controls.iter().fold(0usize, |a, c| a | 1 << c);
        for i in 0..self.amp.len() {
            if i & bit != 0 || i & cmask != cmask {
                continue;
            }
            let (a0, a1) = (self.amp[i], self.amp[i | bit]);
            self.amp[i] = m[0][0].mul(a0).add(m[0][1].mul(a1));
            self.amp[i | bit] = m[1][0].mul(a0).add(m[1][1].mul(a1));
        }
    }

    fn swap(&mut self, a: i64, b: i64) {
        for i in 0..self.amp.len() {
            if i >> a & 1 == 1 && i >> b & 1 == 0 {
                let j = i & !(1 << a) | 1 << b;
                self.amp.swap(i, j);
            }
        }
    }

    /// Go's `Run`.
    pub fn run(&mut self, steps: &[Step]) {
        let x = gate1("x");
        for st in steps {
            let q = &st.qubits;
            match st.gate.as_str() {
                "rx" | "ry" | "rz" | "p" => self.apply1(&rot(&st.gate, st.theta), q[0], &[]),
                "cx" | "cnot" => self.apply1(&x, q[1], &[q[0]]),
                "cy" => self.apply1(&gate1("y"), q[1], &[q[0]]),
                "cz" => self.apply1(&gate1("z"), q[1], &[q[0]]),
                "ch" => self.apply1(&gate1("h"), q[1], &[q[0]]),
                "cp" => self.apply1(&rot("p", st.theta), q[1], &[q[0]]),
                "swap" => self.swap(q[0], q[1]),
                "ccx" | "toffoli" => self.apply1(&x, q[2], &[q[0], q[1]]),
                "cswap" => {
                    self.apply1(&x, q[1], &[q[0], q[2]]);
                    self.apply1(&x, q[2], &[q[0], q[1]]);
                    self.apply1(&x, q[1], &[q[0], q[2]]);
                }
                g => self.apply1(&gate1(g), q[0], &[]),
            }
        }
    }

    fn ket(&self, i: usize) -> String {
        format!("|{:0w$b}>", i, w = self.n as usize)
    }

    /// Go's `Report`: amplitudes, probabilities, each qubit's chance of reading 1, and with shots > 0 a sample of
    /// measurement outcomes drawn with Go's `math/rand` from `seed`.
    pub fn report(&self, shots: i64, seed: i64) -> String {
        let prob = |a: &C| a.0 * a.0 + a.1 * a.1;
        let mut rows: Vec<(usize, f64)> = self.amp.iter().enumerate().map(|(i, a)| (i, prob(a))).filter(|&(_, p)| p > 1e-12).collect();
        rows.sort_by(|x, y| {
            if x.1 > y.1 {
                std::cmp::Ordering::Less
            } else if y.1 > x.1 {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        let mut b = format!("{} qubit(s); qubit 0 is the rightmost bit. Final state, exact:\n", self.n);
        for (k, &(i, p)) in rows.iter().enumerate() {
            if k == 64 {
                b += &format!("... and {} more basis states\n", rows.len() - 64);
                break;
            }
            b += &format!("  {}  amplitude {}  probability {}\n", self.ket(i), cnum(self.amp[i]), num(p));
        }
        b += "Chance each qubit reads 1:";
        for q in 0..self.n {
            let mut p = 0.0;
            for (i, a) in self.amp.iter().enumerate() {
                if i >> q & 1 == 1 {
                    p += prob(a);
                }
            }
            b += &format!(" q{q}={}", num(p));
        }
        b += "\n";
        if shots > 0 {
            let shots = shots.min(100_000);
            let mut rng = crate::gorand::Rand::new(seed);
            let mut counts: BTreeMap<usize, i64> = BTreeMap::new();
            for _ in 0..shots {
                let (x, mut acc) = (rng.float64(), 0.0);
                for &(i, p) in &rows {
                    acc += p;
                    if x < acc {
                        *counts.entry(i).or_default() += 1;
                        break;
                    }
                }
            }
            b += &format!("Measured {shots} times (random, like a real device):");
            for &(i, _) in &rows {
                if let Some(c) = counts.get(&i).filter(|c| **c > 0) {
                    b += &format!(" {}:{c}", self.ket(i));
                }
            }
            b += "\n";
        }
        b
    }
}

/// Go's `strconv.FormatFloat(x, 'f', 4, 64)` after values within 5e-10 of zero are made zero.
pub fn num(x: f64) -> String {
    let x = if x.abs() < 5e-10 { 0.0 } else { x };
    if x.is_nan() {
        "NaN".into()
    } else if x.is_infinite() {
        if x > 0.0 { "+Inf" } else { "-Inf" }.into()
    } else {
        format!("{x:.4}")
    }
}

/// Go's `cnum`: a complex amplitude as the report writes it.
pub fn cnum(c: C) -> String {
    let (re, im) = (c.0, c.1);
    if im.abs() < 5e-10 {
        num(re)
    } else if re.abs() < 5e-10 {
        num(im) + "i"
    } else if im < 0.0 {
        format!("{} - {}i", num(re), num(-im))
    } else {
        format!("{} + {}i", num(re), num(im))
    }
}

/// Go's `angle`: "pi/4", "3*pi/2", "-pi", "0.5".
pub fn angle(s: &str) -> Result<f64, String> {
    let s = to_lower(trim_space(s)).replace(' ', "");
    let (sign, s) = match s.strip_prefix('-') {
        Some(r) => (-1.0, r.to_owned()),
        None => (1.0, s),
    };
    let (num_, den) = match s.find('/') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (&s[..], "1"),
    };
    let val = |x: &str| -> Result<f64, ()> {
        let (mut mul, mut x) = (1.0, x);
        if let Some(r) = x.strip_suffix("pi") {
            mul = std::f64::consts::PI;
            x = r.strip_suffix('*').unwrap_or(r);
            if x.is_empty() {
                return Ok(mul);
            }
        }
        crate::gotext::parse_float(x).map(|v| v * mul).map_err(|_| ())
    };
    let bad = || format!("angle {} not understood", quote(&s));
    let a = val(num_).map_err(|_| bad())?;
    let b = val(den).map_err(|_| bad())?;
    if b == 0.0 {
        return Err(bad());
    }
    Ok(sign * a / b)
}

/// Go's `Parse` (on bytes: a circuit from standard input need not be UTF-8): one instruction per line or separated by
/// ';', like "h 0", "cx 0 1", "rz(pi/2) 1", "ccx 0 1 2"; comments after '#', "measure", "measure all" and barriers skipped.
pub fn parse(text: &[u8], n: i64) -> Result<Vec<Step>, String> {
    let mut out = Vec::new();
    for raw in text.split(|&c| c == b';' || c == b'\n').filter(|r| !r.is_empty()) {
        let line = trim_space_bytes(raw.split(|&c| c == b'#').next().unwrap_or_default());
        if line.is_empty() || {
            let l = crate::gotext::lossy(line);
            equal_fold(&l, "measure") || equal_fold(&l, "measure all") || to_lower_bytes(line).starts_with("barrier")
        } {
            continue;
        }
        let ql = quote_bytes(line);
        let replaced = replace(line);
        let f = fields_bytes(&replaced);
        if f.is_empty() {
            continue; // only brackets or commas: no gate
        }
        let mut name = to_lower_bytes(f[0]);
        let mut theta = 0.0;
        if let Some(i) = name.find('(') {
            if !name.ends_with(')') {
                return Err(format!("{ql}: the angle needs a closing bracket"));
            }
            theta = angle(&name[i + 1..name.len() - 1]).map_err(|e| format!("{ql}: {e}"))?;
            name.truncate(i);
        } else if matches!(name.as_str(), "rx" | "ry" | "rz" | "p" | "cp") {
            return Err(format!("{ql}: {name} needs an angle, like {name}(pi/2) 0"));
        }
        let Some(k) = arity(&name) else {
            return Err(format!("unknown gate {} (known: x y z h s sdg t tdg rx ry rz p cx cy cz ch cp swap ccx cswap)", quote(&name)));
        };
        if f.len() - 1 != k {
            return Err(format!("{ql}: {name} acts on {k} qubit(s)"));
        }
        let mut qubits = Vec::new();
        for a in &f[1..] {
            let q = match crate::gotext::atoi(a) {
                Some(q) if q >= 0 && q < n => q,
                _ => return Err(format!("{ql}: qubit {} is not between 0 and {}", quote_bytes(a), n - 1)),
            };
            if qubits.contains(&q) {
                return Err(format!("{ql}: the same qubit twice"));
            }
            qubits.push(q);
        }
        out.push(Step { gate: name, qubits, theta });
        if out.len() > MAX_GATES {
            return Err(format!("at most {MAX_GATES} gates"));
        }
    }
    Ok(out)
}

/// Go's `strings.NewReplacer(",", " ", "q[", "", "[", " ", "]", "")`.
fn replace(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b',' | b'[' => out.push(b' '),
            b']' => {}
            b'q' if b.get(i + 1) == Some(&b'[') => i += 1,
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// Go's `Simulate`: parse, run, report.
pub fn simulate(qubits: i64, circuit: &[u8], shots: i64, seed: i64) -> Result<String, String> {
    let mut s = State::new(qubits)?;
    let steps = parse(circuit, qubits)?;
    s.run(&steps);
    Ok(s.report(shots, seed))
}

/// Go's conversion of a float64 to int on amd64 (CVTTSD2SQ): out of range or NaN gives the smallest int64.
fn go_int(x: f64) -> i64 {
    if x.is_nan() || x >= 9.223_372_036_854_776e18 || x < -9.223_372_036_854_776e18 {
        i64::MIN
    } else {
        x as i64
    }
}

fn now_nanos() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i64)
}

/// Go's `Tools`: quantum_sim when SARINA_QUANTUM=on (read when the registry is built).
pub fn tools() -> Vec<Tool> {
    if std::env::var("SARINA_QUANTUM").unwrap_or_default() != "on" {
        return Vec::new();
    }
    vec![crate::tools::defs::tool(
        "quantum_sim",
        Arc::new(|args, _| {
            let f = |k: &str| match args.get(k) {
                Some(Any::Number(v)) => *v,
                _ => 0.0,
            };
            let circuit = match args.get("circuit") {
                Some(Any::String(v)) => v.clone(),
                _ => String::new(),
            };
            let n = go_int(f("qubits"));
            match simulate(n, circuit.as_bytes(), go_int(f("shots")), now_nanos()) {
                Ok(out) => {
                    let md = BTreeMap::from([
                        ("action".to_string(), Any::String("quantum_sim".into())),
                        ("qubits".to_string(), Any::Number(n as f64)),
                    ]);
                    Output {
                        content: out + "\n[Exact simulation of an ideal, noise-free device. Real hardware adds noise.]",
                        metadata: md,
                        error: None,
                    }
                }
                Err(e) => crate::tools::files::fail(e),
            }
        }),
    )]
}

/// `sarina quantum [--shots N] QUBITS 'CIRCUIT'` (or the circuit on standard input, at most 64 KiB).
pub fn command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("quantum");
    fs.int("shots", 0, "also measure this many times");
    let rest = match fs.parse(args, &mut env.stderr) {
        Ok(rest) => rest,
        Err(_) => {
            env.flush();
            return 2;
        }
    };
    if rest.is_empty() {
        env.eprintln("usage: sarina quantum [--shots N] QUBITS 'CIRCUIT'");
        return 2;
    }
    let Some(n) = crate::gotext::sscan_int(&rest[0]) else {
        env.eprintln("sarina quantum: the first argument is the number of qubits");
        return 2;
    };
    let mut circuit = rest[1..].join(" ").into_bytes();
    if circuit.is_empty() {
        use std::io::Read;
        let _ = std::io::stdin().lock().take(64 << 10).read_to_end(&mut circuit);
    }
    match simulate(n, &circuit, fs.num("shots"), now_nanos()) {
        Ok(out) => {
            env.stdout.extend_from_slice(out.as_bytes());
            env.flush();
            0
        }
        Err(e) => {
            env.eprintln(&format!("sarina quantum: {e}"));
            2
        }
    }
}
