//! One speak request: the text cut into sentences, each made into a wav file and handed to the caller as soon as it is
//! ready (the `Voice` of labyrinth-voiced).
//!
//! **Pipelined** (the default): one thread writes the speech tokens of sentence N+1 while this one decodes sentence N, writes
//! its file and sends it, with at most one finished sentence waiting between them. The random draws stay in the order the
//! Python made them, so a seed gives the same speech either way: the writer draws for each sentence in turn, and when the
//! decoder finds invalid audio (the Python's ValueError, tried again up to three times), the writer is sent back to that
//! sentence with the generator as it stood right after that sentence's tokens, which is where the Python's retry drew from.
//! `LABYRINTH_VOICE_PIPELINE=0` makes the sentences one after the other, exactly as the Python did.

use crate::engine::{Config, Decoder, Engine, Fail, Voice as Conditioning, Writer};
use crate::npy::Generator;
use crate::text;
use sarina_voice::json::{self, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Condvar, Mutex};
use std::time::{Instant, SystemTime};

/// The service's settings (the Python's module constants, from the same environment variables).
#[derive(Clone, Debug)]
pub struct Settings {
    pub run: PathBuf,
    pub dir: PathBuf,
    pub dtype: String,
    pub idle: f64,
    pub reference: String,
    pub venv: PathBuf,
    pub pipeline: bool,
    /// a fixed seed for every request (tests only: LABYRINTH_VOICE_SEED); None draws one from the system, as the Python's
    /// random.getrandbits(32) did
    pub seed: Option<u128>,
    /// where the pronunciation table is read from (~/.config/labyrinth/pronunciations)
    pub pronunciations: String,
}

/// Tests only: make the decoder report invalid audio for these (sentence, attempt) pairs, to exercise the retries.
pub type FailPlan = Vec<(usize, usize)>;

pub struct Service {
    pub settings: Settings,
    /// the Python's voice.lock: held while loading, speaking or unloading
    pub lock: Mutex<Option<Engine>>,
    pub loaded: AtomicBool,
    /// the last time a request started or finished (seconds since the epoch)
    pub last: Mutex<f64>,
    counter: AtomicU64,
    pub fail_plan: FailPlan,
}

pub fn now() -> f64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn system_seed() -> u128 {
    let mut b = [0u8; 4];
    let n = unsafe { libc::getrandom(b.as_mut_ptr().cast(), 4, 0) };
    if n == 4 {
        u32::from_le_bytes(b) as u128
    } else {
        // no getrandom: the clock's nanoseconds (never reached on Linux 3.17 and later)
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.subsec_nanos() as u128).unwrap_or(0)
    }
}

/// What the caller is told about one sentence.
pub struct Said {
    pub wav: String,
    pub text: String,
    pub before: i64,
    pub after: i64,
}

impl Said {
    pub fn json(&self) -> String {
        json::dumps(&json::obj(vec![
            ("wav", json::s(&self.wav)),
            ("text", json::s(&self.text)),
            ("pause_before_ms", Value::Int(self.before.to_string())),
            ("pause_after_ms", Value::Int(self.after.to_string())),
        ]))
    }
}

struct Sentence {
    plain: String,
    name: String,
}

impl Service {
    pub fn new(settings: Settings) -> Service {
        Service {
            settings,
            lock: Mutex::new(None),
            loaded: AtomicBool::new(false),
            last: Mutex::new(now()),
            counter: AtomicU64::new(0),
            fail_plan: Vec::new(),
        }
    }

