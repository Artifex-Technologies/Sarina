"""Offline tests for embed_group, classify, memory_search, serve and voice_bridge.

Models, audio and the Go binary are all fakes: no onnxruntime, no ALSA, no network.
"""

from __future__ import annotations

import io
import json
import subprocess
from dataclasses import dataclass
from pathlib import Path

import pytest

import sarina_helpers
from sarina_helpers import classify, embed_group, memory_search, serve, voice_bridge
from sarina_helpers.audio_io import RecordedAudio, VoiceUnavailable


class BagModel:
    """Bag-of-words embedder over a fixed vocabulary."""

    def __init__(self, vocab):
        self.vocab = list(vocab)
        self.embed_calls = 0

    def embed(self, texts):
        self.embed_calls += 1
        out = []
        for t in texts:
            words = t.lower().replace(',', ' ').split()
            out.append([float(words.count(v)) for v in self.vocab])
        return out

    def embed_one(self, text):
        return self.embed([text])[0]


def test_import_never_fails_and_flags_exist():
    assert isinstance(sarina_helpers.ML_AVAILABLE, bool)
    assert isinstance(sarina_helpers.VOICE_AVAILABLE, bool)


# ---- embed_group

def _cap(name, **meta):
    return {'name': name, 'type': 'nfc', 'meta': json.dumps(meta)}


def test_capture_text_uses_meta_fields_and_survives_bad_meta():
    assert embed_group.capture_text(_cap('a.nfc', uid='AA', ignored=1)) == 'a.nfc nfc uid=AA'
    assert embed_group.capture_text({'name': 'x', 'type': 't', 'meta': 'not json'}) == 'x t'
    assert embed_group.capture_text({'name': 'x', 'meta': '[1]'}) == 'x'


def test_group_captures_greedy_and_only_multi_member_groups():
    model = BagModel(['alpha', 'beta'])
    caps = [{'name': 'one', 'type': 'alpha'}, {'name': 'two', 'type': 'alpha'},
            {'name': 'three', 'type': 'beta'}, {'name': 'solo', 'type': ''}]
    # 'one alpha' and 'two alpha' share only 'alpha'; names are not in the vocab
    assert embed_group.group_captures(caps, model, 0.9) == [['one', 'two']]
    assert embed_group.group_captures([], model, 0.9) == []


def test_group_vectors_first_member_anchors_group():
    vectors = [[1, 0], [0.9, 0.1], [0.6, 0.8]]
    assert embed_group.group_vectors(['a', 'b', 'c'], vectors, 0.95) == [['a', 'b']]


# ---- classify

def test_fingerprint_matches_documented_serialisation_and_is_stable():
    import hashlib
    text = '\n'.join(f'{";".join(c)}|{s}' for c, s in classify.CATEGORY_PROTOTYPES)
    assert classify.prototypes_fingerprint() == hashlib.sha256(text.encode()).hexdigest()
    assert len(classify.CATEGORY_PROTOTYPES) == 13


def test_classifier_picks_nearest_prototype_and_handles_blank():
    vocab = ['drawing', 'image', 'cracking', 'password', 'audio', 'music']
    c = classify.Classifier(BagModel(vocab))
    label, score = c.classify('an image drawing program')
    assert label == 'Graphics' and score > 0
    assert c.classify('   ') == (None, 0.0)
    label, _ = c.classify('password cracking tool')
    assert label == 'System;Security'


# ---- memory_search

def _write_session(directory: Path, sid: str, messages):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f'{sid}.json').write_text(json.dumps({'session_id': sid, 'messages': messages}), encoding='utf-8')


def test_excerpt_strips_system_reminders_and_skips_pure_reminder_turn():
    messages = [
        {'role': 'system', 'content': 'You are helpful.'},
        {'role': 'user', 'content': '<system-reminder>\nuser_context_0\n</system-reminder>'},
        {'role': 'user', 'content': '<system-reminder>x</system-reminder>fix the login bug'},
        {'role': 'assistant', 'content': 'first'},
        {'role': 'tool', 'content': 'output'},
        {'role': 'assistant', 'content': 'fixed in auth.py'},
    ]
    ex = memory_search.excerpt_from_messages('s1', messages)
    # the first user message is the pure reminder: stripped to empty, so the next one is the task
    assert ex.task == 'fix the login bug'
    assert ex.outcome == 'fixed in auth.py'


