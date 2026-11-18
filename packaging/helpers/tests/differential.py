#!/usr/bin/env python3
"""Sarina's ML and voice helper: the Rust build against the Python package, side by side (docs/RUST_PORT.md
"The ML and voice helper"). Standard library only; it runs the Python helper under the interpreters that have its extras.

    cargo build --features parity                       # in packaging/helpers
    python3 -I packaging/helpers/tests/differential.py [--only serve|memory|tts|voice|stt]

What is compared, and how exactly:

- serve: the same request lines to both servers; every answer line byte for byte when the Rust build loads the very
  ONNX Runtime the Python's onnxruntime package ships (its capi/libonnxruntime.so.*), then again with the system's
  onnxruntime, where vectors and scores may differ in their last bits (tolerance below) and everything else is exact.
- memory: the command on the same session folders: stdout, stderr and the exit status byte for byte (same ONNX Runtime).
- tts: piper's phonemes for each line of a corpus (Python's PiperVoice.phonemize against the Rust port), and the helper's
  WAV for sentences, byte for byte, with the voice's noise turned off (a copy of its config with noise_scale and noise_w 0;
  with noise both draw unseeded random numbers, so no two runs of either agree).
- voice: voice-chat's whole loop with stand-ins for arecord, aplay, the agent and the speech recogniser (a stub
  faster_whisper module for the Python, a stand-in whisper-cli for the Rust), the real Piper voice: stdout, the exit
  status, every command line the agent got and every WAV played, byte for byte.
- stt: the real whisper.cpp on speech the Rust Piper made (Rust only: the Python's faster-whisper is not installed and
  needs a CTranslate2 model there is none of).

Nothing here records the microphone or plays sound; no model is downloaded (the stand-in embedding models are made here
or linked from files already on the computer).
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HOME = Path.home()
REPO = Path(__file__).resolve().parents[3]
HELPERS_PY = REPO / 'python' / 'sarina_helpers'
TTS_DIR = HOME / '.local/share/labyrinth/tts'
VOICE = TTS_DIR / 'voices/en_US-amy-medium.onnx'
STT_MODEL = HOME / '.local/share/labyrinth/stt/ggml-base.en.bin'
CHATTERBOX = TTS_DIR / 'chatterbox'

#: Vectors and scores from two ONNX Runtime builds: unit vectors and cosines, so every value is in [-1, 1]; float32 has
#: 24 bits (6e-8 relative), a build may order a float32 sum differently in each MatMul and LayerNorm (a few ulps each),
#: so a few-layer model ends within about 1e-7; 1e-6 leaves room and is still far below anything a label, a threshold
#: or a three-place score could notice. With the same build the answers are byte for byte (checked first).
TOLERANCE = 1e-6

failures: list[str] = []
counts: dict[str, int] = {}


def check(group: str, name: str, ok: bool, detail: str = '') -> None:
    counts[group] = counts.get(group, 0) + 1
    if not ok:
        failures.append(f'{group}: {name}: {detail}')
        print(f'  DIFF {group}: {name}\n{detail}', file=sys.stderr)


def ort_lib(python: str) -> str:
    out = subprocess.run([python, '-I', '-c', 'import onnxruntime, os; print(os.path.dirname(onnxruntime.__file__))'],
                         capture_output=True, text=True, check=True).stdout.strip()
    libs = sorted(glob.glob(os.path.join(out, 'capi', 'libonnxruntime.so.*')))
    if not libs:
        sys.exit(f'no libonnxruntime.so.* beside {python}\'s onnxruntime')
    return libs[-1]


def py_helper(python: str, args: list[str], extra_path: list[str] = (), **kw) -> subprocess.CompletedProcess:
    env = dict(kw.pop('env', os.environ))
    env['PYTHONPATH'] = os.pathsep.join([*extra_path, str(HELPERS_PY)])
    return subprocess.run([python, '-P', '-m', 'sarina_helpers', *args], env=env, capture_output=True, **kw)


def rs_helper(rust: str, args: list[str], ort: str | None, **kw) -> subprocess.CompletedProcess:
    env = dict(kw.pop('env', os.environ))
    if ort:
        env['ORT_DYLIB_PATH'] = ort
    else:
        env.pop('ORT_DYLIB_PATH', None)
    return subprocess.run([rust, *args], env=env, capture_output=True, **kw)


# ---------------------------------------------------------------------------------------------------------------------
# stand-in embedding models (no sentence-embedding model is on this computer)

def make_models(work: Path, ml_python: str) -> dict[str, Path]:
    models = {}
    bert = work / 'models' / 'bert'
    bert.mkdir(parents=True, exist_ok=True)
    subprocess.run([ml_python, '-I', str(Path(__file__).with_name('make_standin_model.py')), str(bert)], check=True,
                   capture_output=True)
    models['bert'] = bert
    if (CHATTERBOX / 'onnx' / 'embed_tokens.onnx').is_file():
        gpt = work / 'models' / 'gpt'
        gpt.mkdir(parents=True, exist_ok=True)
        for name, src in (('model.onnx', CHATTERBOX / 'onnx/embed_tokens.onnx'),
                          ('embed_tokens.onnx_data', CHATTERBOX / 'onnx/embed_tokens.onnx_data'),
                          ('tokenizer.json', CHATTERBOX / 'tokenizer.json')):
            if not (gpt / name).exists():
                (gpt / name).symlink_to(src)
        models['gpt'] = gpt
    broken = work / 'models' / 'broken'
    broken.mkdir(parents=True, exist_ok=True)
    (broken / 'model.onnx').write_bytes(b'not a model')
    (broken / 'tokenizer.json').write_text('{}')
    notok = work / 'models' / 'notokenizer'
    notok.mkdir(parents=True, exist_ok=True)
    shutil.copy(bert / 'model.onnx', notok / 'model.onnx')
    badtok = work / 'models' / 'badtokenizer'
    badtok.mkdir(parents=True, exist_ok=True)
    shutil.copy(bert / 'model.onnx', badtok / 'model.onnx')
    (badtok / 'tokenizer.json').write_text('{"version": 1')
    return models


# ---------------------------------------------------------------------------------------------------------------------
# serve

LONG = ' '.join(['network scanning and packet capture'] * 400)
CAPS = [
    {'name': 'gate.sub', 'type': 'SubGhz', 'meta': json.dumps({'protocol': 'Princeton', 'frequency': 433920000, 'preset': 'AM650'})},
    {'name': 'gate2.sub', 'type': 'SubGhz', 'meta': json.dumps({'protocol': 'Princeton', 'frequency': 433920000, 'preset': 'AM650'})},
    {'name': 'card.nfc', 'type': 'NFC', 'meta': json.dumps({'deviceType': 'Mifare Classic', 'uid': '04 A2 1B'})},
    {'name': 'card-copy.nfc', 'type': 'NFC', 'meta': json.dumps({'deviceType': 'Mifare Classic', 'uid': '04 A2 1B'})},
    {'name': 'tv.ir', 'type': 'Infrared', 'meta': '{not json'},
    {'name': 'remote.ir', 'type': 'Infrared', 'meta': '[1, 2]'},
    {'name': 7, 'type': None, 'meta': 5},
    {'type': 'RFID', 'meta': json.dumps({'uid': [1, 2.5, None, True, {'k': 'v'}], 'frequency': 1.5e-7})},
    'not a dict', 3, None,
]


def serve_requests() -> list[str]:
    r = [
        '{"op":"ping","id":7}', '{"op":"ping"}', '', '   ', '\x1c{"op":"ping","id":"spaces"}\x1c',
        json.dumps({'op': 'embed', 'texts': ['hello world', 'Sarina opens the vault', '', 'ünïcödé — 🦊 text', LONG], 'id': 1}),
        '{"op":"embed","texts":[]}', '{"op":"embed","texts":"x"}', '{"op":"embed","texts":["a",1]}', '{"op":"embed"}',
        '{"op":"classify","text":"nmap: network exploration tool and security port scanner"}',
        '{"op":"classify","text":"GNU Image Manipulation Program"}', '{"op":"classify","text":"  \\t "}',
        '{"op":"classify","text":""}', '{"op":"classify","text":5}', json.dumps({'op': 'classify', 'text': LONG}),
        json.dumps({'op': 'classify', 'text': 'a spreadsheet, a word processor and presentations', 'id': [1, {'x': None}]}),
        json.dumps({'op': 'group', 'captures': CAPS, 'threshold': 0.9, 'id': 'g'}),
        json.dumps({'op': 'group', 'captures': CAPS}), json.dumps({'op': 'group', 'captures': CAPS, 'threshold': 0}),
        json.dumps({'op': 'group', 'captures': CAPS, 'threshold': True}),
        json.dumps({'op': 'group', 'captures': CAPS[:2], 'threshold': -1}),
        '{"op":"group","captures":[],"threshold":0.5}', '{"op":"group","captures":[1,2],"threshold":0.5}',
        '{"op":"group","captures":{},"threshold":0.5}', '{"op":"group","captures":[],"threshold":"0.5"}',
        '{"op":"group","captures":[],"threshold":null}', '{"op":"group","captures":[],"threshold":' + '9' * 400 + '}',
        '{"op":"group","captures":[{"name":"a","meta":"' + '[' * 30000 + '"}],"threshold":0.5}',
        '{"op":"group","captures":[{"name":"' + '[' * 3000 + '"}],"threshold":0.5}',
        '{"op":"nope","id":1}', '{"id":2}', '{"op":null}', '{"op":5}', '{"op":["x"]}', '{"op":{"a":"b"}}',
        '[]', '"text"', '1', 'null', 'true',
        'nope', '{', '[', '{"a":1,}', '[1,]', '{"a" 1}', '{1:2}', '{"a":}', '"abc', '"a\\x"', '"\\u12"', '"\\ud800\\u12g4"',
        '"\x01"', '{"a":1}x', '01', '-', '﻿{"op":"ping"}', '9' * 4301,
        '{"op":"ping","id":[1.5e-7,1e400,-1e400,NaN,-0.0,0,-0,123456789012345678901234567890,1e22,1e16,1e-5,0.1]}',
        '{"op":"ping","id":"\\ud83e\\udd8a \\u00e9 \\" \\\\ \\n"}', '{"op":"ping","id":"dup","id":"last"}',
        '{"op":"ping","x":' + '[' * 30000 + ']' * 30000 + '}', '{"op":"ping","x":' + '[' * 25000 + '}',
    ]
    return r


def run_server(cmd: list[str], env: dict, lines: list[str]) -> tuple[bytes, int]:
    data = ''.join(line + '\n' for line in lines).encode()
    p = subprocess.run(cmd, input=data, env=env, capture_output=True, timeout=600)
    return p.stdout, p.returncode


def close(a, b, path='') -> str:
    """'' when a and b are the same JSON but for floats within TOLERANCE, else where they differ."""
    if isinstance(a, float) and isinstance(b, float):
        return '' if a == b or abs(a - b) <= TOLERANCE or (a != a and b != b) else f'{path}: {a!r} vs {b!r}'
    if type(a) is not type(b):
        return f'{path}: {a!r} vs {b!r}'
    if isinstance(a, list):
        if len(a) != len(b):
            return f'{path}: length {len(a)} vs {len(b)}'
        for i, (x, y) in enumerate(zip(a, b)):
            d = close(x, y, f'{path}[{i}]')
            if d:
                return d
        return ''
    if isinstance(a, dict):
        if list(a) != list(b):
            return f'{path}: keys {list(a)} vs {list(b)}'
        for k in a:
            d = close(a[k], b[k], f'{path}.{k}')
            if d:
                return d
        return ''
    return '' if a == b else f'{path}: {a!r} vs {b!r}'


def max_diff(a, b) -> float:
    if isinstance(a, float) and isinstance(b, float):
        return 0.0 if a == b or a != a else abs(a - b)
    if isinstance(a, list) and isinstance(b, list):
        return max([max_diff(x, y) for x, y in zip(a, b)] or [0.0])
    if isinstance(a, dict) and isinstance(b, dict):
        return max([max_diff(a[k], b.get(k)) for k in a] or [0.0])
    return 0.0


def test_serve(rust: str, ml_python: str, models: dict[str, Path], same_ort: str) -> None:
    print('serve')
    reqs = serve_requests()
    py_env = dict(os.environ, PYTHONPATH=str(HELPERS_PY))
    for name in ('bert', 'gpt'):
        if name not in models:
            continue
        mdir = str(models[name])
        py_out, py_rc = run_server([ml_python, '-P', '-m', 'sarina_helpers', 'serve', '--model-dir', mdir], py_env, reqs)
        rs_out, rs_rc = run_server([rust, 'serve', '--model-dir', mdir], dict(os.environ, ORT_DYLIB_PATH=same_ort), reqs)
        pl, rl = py_out.decode().splitlines(), rs_out.decode().splitlines()
        check('serve', f'{name}: exit status', py_rc == rs_rc, f'{py_rc} vs {rs_rc}')
        check('serve', f'{name}: answer count', len(pl) == len(rl), f'{len(pl)} vs {len(rl)}')
        for req, a, b in zip([r for r in reqs if r.strip(' \x1c')], pl, rl):
            check('serve', f'{name} same ONNX Runtime: {req[:70]!r}', a == b, f'  py {a[:300]}\n  rs {b[:300]}')
        # the system's onnxruntime: vectors and scores within TOLERANCE, all else exact
        sys_out, sys_rc = run_server([rust, 'serve', '--model-dir', mdir], {k: v for k, v in os.environ.items() if k != 'ORT_DYLIB_PATH'}, reqs)
        worst = 0.0
        for req, a, b in zip([r for r in reqs if r.strip(' \x1c')], pl, sys_out.decode().splitlines()):
            ja, jb = json.loads(a), json.loads(b)
            d = close(ja, jb)
            worst = max(worst, max_diff(ja, jb))
            check('serve', f'{name} system ONNX Runtime: {req[:70]!r}', not d, d)
        print(f'  {name}: {len(pl)} answers; system onnxruntime: largest difference {worst:.3g}')
    # load failures: one error line, exit 1
    for label, mdir in (('missing', '/nonexistent/model//dir/'), ('empty', ''), ('no tokenizer', models['bert'].parent / 'notokenizer'),
                        ('not a model', models['bert'].parent / 'broken'), ('bad tokenizer', models['bert'].parent / 'badtokenizer')):
        py_out, py_rc = run_server([ml_python, '-P', '-m', 'sarina_helpers', 'serve', '--model-dir', str(mdir)], py_env, ['{"op":"ping"}'])
        rs_out, rs_rc = run_server([rust, 'serve', '--model-dir', str(mdir)], dict(os.environ, ORT_DYLIB_PATH=same_ort), ['{"op":"ping"}'])
        check('serve', f'load failure {label}', (py_out, py_rc) == (rs_out, rs_rc), f'  py {py_out!r} {py_rc}\n  rs {rs_out!r} {rs_rc}')
    # the command line
    for args in (['serve'], ['serve', '-h'], ['serve', '--model'], ['serve', '--model-dir', 'x', 'extra'], ['serve', '--bogus']):
        py = py_helper(ml_python, args)
        rs = rs_helper(rust, args, same_ort)
        check('serve', f'arguments {args}', (py.stdout, py.stderr, py.returncode) == (rs.stdout, rs.stderr, rs.returncode),
              f'  py {py.stdout!r} {py.stderr!r} {py.returncode}\n  rs {rs.stdout!r} {rs.stderr!r} {rs.returncode}')
    # nesting past both limits (Python's is its C stack): both refuse the line and go on, each in its own words
    deep = ['{"op":"ping","x":' + '[' * 200000 + ']' * 200000 + '}', '{"op":"ping","id":1}']
    py_out, _ = run_server([ml_python, '-P', '-m', 'sarina_helpers', 'serve', '--model-dir', str(models['bert'])], py_env, deep)
    rs_out, _ = run_server([rust, 'serve', '--model-dir', str(models['bert'])], dict(os.environ, ORT_DYLIB_PATH=same_ort), deep)
    pl, rl = py_out.decode().splitlines(), rs_out.decode().splitlines()
    check('serve', 'nesting 200,000 deep: refused by both, then the next request answered',
          len(pl) == len(rl) == 2 and '"ok":false' in pl[0] and '"ok":false' in rl[0] and pl[1] == rl[1], f'  py {pl}\n  rs {rl}')
    # where the Python died, the Rust answers: a line that is not UTF-8
    p = subprocess.run([rust, 'serve', '--model-dir', str(models['bert'])], input=b'{"op":"ping"}\n\xff\xfe\n{"op":"ping","id":2}\n',
                       env=dict(os.environ, ORT_DYLIB_PATH=same_ort), capture_output=True, timeout=120)
    lines = p.stdout.decode().splitlines()
    check('serve', 'Rust only: a line that is not UTF-8 is answered, the server goes on',
          p.returncode == 0 and len(lines) == 3 and 'invalid start byte' in lines[1] and lines[2].endswith('"id":2}'), repr(p.stdout))


# ---------------------------------------------------------------------------------------------------------------------
# memory-search

def session(messages, sid=None) -> str:
    d = {'messages': messages}
    if sid is not None:
        d['session_id'] = sid
    return json.dumps(d)


def make_sessions(root: Path) -> Path:
    s = root / '.port_sessions' / 'agent'
    s.mkdir(parents=True)
    files = {
        'a1.json': session([{'role': 'system', 'content': 'x'}, {'role': 'user', 'content': 'Open the vault and show the daily note'},
                            {'role': 'assistant', 'content': 'Here is the daily note for today.'}]),
        'b2.json': session([{'role': 'user', 'content': '<system-reminder>ctx</system-reminder>  '},
                            {'role': 'user', 'content': 'scan my network for open ports <system-reminder>\nmore\n</system-reminder> please'},
                            {'role': 'assistant', 'content': 'Found three open ports.'}, {'role': 'assistant', 'content': ''},
                            {'role': 'assistant', 'content': 7}], sid='net-scan'),
        'c3.json': session([{'role': 'user', 'content': 'write a poem about wolves ' * 60}, {'role': 'assistant', 'content': 'Wolves ' * 300}]),
        'dup.json': session([{'role': 'user', 'content': 'duplicate id, later file'}], sid='net-scan'),
        'empty.json': session([]),
        'ids.json': session([{'role': 'assistant', 'content': 'only an outcome'}], sid=''),
        'odd.json': session(['x', 1, None, {'role': 'user'}, {'role': 'user', 'content': 'ünïcödé 🦊 question'}]),
        '.hidden.json': session([{'role': 'user', 'content': 'a hidden session about cooking pasta'}]),
        'bom.json': '﻿' + session([{'role': 'user', 'content': 'bom'}]),
        'bad.json': '{"messages": [',
        'list.json': '[1, 2]',
        'nomsg.json': '{"messages": "no"}',
        'notes.txt': session([{'role': 'user', 'content': 'not a json name'}]),
        'deep.json': session([{'role': 'user', 'content': 'deep'}])[:-1] + ', "x": ' + '[' * 3000 + ']' * 3000 + '}',
    }
    for i, (name, text) in enumerate(files.items()):
        p = s / name
        p.write_text(text, encoding='utf-8')
        os.utime(p, ns=(1_700_000_000_000_000_000 + i * 1_000_000_000,) * 2)
    os.utime(s / 'dup.json', ns=(1_700_000_000_000_000_000,) * 2)  # the same time as a1.json: the folder's order decides
    (s / 'latin1.json').write_bytes(session([{'role': 'user', 'content': 'café'}]).encode('latin-1'))
    (s / 'dir.json').mkdir()
    return s


def test_memory(rust: str, ml_python: str, models: dict[str, Path], same_ort: str, work: Path) -> None:
    print('memory-search')
    root = work / 'mem'
    sessions = make_sessions(root)
    empty = work / 'mem-empty'
    empty.mkdir()
    m = str(models['bert'])
    cases = [
        ['--model-dir', m, '--session-dir', str(sessions), 'open', 'the', 'vault'],
        ['--model-dir', m, '--session-dir', str(sessions), '--top-k', '2', 'network', 'ports'],
        ['--model-dir', m, '--session-dir', str(sessions), '--top-k', '0', 'x'],
        ['--model-dir', m, '--session-dir', str(sessions), '--top-k', '-2', 'wolves'],
        ['--model-dir', m, '--session-dir', str(sessions), '--top-k=1_0', 'wolves'],
        ['--model-dir', m, '--session-dir', str(sessions), '--top', '1', 'poem'],
        ['--model-dir', m, '--cwd', str(root), 'cooking'],
        ['--model-dir', m, '--cwd', str(root), '--session-dir', '', 'cooking'],
        ['--model-dir', m, '--session-dir', str(empty), 'anything'],
        ['--model-dir', m, '--session-dir', str(work / 'nowhere'), 'anything'],
        ['--model-dir', m, '--session-dir', str(sessions), '--', '--looks-like-an-option'],
        ['--model-dir', m, '--session-dir', str(sessions), '-5', 'apples'],
        ['--model-dir', m, '--session-dir', str(sessions), 'has space', '--top-k', '1'],
        ['--model-dir', m, 'q', '--session-dir', str(sessions), 'extra'],
        ['--model-dir', str(models['bert'].parent / 'notokenizer'), 'q'],
        ['--model-dir', str(work / 'no-model'), 'q'],
        ['--model-dir', str(models['bert'].parent / 'broken'), 'q'],
        [], ['q'], ['--model-dir', m], ['-h'], ['--help'], ['-hx'], ['--model-dir'], ['--top-k', 'x', '--model-dir', m, 'q'],
        ['--model-dir', m, '--bogus', '1', 'q'], ['--model-dir=' + m, '--top-k', '1.5', 'q'], ['--model-dir', m, '--s', str(sessions), 'q'],
        ['--model-dir', m, '--session-dir', str(sessions), '--top-k', ' 3 ', 'vault'], ['--model-dir', m, '--help=x', 'q'],
    ]
    if 'gpt' in models:
        cases.append(['--model-dir', str(models['gpt']), '--session-dir', str(sessions), 'the', 'daily', 'note'])
    for env_extra in ({}, {'COLUMNS': '40'}):
        for args in cases if not env_extra else [[], ['-h']]:
            env = dict(os.environ, **env_extra)
            py = py_helper(ml_python, ['memory-search', *args], env=env, timeout=300)
            rs = rs_helper(rust, ['memory-search', *args], same_ort, env=env, timeout=300)
            name = ' '.join(a if len(a) < 60 else a[:20] + '...' for a in args) + (f' {env_extra}' if env_extra else '')
            if py.stderr.startswith(b'Traceback'):
                # an exception the Python did not catch: its traceback; the Rust prints the exception's text alone
                last = py.stderr.decode().strip().splitlines()[-1]
                check('memory', name + ' (traceback: its message)', (py.stdout, py.returncode) == (rs.stdout, rs.returncode)
                      and last.endswith(rs.stderr.decode().strip()), f'  py {py.stderr[-300:]!r}\n  rs {rs.stderr!r}')
                continue
            check('memory', name, (py.stdout, py.stderr, py.returncode) == (rs.stdout, rs.stderr, rs.returncode),
                  f'  py {py.returncode} {py.stdout[:600]!r} {py.stderr[-300:]!r}\n  rs {rs.returncode} {rs.stdout[:600]!r} {rs.stderr[-300:]!r}')
    # hostile folders where the Python stopped or hung: the Rust skips the entry (Rust only)
    hostile = work / 'mem-hostile'
    hostile.mkdir()
    shutil.copy(sessions / 'a1.json', hostile / 'a1.json')
    os.mkfifo(hostile / 'pipe.json')
    (hostile / 'dangling.json').symlink_to(hostile / 'nowhere')
    rs = rs_helper(rust, ['memory-search', '--model-dir', m, '--session-dir', str(hostile), 'vault'], same_ort, timeout=60)
    check('memory', 'Rust only: a named pipe and a dangling link are skipped', rs.returncode == 0 and rs.stdout.count(b'task:') == 1,
          repr((rs.returncode, rs.stdout, rs.stderr)))


# ---------------------------------------------------------------------------------------------------------------------
# text to speech

CORPUS = [
    'Hello world.', 'Hello, world. How are you?', "Wow! That's great; really: yes.", 'No final mark',
    'He said "hi." Then left.', 'Mr. Smith paid $3.50 for 2 apples.', 'Wait... what?', 'Item one\n\nItem two',
    'A dash - and an em dash — here.', "It's 10:30 now, isn't it?", 'Use e.g. this, i.e. that.', '(Parenthetical.) Next sentence!',
    'Numbers: 1, 2, 3.', 'Ends with a quote?"', 'Sarina, open the vault.', 'The URL is example.com/path.',
    'Tabs\tand  spaces .  Next', 'Question?! Exclamation!?', "'Single quotes.' Done", 'Ellipsis… unicode',
    'Three open ports; see it, the end, carry on, very easy.', 'Say [[ hɛloʊ ]] to the [[wɝld]] now.', '[[ raw ]]',
    '', '   ', '...', 'Bonjour, ça va? Ünïcödé.', 'Emoji 🦊 and symbols © ® ™.',
]


def doc_corpus(limit: int) -> list[str]:
    out = []
    for f in sorted(glob.glob(str(REPO / 'docs' / '*.md'))) + [str(REPO / 'README.md')]:
        for line in open(f, encoding='utf-8'):
            line = line.strip()
            if not line or line.startswith(('|', '```', '#', '-', '<', '>')) or '`' in line or 'http' in line:
                continue
            line = line.replace('*', '').replace('_', '').replace('[', '').replace(']', '')
            if 20 < len(line) < 400:
                out.append(line)
    return out[:limit]


PY_PHONEMES = r'''
import json, sys
from piper import PiperVoice
v = PiperVoice.load(sys.argv[1])
for line in sys.stdin:
    print(json.dumps(v.phonemize(json.loads(line)), ensure_ascii=False))
'''

PY_SPEAK = r'''
import sys
sys.path.insert(0, sys.argv[1])
from sarina_helpers.tts import TextToSpeech
t = TextToSpeech(sys.argv[2])
open(sys.argv[3], 'wb').write(t.synthesize(sys.argv[4]))
'''


def quiet_voice(work: Path) -> Path:
    d = work / 'quiet'
    d.mkdir(exist_ok=True)
    model = d / VOICE.name
    if not model.exists():
        model.symlink_to(VOICE)
    cfg = json.loads(Path(str(VOICE) + '.json').read_text())
    cfg['inference']['noise_scale'] = 0.0
    cfg['inference']['noise_w'] = 0.0
    Path(str(model) + '.json').write_text(json.dumps(cfg))
    return model


def test_tts(parity: str, voice_python: str, work: Path, same_ort: str) -> None:
    print('tts')
    model = quiet_voice(work)
    texts = CORPUS + doc_corpus(1200)
    feed = ''.join(json.dumps(t) + '\n' for t in texts)
    py = subprocess.run([voice_python, '-I', '-c', PY_PHONEMES, str(model)], input=feed, capture_output=True, text=True, check=True)
    rs = subprocess.run([parity, 'phonemes-lines', str(model)], input=feed, capture_output=True, text=True,
                        env=dict(os.environ, ORT_DYLIB_PATH=same_ort), check=True)
    pl, rl = py.stdout.splitlines(), rs.stdout.splitlines()
    same = sum(1 for a, b in zip(pl, rl) if json.loads(a) == json.loads(b))
    only_linking = 0
    for t, a, b in zip(texts, pl, rl):
        a, b = json.loads(a), json.loads(b)
        if a == b:
            continue
        if [[p for p in s if p != 'ʲ'] for s in a] == [[p for p in s if p != 'ʲ'] for s in b]:
            only_linking += 1
        elif t in CORPUS:
            check('tts', f'phonemes {t!r}', False, f'  py {a}\n  rs {b}')
    print(f'  phonemes: {same} of {len(pl)} texts identical; {only_linking} differ only by a linking ʲ; '
          f'{len(pl) - same - only_linking} otherwise (espeak-ng builds: piper 1.8 has its own)')
    check('tts', 'phonemes: corpus lines answered', len(pl) == len(rl) == len(texts), f'{len(pl)} {len(rl)} {len(texts)}')
    check('tts', 'phonemes: at least 98 % identical', same >= 0.98 * len(pl), f'{same} of {len(pl)}')
    # the WAV, byte for byte, for texts whose phonemes agree
    for i, t in enumerate(['Hello there.', 'The scan found two open ports; one is expected, one is not! Shall I look closer?',
                           'Sarina, open the vault. Then read me the daily note, please.', 'One sentence without a stop',
                           '...', 'Say [[ hɛloʊ ]] now.']):
        a, b = work / f'py-{i}.wav', work / f'rs-{i}.wav'
        p = subprocess.run([voice_python, '-I', '-c', PY_SPEAK, str(HELPERS_PY), str(model), str(a), t], capture_output=True, text=True)
        r = subprocess.run([parity, 'speak', str(model), str(b), t], capture_output=True, text=True, env=dict(os.environ, ORT_DYLIB_PATH=same_ort))
        same_bytes = a.exists() and b.exists() and a.read_bytes() == b.read_bytes()
        check('tts', f'WAV {t!r}', (p.returncode == 0) == (r.returncode == 0) and (p.returncode != 0 or same_bytes),
              f'  py {p.returncode} {p.stderr[-300:]}\n  rs {r.returncode} {r.stderr[-300:]}')
    # a missing phoneme: piper's warning on stderr (a voice whose map lacks "ð")
    cfg = json.loads(Path(str(model) + '.json').read_text())
    del cfg['phoneme_id_map']['ð']
    lacking = work / 'lacking.onnx'
    if not lacking.exists():
        lacking.symlink_to(VOICE)
    Path(str(lacking) + '.json').write_text(json.dumps(cfg))
    a, b = work / 'py-lack.wav', work / 'rs-lack.wav'
    p = subprocess.run([voice_python, '-I', '-c', PY_SPEAK, str(HELPERS_PY), str(lacking), str(a), 'The other one.'], capture_output=True, text=True)
    r = subprocess.run([parity, 'speak', str(lacking), str(b), 'The other one.'], capture_output=True, text=True, env=dict(os.environ, ORT_DYLIB_PATH=same_ort))
    check('tts', 'a phoneme the voice lacks', a.read_bytes() == b.read_bytes() and p.stderr == r.stderr, f'  py {p.stderr!r}\n  rs {r.stderr!r}')


# ---------------------------------------------------------------------------------------------------------------------
# voice-chat, the whole loop with stand-ins

FAKE_ARECORD = '''#!/bin/sh
# the last argument is the file to write; stops on SIGTERM as arecord does
for a; do out=$a; done
echo "$*" >> "$FAKE_LOG/arecord.args"
# the file first, even when told to stop at once (a stop that comes during the copy is kept for after it)
trap 'term=1' TERM
cp "$FAKE_WAV" "$out"
[ -n "$term" ] && exit 0
trap 'exit 0' TERM
while :; do sleep 0.05; done
'''

FAKE_APLAY = '''#!/bin/sh
echo "$*" >> "$FAKE_LOG/aplay.args"
n=$(ls "$FAKE_LOG" | grep -c '^played-')
cat > "$FAKE_LOG/played-$n.wav"
if [ -n "$FAKE_APLAY_FAIL" ]; then echo "aplay: device busy" >&2; exit 1; fi
'''

FAKE_WHISPER = '''#!/bin/sh
# the next transcript of the list, one per call
n=$(cat "$FAKE_LOG/stt.count" 2>/dev/null || echo 0)
echo $((n + 1)) > "$FAKE_LOG/stt.count"
sed -n "$((n + 1))p" "$FAKE_TRANSCRIPTS"
'''

FAKE_SARINA = '''#!/bin/sh
# the agent: logs its command line, answers from the text, writes a session file on chat
printf '%s\\0' "$@" >> "$FAKE_LOG/sarina.args"; printf '\\n' >> "$FAKE_LOG/sarina.args"
cmd=$1; shift
dir=
while [ $# -gt 0 ]; do
  case $1 in --session-dir) dir=$2; shift 2;; --cwd) shift 2;; --) shift; break;; -*) shift;; *) break;; esac
done
text=$(eval echo "\\${$#}")
case $cmd in
  memory-review) echo "memory_review=1 note"; exit 0;;
  chat) mkdir -p "$dir"; echo '{"messages":[]}' > "$dir/sess-new.json";;
esac
case $text in
  *fail*) echo "first line" >&2; echo "the agent failed: model offline" >&2; exit 3;;
  *silent*) exit 0;;
esac
printf 'You said: %s.\\r\\nI can help with that.\\n' "$text"
'''

STUB_WHISPER = '''
"""A stand-in faster_whisper for the differential: transcripts from $FAKE_TRANSCRIPTS, one per call."""
import os
from types import SimpleNamespace

class WhisperModel:
    def __init__(self, model_dir, device='cpu', compute_type='int8', local_files_only=True):
        self.args = (model_dir, device, compute_type, local_files_only)

    def transcribe(self, path, language=None):
        log = os.environ['FAKE_LOG']
        count = os.path.join(log, 'stt.count')
        n = int(open(count).read()) if os.path.exists(count) else 0
        open(count, 'w').write(str(n + 1) + '\\n')
        lines = open(os.environ['FAKE_TRANSCRIPTS'], encoding='utf-8').read().split('\\n')
        text = lines[n] if n < len(lines) else ''
        return iter([SimpleNamespace(text=text)]), SimpleNamespace(language='en', duration=1.0)
'''


def write_exe(p: Path, text: str) -> None:
    p.write_text(text)
    p.chmod(0o755)


MARKS = (b"Press Enter to record, or type 'quit' then Enter to end the session.\n", b'Recording... press Enter to stop.\n')


def typed(cmd: list[str], env: dict, cwd: Path, stdin: str) -> subprocess.CompletedProcess:
    """Runs cmd as a person at the keyboard would: each line typed once the program has asked for it (its prompt to record
    or to stop has appeared, then 0.3 s, so arecord has started before the Enter that stops it), then input closed.
    A line nothing asks for (the command line was refused) goes after 10 s; "<Ctrl-C>" sends SIGINT instead of a line."""
    env = dict(env, PYTHONUNBUFFERED='1')
    p = subprocess.Popen(cmd, env=env, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    out = bytearray()
    seen = threading.Condition()

    def reader():
        while True:
            chunk = p.stdout.read1(65536)
            with seen:
                if not chunk:
                    seen.notify_all()
                    return
                out.extend(chunk)
                seen.notify_all()

    def marks() -> int:
        return sum(bytes(out).count(m) for m in MARKS)

    rt = threading.Thread(target=reader)
    rt.start()
    err = bytearray()
    et = threading.Thread(target=lambda: err.extend(p.stderr.read()))
    et.start()
    try:
        for n, line in enumerate(stdin.splitlines(keepends=True), 1):
            with seen:
                seen.wait_for(lambda: marks() >= n or p.poll() is not None, timeout=10)
            time.sleep(0.3)
            if line == '<Ctrl-C>\n':
                p.send_signal(signal.SIGINT)
                continue
            p.stdin.write(line.encode())
            p.stdin.flush()
        with seen:
            seen.wait_for(lambda: p.poll() is not None, timeout=1)
        p.stdin.close()
    except (BrokenPipeError, OSError):
        pass
    p.wait(timeout=300)
    rt.join()
    et.join()
    return subprocess.CompletedProcess(cmd, p.returncode, bytes(out), bytes(err))


def test_voice(rust: str, voice_python: str, work: Path, same_ort: str) -> None:
    print('voice-chat')
    model = quiet_voice(work)
    tools = work / 'voice-tools'
    tools.mkdir(exist_ok=True)
    for name, text in (('arecord', FAKE_ARECORD), ('aplay', FAKE_APLAY), ('whisper-cli', FAKE_WHISPER), ('sarina', FAKE_SARINA)):
        write_exe(tools / name, text)
    stub = work / 'voice-stub'
    stub.mkdir(exist_ok=True)
    (stub / 'faster_whisper.py').write_text(STUB_WHISPER)
    stt_dir = work / 'stt-model'
    stt_dir.mkdir(exist_ok=True)
    (stt_dir / 'ggml-test.bin').write_bytes(b'stand-in')
    wav = work / 'utterance-in.wav'
    wav.write_bytes(b'RIFF$\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\x80>\x00\x00\x00}\x00\x00\x02\x00\x10\x00data\x00\x00\x00\x00')
    base = ['--stt-model-dir', str(stt_dir), '--tts-model', str(model), '--sarina-bin', str(tools / 'sarina')]
    cases = [
        ('one turn, then quit', base, '\n\nquit\n', ['open the vault']),
        ('two turns, then end of input', base, '\n\n\n\n', ['what time is it', 'and tomorrow']),
        ('quit at once', base, 'q\n', []),
        ('end of input at once', base, '', []),
        ('QUIT in capitals with spaces', base, '  QUIT \n', []),
        ('no memory review', base + ['--no-auto-memory-review'], '\n\nexit\n', ['hello']),
        ('resume', base + ['--resume-session-id', 'abc-123'], '\n\nstop\n', ['carry on']),
        ('resume id empty', base + ['--resume-session-id', ''], '\n\nstop\n', ['carry on']),
        ('the agent fails', base, '\n\n\n\nq\n', ['this will fail', 'and now fine']),
        ('the agent answers nothing', base, '\n\nq\n', ['be silent']),
        ('nothing heard', base, '\n\n\n\nq\n', ['', 'something']),
        ('agent args, cwd, session folder', base + ['--agent-arg=--allow-write', '--agent-arg', 'x', '--cwd', str(work), '--session-dir', str(work / 'vs//sess/')],
         '\n\nq\n', ['hi']),
        ('input and output devices', base + ['--input-device', 'hw:9', '--output-device', 'plughw:8', '--stt-device', 'cpu'], '\n\nq\n', ['hi']),
        ('playback fails', base, '\n\nq\n', ['hi']),
        ('a partial last line', base, '\n\nq', ['hi']),
        ('a short timer', base + ['--max-record-seconds', '0.2'], '\n\nq\n', ['timed']),
        ('several sentences spoken', base, '\n\nq\n', ['first. second! third?']),
        ('/exit spoken', base, '\n\n', ['/exit']),
        ('Ctrl-C at the prompt', base, '<Ctrl-C>\n', []),
        ('Ctrl-C while recording, after a turn', base, '\n\n\n<Ctrl-C>\n', ['hello']),
        ('missing STT folder', ['--stt-model-dir', str(work / 'nowhere//x/'), '--tts-model', str(model)], '', []),
        ('missing voice', ['--stt-model-dir', str(stt_dir), '--tts-model', str(work / 'none.onnx')], '', []),
        ('missing voice config', base + ['--tts-config', str(work / 'none.json')], '', []),
        ('arguments: none', [], '', []), ('arguments: -h', ['-h'], '', []),
        ('arguments: a bad choice', base + ['--stt-device', 'gpu'], '', []),
        ('arguments: not a number', base + ['--max-record-seconds', 'soon'], '', []),
        ('arguments: an option as a value', base + ['--agent-arg', '--allow-write'], '', []),
        ('arguments: ambiguous', base + ['--s', 'x'], '', []),
        ('arguments: unknown', base + ['--bogus'], '', []),
        ('arguments: flag with a value', base + ['--no-auto-memory-review=yes'], '', []),
    ]
    for name, args, stdin, transcripts in cases:
        results = []
        for side in ('py', 'rs'):
            log = work / f'voice-log-{side}'
            shutil.rmtree(log, ignore_errors=True)
            log.mkdir()
            (log / 'transcripts').write_text('\n'.join(transcripts) + '\n', encoding='utf-8')
            sess = work / 'vs'
            shutil.rmtree(sess, ignore_errors=True)
            shutil.rmtree(work / '.port_sessions', ignore_errors=True)
            env = dict(os.environ, PATH=f'{tools}:/usr/bin:/bin', FAKE_LOG=str(log), FAKE_WAV=str(wav),
                       FAKE_TRANSCRIPTS=str(log / 'transcripts'), TMPDIR=str(work))
            if name == 'playback fails':
                env['FAKE_APLAY_FAIL'] = '1'
            cwd = work / 'voice-cwd'
            shutil.rmtree(cwd, ignore_errors=True)
            cwd.mkdir()
            if side == 'py':
                env['PYTHONPATH'] = os.pathsep.join([str(stub), str(HELPERS_PY)])
                cmd = [voice_python, '-P', '-m', 'sarina_helpers', 'voice-chat', *args]
            else:
                env['ORT_DYLIB_PATH'] = same_ort
                cmd = [rust, 'voice-chat', *args]
            p = typed(cmd, env, cwd, stdin)
            played = [(log / f).read_bytes() for f in sorted(os.listdir(log)) if f.startswith('played-')]
            argv = (log / 'sarina.args').read_bytes().replace(str(work).encode(), b'{W}') if (log / 'sarina.args').exists() else b''
            rec = (log / 'arecord.args').read_text().replace(str(work), '{W}') if (log / 'arecord.args').exists() else ''
            rec = '\n'.join(' '.join('{TMP}/utterance.wav' if a.endswith('/utterance.wav') else a for a in l.split()) for l in rec.splitlines())
            aplay = (log / 'aplay.args').read_text() if (log / 'aplay.args').exists() else ''
            results.append((p.stdout, p.stderr, p.returncode, argv, rec, aplay, played))
        (po, pe, pc, pa, pr, pp, pw), (ro, re_, rc, ra, rr, rp, rw) = results
        detail = (f'  py {pc} {po[-800:]!r}\n     {pe[-400:]!r}\n     {pa!r}\n     {pr!r} {pp!r} {len(pw)} played\n'
                  f'  rs {rc} {ro[-800:]!r}\n     {re_[-400:]!r}\n     {ra!r}\n     {rr!r} {rp!r} {len(rw)} played')
        check('voice', name, (po, pc, pa, pr, pp, pw) == (ro, rc, ra, rr, rp, rw) and (pe == re_), detail)
    # the tools missing: the Python named its own pieces (faster-whisper, piper-tts), the Rust names its own; both exit 1
    env = dict(os.environ, PATH='/nonexistent')
    p = py_helper(voice_python, ['voice-chat', *base], extra_path=[str(stub)], env=env, timeout=60)
    r = rs_helper(rust, ['voice-chat', *base], same_ort, env=env, timeout=60)
    check('voice', 'tools missing: one message, exit 1 (wording differs by design)',
          p.returncode == r.returncode == 1 and r.stdout.startswith(b'missing alsa-utils (arecord/aplay), whisper-cpp (whisper-cli)'),
          f'  py {p.stdout!r}\n  rs {r.stdout!r}')


def test_stt(parity: str, work: Path) -> None:
    print('stt (Rust only)')
    if not STT_MODEL.is_file() or not shutil.which('whisper-cli'):
        print('  skipped: no whisper-cli or no ggml model')
        return
    wav = work / 'stt.wav'
    subprocess.run([parity, 'speak', str(VOICE), str(wav), 'Please open the vault and show me the daily note.'], check=True)
    r = subprocess.run(['nice', '-n', '19', parity, 'transcribe', str(STT_MODEL), str(wav)], capture_output=True, text=True, timeout=300)
    heard = r.stdout.strip().lower()
    print(f'  heard: {r.stdout.strip()!r}')
    check('stt', 'whisper.cpp hears the sentence', all(w in heard for w in ('open', 'vault', 'daily', 'note')), r.stdout + r.stderr)


def main() -> int:
    ap = argparse.ArgumentParser()
    target = Path(os.environ.get('CARGO_TARGET_DIR', REPO / 'packaging/helpers/target')) / 'debug'
    ap.add_argument('--rust', default=str(target / 'sarina-helpers'))
    ap.add_argument('--parity', default=str(target / 'sarina-helpers-parity'))
    ap.add_argument('--ml-python', default=str(TTS_DIR / 'cb-venv/bin/python'), help='a Python with onnxruntime, tokenizers, numpy, onnx')
    ap.add_argument('--voice-python', default=str(TTS_DIR / 'venv/bin/python'), help='a Python with piper-tts')
    ap.add_argument('--only')
    ap.add_argument('--keep', action='store_true')
    args = ap.parse_args()
    work = Path(tempfile.mkdtemp(prefix='sarina-helpers-diff-', dir=os.environ.get('DIFF_TMP')))
    try:
        ml_ort = ort_lib(args.ml_python)
        voice_ort = ort_lib(args.voice_python)
        models = make_models(work, args.ml_python)
        if args.only in (None, 'serve'):
            test_serve(args.rust, args.ml_python, models, ml_ort)
        if args.only in (None, 'memory'):
            test_memory(args.rust, args.ml_python, models, ml_ort, work)
        if args.only in (None, 'tts'):
            test_tts(args.parity, args.voice_python, work, voice_ort)
        if args.only in (None, 'voice'):
            test_voice(args.rust, args.voice_python, work, voice_ort)
        if args.only in (None, 'stt'):
            test_stt(args.parity, work)
    finally:
        if not args.keep:
            shutil.rmtree(work, ignore_errors=True)
    print('cases:', ', '.join(f'{k} {v}' for k, v in counts.items()))
    if failures:
        print(f'{len(failures)} differences', file=sys.stderr)
        return 1
    print('no difference')
    return 0


if __name__ == '__main__':
    sys.exit(main())
