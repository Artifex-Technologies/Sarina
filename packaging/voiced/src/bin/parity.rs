//! sarina-voiced-parity: drives the library for the side-by-side checks in tests/ (never installed).
//!
//!   numpy exp IN OUT | numpy expall (stdin) | numpy sum FILE | numpy rng FILE    NumPy's numbers, compared
//!   text            JSON lines on stdin, one answer per line (the text functions)
//!   engine CASES OUTDIR       {"text","seed","temperature"} per line: tokens, timings and OUTDIR/N.wav + N.f32
//!   pcm16 IN.f32 OUT.wav      a sentence's file as the service writes it
//!   load WAV OUT.f32          the reference recording as the engine reads it (raw float32)
//!   request TEXTFILE SEED     a whole speak request through the service (LABYRINTH_VOICE_* as the service reads them),
//!                             the messages printed with the time each arrived; FAIL="i:a,i:a" makes decodes fail
//!
//! Environment: VENV (the venv whose libonnxruntime is used), DIR (the Chatterbox folder), DTYPE (default fp32).

use sarina_voice::json::{self, Value};
use sarina_voiced::engine::{Config, Engine};
use sarina_voiced::npy::{self, Generator};
use sarina_voiced::{text, voice};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::time::Instant;

fn f32s(path: &str) -> Vec<f32> {
    let mut b = Vec::new();
    std::fs::File::open(path).unwrap().read_to_end(&mut b).unwrap();
    b.chunks(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn hwm_kb() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn cpu_s() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let t = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
    t(ru.ru_utime) + t(ru.ru_stime)
}

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| d.to_string())
}

fn home(p: &str) -> String {
    p.replacen('~', &std::env::var("HOME").unwrap_or_default(), 1)
}

fn numpy(a: &[String]) {
    match a[0].as_str() {
        "exp" => {
            let (x, y) = (f32s(&a[1]), f32s(&a[2]));
            let bad = x.iter().zip(&y).filter(|(xi, yi)| npy::exp_f32(**xi).to_bits() != yi.to_bits()).count();
            println!("exp: {} of {} differ", bad, x.len());
        }
        "expall" => {
            // NumPy's exp of every float32 from -0.0 down to -104 (bit patterns 0x80000000..0xC2D00000), read from stdin
            let mut inp = std::io::BufReader::with_capacity(1 << 24, std::io::stdin());
            let mut buf = vec![0u8; 1 << 24];
            let (mut bits, end, mut bad) = (0x8000_0000u32, 0xC2D0_0000u32, 0u64);
            while bits < end {
                let n = (((end - bits) as usize) * 4).min(buf.len());
                inp.read_exact(&mut buf[..n]).unwrap();
                for c in buf[..n].chunks(4) {
                    if npy::exp_f32(f32::from_bits(bits)).to_bits() != u32::from_le_bytes([c[0], c[1], c[2], c[3]]) {
                        bad += 1;
                    }
                    bits += 1;
                }
            }
            println!("expall: {} of {} differ", bad, end - 0x8000_0000);
        }
        "sum" => {
            let v = f32s(&a[1]);
            let (mut i, mut bad, mut cnt) = (0, 0, 0);
            while i < v.len() {
                let n = v[i].to_bits() as usize;
                i += 1;
                if npy::sum_f32(&v[i..i + n]).to_bits() != v[i + n].to_bits() {
                    bad += 1;
                }
                i += n + 1;
                cnt += 1;
            }
            println!("sum: {} of {} differ", bad, cnt);
        }
        "rng" => {
            let text = std::fs::read_to_string(&a[1]).unwrap();
            let mut g = Generator::new(0);
            let (mut bad, mut cnt) = (0, 0);
            for line in text.lines() {
                let w: Vec<&str> = line.split_whitespace().collect();
                let got = match w[0] {
                    "seed" => {
                        g = Generator::new(w[1].parse().unwrap());
                        continue;
                    }
                    "random" => format!("{:e}", g.random()),
                    "below" => format!("{}", g.below(w[1].parse().unwrap())),
                    "u64" => format!("{}", g.next_u64()),
                    "choice3" => g.choice(&["Hmm... ", "Um... ", "Well... "]).trim().to_string(),
                    "choicep" => {
                        let p: Vec<f32> = w[1].split(',').map(|s| f32::from_bits(s.parse::<u32>().unwrap())).collect();
                        format!("{}", g.choice_p(&p).unwrap())
                    }
                    "sample" => {
                        let l: Vec<f32> = w[2].split(',').map(|s| f32::from_bits(s.parse::<u32>().unwrap())).collect();
                        format!("{}", sarina_voiced::engine::sample(&l, &mut g, w[1].parse().unwrap()).unwrap())
                    }
                    _ => panic!("{}", w[0]),
                };
                let want = w.last().unwrap();
                let want = if w[0] == "random" { format!("{:e}", want.parse::<f64>().unwrap()) } else { want.to_string() };
                cnt += 1;
                if got != want {
                    bad += 1;
                    if bad < 5 {
                        println!("differs: {} -> {} (NumPy {})", &line[..line.len().min(60)], got, want);
                    }
                }
            }
            println!("rng: {} of {} differ", bad, cnt);
        }
        _ => panic!("numpy what?"),
    }
}