def test_excerpt_non_string_content_does_not_crash():
    ex = memory_search.excerpt_from_messages('s', [{'role': 'user', 'content': ['a']}, {'role': 'assistant', 'content': None}, 'junk'])
    assert ex.task == '' and ex.outcome == ''


def test_search_ranks_go_session_files_and_skips_bad_ones(tmp_path):
    _write_session(tmp_path, 'a', [{'role': 'user', 'content': 'bake sourdough bread'}, {'role': 'assistant', 'content': 'done'}])
    _write_session(tmp_path, 'b', [{'role': 'user', 'content': 'fix python bug'}, {'role': 'assistant', 'content': 'patched'}])
    (tmp_path / 'broken.json').write_text('{not json', encoding='utf-8')
    (tmp_path / 'nomsgs.json').write_text('{"session_id": "z"}', encoding='utf-8')
    model = BagModel(['bread', 'sourdough', 'python', 'bug'])
    results = memory_search.search_sessions('python bug', model, tmp_path, top_k=1)
    assert [r[0].session_id for r in results] == ['b']
    text = memory_search.format_results(results)
    assert text.splitlines()[0].endswith('  b') and '  task:    fix python bug' in text


def test_search_empty_or_missing_directory(tmp_path):
    assert memory_search.search_sessions('q', BagModel(['q']), tmp_path / 'nope') == []


# ---- serve (the protocol)

def _run_serve(lines, model=None):
    out = io.StringIO()
    code = serve.serve(lambda: model or BagModel(['image']), io.StringIO('\n'.join(lines) + '\n'), out)
    return code, [json.loads(l) for l in out.getvalue().splitlines()]


def test_serve_ping_embed_classify_group_and_ids():
    code, resp = _run_serve([
        '{"op":"ping","id":1}',
        '{"op":"embed","texts":["image","x"]}',
        '{"op":"classify","text":"image drawing"}',
        '{"op":"group","captures":[{"name":"a","type":"image"},{"name":"b","type":"image"}],"threshold":0.5}',
    ])
    assert code == 0
    assert resp[0] == {'ok': True, 'protocol': 1, 'prototypes': classify.prototypes_fingerprint(), 'id': 1}
    assert resp[1]['vectors'] == [[1.0], [0.0]]
    assert resp[2]['ok'] and 'label' in resp[2] and 'score' in resp[2]
    assert resp[3] == {'ok': True, 'groups': [['a', 'b']]}


def test_serve_errors_do_not_kill_the_loop():
    code, resp = _run_serve(['not json', '[1]', '{"op":"nope"}', '{"op":"embed","texts":3}', '', '{"op":"ping"}'])
    assert code == 0
    assert [r['ok'] for r in resp] == [False, False, False, False, True]


def test_serve_model_load_failure_is_one_error_line():
    def boom():
        raise RuntimeError('missing model.onnx')

    out = io.StringIO()
    assert serve.serve(boom, io.StringIO(''), out) == 1
    assert json.loads(out.getvalue()) == {'ok': False, 'error': 'missing model.onnx'}


# ---- voice_bridge

@dataclass
class FakeTranscript:
    text: str


class FakeSTT:
    def __init__(self, *texts):
        self.texts = list(texts)

    def transcribe(self, wav_bytes, *, language=None):
        return FakeTranscript(self.texts.pop(0))


class FakeTTS:
    def __init__(self, fail=False):
        self.fail, self.spoken = fail, []

    def synthesize(self, text):
        if self.fail:
            raise VoiceUnavailable('no audio device')
        self.spoken.append(text)
        return b'RIFF'


def _record(*, input_func, output_func, device=None, max_seconds=120.0):
    input_func('')
    return RecordedAudio(b'RIFF', 16000, 1.0)


class FakeSarina:
    """Stands in for the Go binary: records argv, writes a session file on `chat`."""

    def __init__(self, session_dir, replies):
        self.session_dir, self.replies, self.calls = session_dir, list(replies), []

    def __call__(self, argv, **kwargs):
        self.calls.append(argv)
        if argv[1] == 'chat':
            _write_session(self.session_dir, 'sess-1', [])
        if argv[1] == 'memory-review':
            return subprocess.CompletedProcess(argv, 0, 'memory_review: proposed 1 entry', '')
        code, reply = self.replies.pop(0)
        return subprocess.CompletedProcess(argv, code, reply, 'boom\nlast line' if code else '')


