//! sarina-voice: one binary for Sarina's spoken-question programs, chosen by the name it is started as (a link named
//! labyrinth-ask-sarina, labyrinth-sarina-checkup or labyrinth-sarina-speak-voiced), or by the first argument:
//! `sarina-voice ask-sarina ...`, `sarina-voice checkup ...`, `sarina-voice speak-voiced ...`.

use std::ffi::OsString;

fn main() {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    let argv0 = args.first().map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
    let mut name = argv0.rsplit('/').next().unwrap_or("").to_string();
    let mut prog = sarina_voice::program(&name);
    if prog.is_none() && args.len() > 1 {
        name = args[1].to_string_lossy().into_owned();
        prog = sarina_voice::program(&name);
        if prog.is_some() {
            args.remove(1);
            // the program's own name, as Python's argparse would have shown it
            name = match prog {
                Some("ask") => "labyrinth-ask-sarina".into(),
                Some("checkup") => "labyrinth-sarina-checkup".into(),
                _ => "labyrinth-sarina-speak-voiced".into(),
            };
        }
    }
    let rest: Vec<String> = args[1.min(args.len())..].iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let code = match prog {
        Some("ask") => sarina_voice::ask::main(&name, &rest),
        Some("checkup") => sarina_voice::checkup::main(&rest),
        Some("speak-voiced") => sarina_voice::speak_voiced::main(&args[1.min(args.len())..]),
        _ => {
            eprintln!("usage: sarina-voice ask-sarina|checkup|speak-voiced ARGS... (or start it as labyrinth-ask-sarina, labyrinth-sarina-checkup, labyrinth-sarina-speak-voiced)");
            2
        }
    };
    std::process::exit(code);
}