fn strs(v: &[String]) -> Value {
    Value::Arr(v.iter().map(|s| json::s(s)).collect())
}

fn text_cmd() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        let req = json::loads(&line).unwrap();
        let t = req.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let table: Vec<(String, String)> = match req.get("table") {
            Some(Value::Arr(a)) => a
                .iter()
                .map(|p| match p {
                    Value::Arr(kv) => (kv[0].as_str().unwrap().to_string(), kv[1].as_str().unwrap().to_string()),
                    _ => panic!(),
                })
                .collect(),
            _ => Vec::new(),
        };
        let r = match req.get("fn").and_then(|v| v.as_str()).unwrap() {
            "mark" => json::s(&text::mark_delivery(&t)),
            "take" => {
                let (n, p) = text::take_delivery(&t);
                Value::Arr(vec![json::s(&n), json::s(&p)])
            }
            "clean" => json::s(&text::clean(&t, &table)),
            "respell" => json::s(&text::respell(&t, &table)),
            "split" => strs(&text::split_sentences(&t)),
            "sentences" => Value::Arr(text::sentences(&t, &table).iter().map(|(n, p)| Value::Arr(vec![json::s(n), json::s(p)])).collect()),
            "dress" => {
                let seed: u128 = req.get("seed").unwrap().py_str().parse().unwrap();
                let mut g = Generator::new(seed);
                let name = req.get("name").and_then(|v| v.as_str()).unwrap();
                let a = text::dress(&t, name, &mut g);
                let b = text::dress(&t, name, &mut g);
                Value::Arr(vec![json::s(&a), json::s(&b), json::s(&g.random().to_bits().to_string())])
            }
            "pron" => {
                let p = req.get("path").and_then(|v| v.as_str()).unwrap();
                match text::pronunciations(p) {
                    Ok(t) => Value::Arr(t.iter().map(|(k, v)| Value::Arr(vec![json::s(k), json::s(v)])).collect()),
                    Err(_) => json::s("error"),
                }
            }
            "tok" => {
                use std::sync::OnceLock;
                static TOK: OnceLock<tokenizers::Tokenizer> = OnceLock::new();
                let tok = TOK.get_or_init(|| {
                    tokenizers::Tokenizer::from_file(
                        PathBuf::from(home(&env_or("DIR", "~/.local/share/labyrinth/tts/chatterbox"))).join("tokenizer.json"),
                    )
                    .unwrap()
                });
                let ids = tok.encode(t.as_str(), false).unwrap().get_ids().to_vec();
                Value::Arr(ids.iter().map(|i| Value::Int(i.to_string())).collect())
            }
            other => panic!("fn {}", other),
        };
        writeln!(out, "{}", json::dumps(&r)).unwrap();
    }
}

fn cfg() -> Config {
    Config {
        dir: PathBuf::from(home(&env_or("DIR", "~/.local/share/labyrinth/tts/chatterbox"))),
        dtype: env_or("DTYPE", "fp32"),
        gpu: false,
        venv: PathBuf::from(home(&env_or("VENV", "~/.local/share/labyrinth/tts/cb-venv"))),
    }
}

fn engine_cmd(cases: &str, outdir: &str) {
    let c = cfg();
    let t0 = Instant::now();
    let mut e = Engine::load(&c).unwrap();
    let reference = PathBuf::from(&c.dir).join("voice").join("sarina.wav");
    e.set_voice(&env_or("REFERENCE", reference.to_str().unwrap())).unwrap();
    let ready_ms = voice::elapsed_ms(t0);
    println!(
        "{}",
        json::dumps(&json::obj(vec![
            ("ready_ms", Value::Float(ready_ms)),
            ("rss_kb", Value::Int(hwm_kb().to_string())),
            ("lib", json::s(&sarina_voiced::engine::loaded_library().map(|p| p.display().to_string()).unwrap_or_default())),
        ]))
    );
    let voice = e.voice.clone().unwrap();
    for (i, line) in std::fs::read_to_string(cases).unwrap().lines().enumerate() {
        let case = json::loads(line).unwrap();
        let t = case.get("text").and_then(|v| v.as_str()).unwrap().to_string();
        let seed: u128 = case.get("seed").unwrap().py_str().parse().unwrap();
        let temp = match case.get("temperature") {
            Some(Value::Float(f)) => *f,
            Some(Value::Int(s)) => s.parse().unwrap(),
            _ => 0.8,
        };
        let mut g = Generator::new(seed);
        let (c0, t1) = (cpu_s(), Instant::now());
        let tokens = e.writer.write(&voice, &t, &mut g, temp, &|| false);
        let write_ms = voice::elapsed_ms(t1);
        let t2 = Instant::now();
        let r = tokens.clone().and_then(|tk| e.decoder.decode(&voice, &tk));
        let decode_ms = voice::elapsed_ms(t2);
        let cpu_ms = (cpu_s() - c0) * 1000.0;
        let mut fields = vec![
            ("i", Value::Int(i.to_string())),
            ("write_ms", Value::Float(write_ms)),
            ("decode_ms", Value::Float(decode_ms)),
            ("cpu_ms", Value::Float(cpu_ms)),
            ("rss_kb", Value::Int(hwm_kb().to_string())),
        ];
        match (&tokens, &r) {
            (Ok(tk), Ok(wav)) => {
                let path = format!("{}/{}.wav", outdir, i);
                sarina_voiced::wav::write_pcm16(&path, wav).unwrap();
                let raw: Vec<u8> = wav.iter().flat_map(|x| x.to_le_bytes()).collect();
                std::fs::write(format!("{}/{}.f32", outdir, i), raw).unwrap();
                fields.push(("tokens", Value::Arr(tk.iter().map(|t| Value::Int(t.to_string())).collect())));
                fields.push(("samples", Value::Int(wav.len().to_string())));
            }
            (Err(f), _) | (_, Err(f)) => fields.push(("error", json::s(f.message()))),
        }
        println!("{}", json::dumps(&json::obj(fields)));
        std::io::stdout().flush().unwrap();
    }
}

