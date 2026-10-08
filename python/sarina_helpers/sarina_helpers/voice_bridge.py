"""Push-to-talk voice chat that drives the Go agent one turn at a time.

The bridge owns only audio: record, transcribe, speak. Each turn it runs the Go
binary as a subprocess and speaks its stdout:

* first turn:  ``sarina chat [flags] -- <text>``
* later turns: ``sarina resume [flags] <session_id> <text>``

``chat`` does not print its session id, so the bridge finds it as the session
file in the session directory that appeared or changed during that call.
Each subprocess runs with ``--no-auto-memory-review``; unless the operator
passed that flag to the bridge, one ``sarina memory-review <id>`` runs when the
session ends. See PROTOCOL.md.
"""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Callable, Sequence

from .audio_io import RecordedAudio, VoiceUnavailable, play_audio, record_push_to_talk

QUIT_WORDS = {'q', 'quit', 'exit', 'stop'}


class AgentTurns:
    """Runs agent turns through the Go binary and tracks the session id."""

    def __init__(
        self, sarina_bin: str, *, cwd: str = '.', session_dir: str | None = None,
        agent_args: Sequence[str] = (), session_id: str | None = None,
        run: Callable[..., subprocess.CompletedProcess] = subprocess.run,
    ):
        self.sarina_bin = sarina_bin
        self.cwd = cwd
        self.session_dir = Path(session_dir) if session_dir else Path(cwd) / '.port_sessions' / 'agent'
        self.agent_args = list(agent_args)
        self.session_id = session_id
        self._run = run

    def _flags(self) -> list[str]:
        return ['--cwd', self.cwd, '--session-dir', str(self.session_dir),
                '--no-auto-memory-review', *self.agent_args]

    def _snapshot(self) -> dict[str, int]:
        if not self.session_dir.is_dir():
            return {}
        return {p.stem: p.stat().st_mtime_ns for p in self.session_dir.glob('*.json')}

    def _exec(self, argv: list[str]) -> subprocess.CompletedProcess:
        return self._run(argv, capture_output=True, text=True, encoding='utf-8', errors='replace')

    def send(self, text: str) -> tuple[str, int]:
        """Run one turn. Returns (reply text, exit code)."""
        before = self._snapshot()
        if self.session_id:
            argv = [self.sarina_bin, 'resume', *self._flags(), self.session_id, text]
        else:
            argv = [self.sarina_bin, 'chat', *self._flags(), '--', text]
        proc = self._exec(argv)
        if not self.session_id:
            after = self._snapshot()
            changed = [sid for sid, mtime in after.items() if before.get(sid) != mtime]
            if changed:
                self.session_id = max(changed, key=lambda sid: after[sid])
        reply = (proc.stdout or '').strip()
        if proc.returncode != 0 and not reply:
            tail = (proc.stderr or '').strip().splitlines()[-1:] or ['no output']
            reply = f'(agent turn failed, exit {proc.returncode}: {tail[0]})'
        return reply, proc.returncode

    def review_memory(self) -> subprocess.CompletedProcess | None:
        if not self.session_id:
            return None
        return self._exec([self.sarina_bin, 'memory-review', '--cwd', self.cwd,
                           '--session-dir', str(self.session_dir), self.session_id])


def make_input_func(
    stt, *, device: str | None = None, max_record_seconds: float = 120.0,
    record_func: Callable[..., RecordedAudio] = record_push_to_talk,
    read_line: Callable[[str], str] = input, echo: Callable[[str], None] = print,
) -> Callable[[str], str]:
    """Record, transcribe, return text; a quit word instead of recording returns ``/exit``."""

    def _input(prompt: str) -> str:
        echo(prompt)
        echo("Press Enter to record, or type 'quit' then Enter to end the session.")
        if read_line('').strip().lower() in QUIT_WORDS:
            return '/exit'
        recorded = record_func(input_func=read_line, output_func=echo, device=device,
                               max_seconds=max_record_seconds)
        transcript = stt.transcribe(recorded.wav_bytes)
        echo(f'(heard: {transcript.text})' if transcript.text else '(heard nothing)')
        return transcript.text

    return _input