def _turns(tmp_path, replies):
    fake = FakeSarina(tmp_path / 'agent', replies)
    return voice_bridge.AgentTurns('sarina', cwd='.', session_dir=str(tmp_path / 'agent'), agent_args=['--allow-write'], run=fake), fake


def test_input_func_quit_word_and_transcription():
    stt = FakeSTT('hello there', '')
    heard = []
    typed = iter(['', '', 'QUIT'])  # enter-to-record, enter-to-stop, then a quit word on the next prompt
    f = voice_bridge.make_input_func(stt, record_func=_record, read_line=lambda p: next(typed), echo=heard.append)
    assert f('user> ') == 'hello there'
    assert '(heard: hello there)' in heard
    assert f('user> ') == '/exit'


def test_first_turn_chats_then_resumes_with_found_session_id(tmp_path):
    turns, fake = _turns(tmp_path, [(0, 'hi back'), (0, 'second')])
    inputs = iter(['hello', 'again', '/exit'])
    tts, lines = FakeTTS(), []
    code = voice_bridge.run_loop(turns, lambda p: next(inputs), tts, play_func=lambda b, device=None: None, echo=lines.append)
    assert code == 0
    assert fake.calls[0][:2] == ['sarina', 'chat'] and fake.calls[0][-2:] == ['--', 'hello']
    assert fake.calls[1][:2] == ['sarina', 'resume'] and fake.calls[1][-2:] == ['sess-1', 'again']
    assert all('--no-auto-memory-review' in c and '--allow-write' in c for c in fake.calls[:2])
    assert tts.spoken == ['hi back', 'second']
    assert fake.calls[2][1] == 'memory-review' and fake.calls[2][-1] == 'sess-1'
    assert 'chat_ended=user_exit' in lines and 'memory_review: proposed 1 entry' in lines


def test_failed_turn_is_reported_not_spoken_and_review_can_be_disabled(tmp_path):
    turns, fake = _turns(tmp_path, [(1, '')])
    inputs = iter(['hello', '/quit'])
    tts, lines = FakeTTS(), []
    voice_bridge.run_loop(turns, lambda p: next(inputs), tts, auto_memory_review=False,
                          play_func=lambda b, device=None: None, echo=lines.append)
    assert tts.spoken == []
    assert any('agent turn failed, exit 1: last line' in l for l in lines)
    assert len(fake.calls) == 1


def test_playback_failure_does_not_abort(tmp_path):
    turns, _ = _turns(tmp_path, [(0, 'ok')])
    inputs = iter(['hello', '/exit'])
    lines = []
    voice_bridge.run_loop(turns, lambda p: next(inputs), FakeTTS(fail=True), auto_memory_review=False, echo=lines.append)
    assert any(l.startswith('(voice playback failed:') for l in lines)


def test_eof_blank_input_and_interrupt(tmp_path):
    turns, fake = _turns(tmp_path, [])
    seq = iter(['', '  '])

    def inp(p):
        try:
            return next(seq)
        except StopIteration:
            raise EOFError

    lines = []
    assert voice_bridge.run_loop(turns, inp, FakeTTS(), echo=lines.append) == 0
    assert 'chat_ended=eof' in lines and fake.calls == []

    def ctrl_c(p):
        raise KeyboardInterrupt

    lines = []
    assert voice_bridge.run_loop(turns, ctrl_c, FakeTTS(), echo=lines.append) == 130
    assert 'chat_ended=interrupt' in lines


def test_resume_session_id_skips_chat(tmp_path):
    fake = FakeSarina(tmp_path / 'agent', [(0, 'r')])
    turns = voice_bridge.AgentTurns('sarina', session_dir=str(tmp_path / 'agent'), session_id='old', run=fake)
    turns.send('x')
    assert fake.calls[0][1] == 'resume' and 'old' in fake.calls[0]


def test_main_reports_missing_voice_pieces(monkeypatch, capsys):
    monkeypatch.setattr('sarina_helpers.audio_io.ALSA_AVAILABLE', False)
    assert voice_bridge.main(['--stt-model-dir', 'x', '--tts-model', 'y']) == 1
    assert 'missing' in capsys.readouterr().out