fn request_cmd(textfile: &str, seed: u128) {
    let text = std::fs::read_to_string(textfile).unwrap();
    let c = cfg();
    let run = PathBuf::from(env_or("RUN", "/tmp"));
    let settings = voice::Settings {
        run,
        dir: c.dir.clone(),
        dtype: env_or("SERVICE_DTYPE", "fp16"),
        idle: 600.0,
        reference: c.dir.join("voice").join("sarina.wav").to_string_lossy().into_owned(),
        venv: c.venv.clone(),
        pipeline: env_or("PIPELINE", "1") != "0",
        seed: Some(seed),
        pronunciations: env_or("PRONUNCIATIONS", "/nonexistent"),
    };
    let mut svc = voice::Service::new(settings);
    if let Ok(plan) = std::env::var("FAIL") {
        svc.fail_plan = plan
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|p| {
                let (a, b) = p.split_once(':').unwrap();
                (a.parse().unwrap(), b.parse().unwrap())
            })
            .collect();
    }
    // load first (as a warm request would), then time the request alone
    {
        let mut slot = svc.lock.lock().unwrap();
        svc.load(&mut slot).unwrap();
    }
    let t0 = Instant::now();
    let mut send = |line: &str| {
        println!("{:.1}\t{}", voice::elapsed_ms(t0), line);
        std::io::stdout().flush().unwrap();
        true
    };
    match svc.speak(&text, &mut send) {
        Ok(()) => println!("{:.1}\t{{\"done\": true}}", voice::elapsed_ms(t0)),
        Err(e) => println!("{:.1}\t{{\"error\": {}}}", voice::elapsed_ms(t0), json::dumps(&json::s(&e.chars().take(200).collect::<String>()))),
    }
    println!("rss_kb\t{}", hwm_kb());
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(|s| s.as_str()) {
        Some("numpy") => numpy(&a[1..]),
        Some("text") => text_cmd(),
        Some("engine") => engine_cmd(&a[1], &a[2]),
        Some("request") => request_cmd(&a[1], a[2].parse().unwrap()),
        Some("ortseed") => println!("{:?}", sarina_voiced::engine::set_graph_seed(&cfg().venv, 42)),
        Some("gpuprobe") => {
            // The CUDA path up to the point where a device is needed, with no device visible: the provider library, CUDA and
            // cuDNN must load, and the error must be "no device". Refuses to run with a device visible.
            if std::env::var("CUDA_VISIBLE_DEVICES").map(|v| !v.is_empty()).unwrap_or(true) {
                eprintln!("gpuprobe runs only with CUDA_VISIBLE_DEVICES= (empty)");
                std::process::exit(2);
            }
            let c = Config { gpu: true, dtype: "fp16".into(), venv: PathBuf::from(home(&env_or("VENV", "~/.local/share/labyrinth/tts/gpu-venv"))), ..cfg() };
            match Engine::load(&c) {
                Ok(_) => println!("loaded (unexpected with no device)"),
                Err(e) => println!("{}", e),
            }
            println!("library: {:?}", sarina_voiced::engine::loaded_library());
        }
        Some("pcm16") => sarina_voiced::wav::write_pcm16(&a[2], &f32s(&a[1])).unwrap(),
        Some("load") => {
            // the reference recording as the engine reads it, as raw float32
            let v = sarina_voiced::wav::load_audio(&a[1]).unwrap();
            std::fs::write(&a[2], v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
        }
        _ => {
            eprintln!("usage: see the source");
            std::process::exit(2);
        }
    }
}