    pub fn touch(&self) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = now();
    }

    /// Loads the voice if it is not loaded: the graphics card first, the processor (full precision) if the card cannot be
    /// used; then the voice to copy. Only a voice that loaded completely is kept.
    pub fn load(&self, slot: &mut Option<Engine>) -> Result<(), String> {
        if slot.is_some() {
            return Ok(());
        }
        let s = &self.settings;
        let cfg = Config { dir: s.dir.clone(), dtype: s.dtype.clone(), gpu: true, venv: s.venv.clone() };
        let mut engine = match Engine::load(&cfg) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("voiced: graphics card not usable ({}); using the processor", e.chars().take(120).collect::<String>());
                let dtype = if s.dtype == "fp16" { "fp32".to_string() } else { s.dtype.clone() };
                Engine::load(&Config { dtype, gpu: false, ..cfg })?
            }
        };
        engine.set_voice(&s.reference)?;
        eprintln!("voiced: voice ready in {:.1}s ({})", engine.load_seconds, engine.providers.join(","));
        *slot = Some(engine);
        self.loaded.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn unload(&self) {
        let mut slot = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        *slot = None;
        self.loaded.store(false, Ordering::SeqCst);
    }

    fn next_path(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        self.settings.run.join(format!("voiced-{}-{}.wav", std::process::id(), n)).to_string_lossy().into_owned()
    }

    /// Writes the sentence's file and tells the caller. Ok(false): the caller went away (the file is removed again).
    fn deliver(&self, wav: &[f32], s: &Sentence, send: &mut dyn FnMut(&str) -> bool) -> Result<bool, String> {
        let (_, before, after) = text::delivery(&s.name);
        let path = self.next_path();
        crate::wav::write_pcm16(&path, wav).map_err(|e| format!("{}: {:?}", e, path))?;
        let said = Said { wav: path.clone(), text: s.plain.clone(), before, after };
        if !send(&said.json()) {
            let _ = std::fs::remove_file(&path);
            return Ok(false);
        }
        Ok(true)
    }

    /// Says the text: one message per sentence through `send` (false = the caller is gone: stop). Err: the message the
    /// caller gets as {"error": ...}.
    pub fn speak(&self, text_in: &str, send: &mut dyn FnMut(&str) -> bool) -> Result<(), String> {
        let seed = self.settings.seed.unwrap_or_else(system_seed);
        let mut rng = Generator::new(seed);
        let mut slot = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.touch();
        self.load(&mut slot)?;
        let engine = slot.as_mut().unwrap();
        let table = text::pronunciations(&self.settings.pronunciations)?;
        let sentences: Vec<Sentence> = text::sentences(text_in, &table).into_iter().map(|(name, plain)| Sentence { plain, name }).collect();
        let voice = engine.voice.clone().ok_or("set_voice first")?;
        let r = if self.settings.pipeline && sentences.len() > 1 {
            self.speak_pipelined(&mut engine.writer, &mut engine.decoder, &voice, &sentences, &mut rng, send)
        } else {
            self.speak_in_turn(&mut engine.writer, &mut engine.decoder, &voice, &sentences, &mut rng, send)
        };
        self.touch();
        r
    }

    fn planned_failure(&self, i: usize, attempt: usize) -> bool {
        self.fail_plan.contains(&(i, attempt))
    }

    fn decode(&self, dec: &mut Decoder, voice: &Conditioning, tokens: &[i64], i: usize, attempt: usize) -> Result<Vec<f32>, Fail> {
        let wav = dec.decode(voice, tokens)?;
        if self.planned_failure(i, attempt) {
            return Err(Fail::Retry("the voice produced invalid audio".into()));
        }
        Ok(wav)
    }

    /// The sentences one after the other, as the Python did.
    fn speak_in_turn(
        &self,
        writer: &mut Writer,
        dec: &mut Decoder,
        voice: &Conditioning,
        sentences: &[Sentence],
        rng: &mut Generator,
        send: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), String> {
        for (i, s) in sentences.iter().enumerate() {
            let (temperature, _, _) = text::delivery(&s.name);
            let spoken = text::dress(&s.plain, &s.name, rng);
            let mut attempt = 0;
            let wav = loop {
                let r = writer.write(voice, &spoken, rng, temperature, &|| false).and_then(|t| self.decode(dec, voice, &t, i, attempt));
                match r {
                    Ok(w) => break w,
                    Err(Fail::Retry(m)) => {
                        if attempt == 2 {
                            return Err(m);
                        }
                        eprintln!("voiced: invalid audio for a sentence, trying again");
                        attempt += 1;
                    }
                    Err(Fail::Fatal(m)) => return Err(m),
                }
            };
            if !self.deliver(&wav, s, send)? {
                return Ok(());
            }
        }
        Ok(())
    }

    /// The writer thread makes tokens ahead while this thread decodes, writes and sends.
    fn speak_pipelined(
        &self,
        writer: &mut Writer,
        dec: &mut Decoder,
        voice: &Conditioning,
        sentences: &[Sentence],
        rng: &mut Generator,
        send: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), String> {
        let ctl = Ctl { state: Mutex::new(CtlState::default()), cv: Condvar::new() };
        let (tx, rx) = mpsc::sync_channel::<Made>(1);
        let start_rng = rng.clone();
        std::thread::scope(|scope| {
            let ctl = &ctl;
            scope.spawn(move || write_ahead(writer, voice, sentences, start_rng, ctl, tx));
            let result = self.decode_in_order(dec, voice, sentences, ctl, &rx, send);
            {
                let mut st = ctl.state.lock().unwrap_or_else(|e| e.into_inner());
                st.finished = true;
            }
            ctl.cv.notify_all();
            drop(rx); // a writer blocked on a full channel gets an error and ends
            result
        })
    }

    fn decode_in_order(
        &self,
        dec: &mut Decoder,
        voice: &Conditioning,
        sentences: &[Sentence],
        ctl: &Ctl,
        rx: &mpsc::Receiver<Made>,
        send: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), String> {
        let mut epoch = 0u64;
        let mut next = 0usize;
        while next < sentences.len() {
            let made = rx.recv().map_err(|_| "the voice stopped".to_string())?;
            if made.epoch != epoch {
                continue; // made before a retry was asked for: thrown away
            }
            let tokens = match made.result {
                Ok(t) => t,
                Err(m) => return Err(m),
            };
            debug_assert_eq!(made.index, next);
            match self.decode(dec, voice, &tokens, made.index, made.attempt) {
                Ok(wav) => {
                    if !self.deliver(&wav, &sentences[made.index], send)? {
                        return Ok(());
                    }
                    next += 1;
                }
                Err(Fail::Retry(m)) => {
                    if made.attempt == 2 {
                        return Err(m);
                    }
                    eprintln!("voiced: invalid audio for a sentence, trying again");
                    epoch += 1;
                    let mut st = ctl.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.epoch = epoch;
                    st.restart = Some(Restart { index: made.index, attempt: made.attempt + 1, rng: made.rng_after, spoken: made.spoken });
                    drop(st);
                    ctl.cv.notify_all();
                }
                Err(Fail::Fatal(m)) => return Err(m),
            }
        }
        Ok(())
    }
}

