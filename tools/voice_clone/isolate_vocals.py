"""Isolate vocals from background music in a reference recording, before
adding it to samples/.

VITS-style models (Piper included) learn to reproduce whatever's actually in
their training audio, not just the voice in it -- faint background music
becomes part of what the trained voice "sounds like", and can also throw off
faster-whisper's transcription accuracy in transcribe_samples.py. Only
needed for source audio with detectable background music; clean recordings
don't need this step.

Uses Demucs (Meta, MIT-licensed, pip install demucs) via its own CLI in
--two-stems=vocals mode, which is built for exactly this and much faster
than full 4-stem (vocals/drums/bass/other) separation since it only needs to
produce two tracks.

Usage:
    python isolate_vocals.py path/to/noisy_recording.mp3 --output samples/Sample06.wav
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import tempfile
from pathlib import Path


def isolate_vocals(input_path: Path, output_path: Path, *, model: str = 'htdemucs') -> Path:
    """Run Demucs' two-stem vocal separation and copy the resulting
    vocals-only track to `output_path`.

    Raises RuntimeError (naming what's missing, or Demucs' own stderr on
    failure) rather than a raw subprocess error -- same convention as
    sarina/chymaera/firmware.py's `_git` helper.
    """
    demucs = shutil.which('demucs')
    if not demucs:
        raise RuntimeError("demucs not found on PATH - install with 'pip install demucs'")

    with tempfile.TemporaryDirectory(prefix='demucs-out-') as tmp_dir:
        proc = subprocess.run(
            [demucs, '--two-stems=vocals', '-n', model, '-o', tmp_dir, str(input_path)],
            capture_output=True, text=True,
        )
        if proc.returncode != 0:
            raise RuntimeError(f'demucs failed: {proc.stderr.strip()}')

        # Demucs writes <out_dir>/<model>/<input_stem>/vocals.wav.
        vocals_path = Path(tmp_dir) / model / input_path.stem / 'vocals.wav'
        if not vocals_path.is_file():
            raise RuntimeError(f'demucs did not produce the expected output at {vocals_path}')

        output_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(vocals_path, output_path)

    return output_path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path, help='recording with background music to clean up')
    parser.add_argument('--output', type=Path, required=True, help='where to write the vocals-only result')
    parser.add_argument('--model', default='htdemucs', help='Demucs model name (default: htdemucs)')
    args = parser.parse_args(argv)

    try:
        output_path = isolate_vocals(args.input, args.output, model=args.model)
    except RuntimeError as exc:
        print(exc)
        return 1
    print(f'wrote {output_path}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