def run_loop(
    turns: AgentTurns, input_func: Callable[[str], str], tts, *,
    device: str | None = None, auto_memory_review: bool = True,
    play_func: Callable[..., None] = play_audio, echo: Callable[[str], None] = print,
) -> int:
    """The conversation loop. Returns the process exit code (130 on Ctrl-C at the prompt)."""
    echo('# Voice Chat')
    echo('Say your request, or type /exit to stop.')
    if turns.session_id:
        echo(f'resuming_session_id={turns.session_id}')
    code = 0
    while True:
        try:
            text = input_func('user> ').strip()
        except EOFError:
            echo('chat_ended=eof')
            break
        except KeyboardInterrupt:
            echo('')
            echo('chat_ended=interrupt')
            code = 130
            break
        if not text:
            continue
        if text in ('/exit', '/quit'):
            echo('chat_ended=user_exit')
            break
        reply, rc = turns.send(text)
        echo(reply)
        if rc != 0 or not reply:
            continue
        try:
            play_func(tts.synthesize(reply), device=device)
        except (VoiceUnavailable, OSError, RuntimeError) as exc:
            echo(f'(voice playback failed: {exc})')
    if auto_memory_review and turns.session_id:
        try:
            proc = turns.review_memory()
            out = (proc.stdout or '').strip() if proc else ''
            if out:
                echo(out)
        except Exception as exc:  # runs after the user left; never changes the exit code
            echo(f'memory_review_skipped=error ({exc})')
    return code


def main(argv: Sequence[str]) -> int:
    import argparse

    from .audio_io import ALSA_AVAILABLE
    from .stt import STT_AVAILABLE, SpeechToText
    from .tts import TTS_AVAILABLE, TextToSpeech

    p = argparse.ArgumentParser(prog='sarina voice-chat')
    p.add_argument('--stt-model-dir', required=True)
    p.add_argument('--tts-model', required=True)
    p.add_argument('--tts-config')
    p.add_argument('--stt-device', default='cpu', choices=['cpu', 'cuda'])
    p.add_argument('--stt-compute-type', default='int8')
    p.add_argument('--input-device')
    p.add_argument('--output-device')
    p.add_argument('--max-record-seconds', type=float, default=120.0)
    p.add_argument('--resume-session-id')
    p.add_argument('--no-auto-memory-review', action='store_true')
    p.add_argument('--cwd', default='.')
    p.add_argument('--session-dir')
    p.add_argument('--sarina-bin', default='sarina', help='the Go binary to run per turn')
    p.add_argument('--agent-arg', action='append', default=[],
                   help='extra flag passed to every chat/resume call (repeatable), e.g. --agent-arg=--allow-write')
    args = p.parse_args(list(argv))

    missing = [name for ok, name in ((ALSA_AVAILABLE, 'alsa-utils (arecord/aplay)'),
                                     (STT_AVAILABLE, 'faster-whisper'),
                                     (TTS_AVAILABLE, 'piper-tts')) if not ok]
    if missing:
        print(f'missing {", ".join(missing)} - install alsa-utils via your package manager and the rest with '
              'pip install "sarina-helpers[voice]"')
        return 1
    try:
        stt = SpeechToText(args.stt_model_dir, device=args.stt_device, compute_type=args.stt_compute_type)
        tts = TextToSpeech(args.tts_model, args.tts_config)
    except VoiceUnavailable as exc:
        print(str(exc))
        return 1
    turns = AgentTurns(args.sarina_bin, cwd=args.cwd, session_dir=args.session_dir,
                       agent_args=args.agent_arg, session_id=args.resume_session_id)
    input_func = make_input_func(stt, device=args.input_device, max_record_seconds=args.max_record_seconds)
    return run_loop(turns, input_func, tts, device=args.output_device,
                    auto_memory_review=not args.no_auto_memory_review)