/// A sentence's tokens from the writer (or why there are none).
struct Made {
    epoch: u64,
    index: usize,
    attempt: usize,
    spoken: String,
    result: Result<Vec<i64>, String>,
    /// the generator right after these tokens: where a retry of this sentence draws from
    rng_after: Generator,
}

struct Restart {
    index: usize,
    attempt: usize,
    rng: Generator,
    spoken: String,
}

#[derive(Default)]
struct CtlState {
    epoch: u64,
    restart: Option<Restart>,
    finished: bool,
}

struct Ctl {
    state: Mutex<CtlState>,
    cv: Condvar,
}

/// The writer's side: the sentences in order, with their retries; after the last one it waits, since the decoder may still
/// send it back to a sentence.
fn write_ahead(
    writer: &mut Writer,
    voice: &Conditioning,
    sentences: &[Sentence],
    mut rng: Generator,
    ctl: &Ctl,
    tx: mpsc::SyncSender<Made>,
) {
    let mut epoch = 0u64;
    let mut i = 0usize;
    let mut attempt = 0usize;
    let mut spoken: Option<String> = None;
    loop {
        // a retry asked for, or the end?
        {
            let mut st = ctl.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if st.finished {
                    return;
                }
                if st.epoch != epoch {
                    let r = st.restart.take().expect("a new epoch comes with a restart");
                    epoch = st.epoch;
                    i = r.index;
                    attempt = r.attempt;
                    rng = r.rng;
                    spoken = Some(r.spoken);
                }
                if i < sentences.len() {
                    break;
                }
                st = ctl.cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        }
        let s = &sentences[i];
        let (temperature, _, _) = text::delivery(&s.name);
        let words = spoken.take().unwrap_or_else(|| text::dress(&s.plain, &s.name, &mut rng));
        let stop = || {
            let st = ctl.state.lock().unwrap_or_else(|e| e.into_inner());
            st.finished || st.epoch != epoch
        };
        let result = writer.write(voice, &words, &mut rng, temperature, &stop);
        let made = match result {
            Ok(tokens) => Made { epoch, index: i, attempt, spoken: words, result: Ok(tokens), rng_after: rng.clone() },
            Err(Fail::Retry(m)) => {
                if attempt < 2 {
                    eprintln!("voiced: invalid audio for a sentence, trying again");
                    attempt += 1;
                    spoken = Some(words);
                    continue;
                }
                Made { epoch, index: i, attempt, spoken: words, result: Err(m), rng_after: rng.clone() }
            }
            Err(Fail::Fatal(m)) => {
                if stop() {
                    continue; // abandoned for a retry or the end: the loop above sorts out which
                }
                Made { epoch, index: i, attempt, spoken: words, result: Err(m), rng_after: rng.clone() }
            }
        };
        let failed = made.result.is_err();
        if tx.send(made).is_err() {
            return;
        }
        if failed {
            // the decoder ends the request on it; wait for that (or for a retry from an earlier sentence)
            let mut st = ctl.state.lock().unwrap_or_else(|e| e.into_inner());
            while !st.finished && st.epoch == epoch {
                st = ctl.cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            continue;
        }
        i += 1;
        attempt = 0;
    }
}

/// How long one thing took, for the log and the measurements.
pub fn elapsed_ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
