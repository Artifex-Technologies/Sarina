"""Build a synthetic training corpus for fine-tuning a cloned Piper voice.

Pipeline: for each sentence in sentences.txt, synthesize base audio with
Piper's own stock voice (reusing sarina.voice.tts.TextToSpeech), then run
OpenVoice's tone color conversion to reshape it toward the reference
speaker(s) in samples/. Writes corpus/wavs/<NNNN>.wav and
corpus/metadata.csv in the `filename.wav|text` format Piper's own trainer
expects (see README.md).

This script is NOT part of sarina's installed package and needs its own
environment (see README.md) -- it has no "degrade gracefully" mode, since it
cannot do anything useful without OpenVoice and a Piper voice model. Real
third-party imports (torch, openvoice) are kept inside the functions that
need them, so this module -- and its pure glue logic (load_sentences,
extract_target_embedding, build_corpus) -- stays importable and testable
without either installed.
"""

from __future__ import annotations

import argparse
import csv
import tempfile
from pathlib import Path
from typing import Callable, Sequence

_SCRIPT_DIR = Path(__file__).parent


def load_sentences(path: Path) -> list[str]:
    """Read one sentence per line, skipping blanks and '#' comments."""
    sentences = []
    for raw_line in path.read_text(encoding='utf-8').splitlines():
        line = raw_line.strip()
        if not line or line.startswith('#'):
            continue
        sentences.append(line)
    return sentences


def _torch_average(embeddings):
    import torch

    return torch.stack(list(embeddings)).mean(dim=0)


def extract_target_embedding(
    sample_paths: Sequence[Path],
    converter,
    *,
    get_se: Callable[..., tuple[object, str]],
    average: Callable[[Sequence[object]], object] = _torch_average,
):
    """Average the target speaker embedding across every reference sample -
    more robust than trusting a single recording. `get_se` and `average` are
    injected so this can be tested without OpenVoice or torch installed.
    """
    embeddings = [get_se(str(path), converter, vad=True)[0] for path in sample_paths]
    if not embeddings:
        raise ValueError('no reference samples given')
    return average(embeddings)


def build_corpus(
    sentences: Sequence[str],
    *,
    tts,
    converter,
    source_se,
    target_se,
    output_dir: Path,
) -> Path:
    """Synthesize each sentence with `tts` (Piper's stock voice), tone-convert
    it toward `target_se`, and write the result plus a metadata.csv line.

    `tts` needs only a `.synthesize(text) -> bytes` method (matching
    sarina.voice.tts.TextToSpeech) and `converter` only a `.convert(...)`
    method (matching openvoice.api.ToneColorConverter) - both are injected
    so this function can be tested without loading a real model.
    """
    wavs_dir = output_dir / 'wavs'
    wavs_dir.mkdir(parents=True, exist_ok=True)
    metadata_path = output_dir / 'metadata.csv'

    with tempfile.TemporaryDirectory(prefix='sarina-voice-clone-') as tmp_dir:
        base_wav_path = Path(tmp_dir) / 'base.wav'
        with metadata_path.open('w', encoding='utf-8', newline='') as metadata_file:
            writer = csv.writer(metadata_file, delimiter='|')
            for index, sentence in enumerate(sentences):
                base_wav_path.write_bytes(tts.synthesize(sentence))
                cloned_name = f'{index:04d}.wav'
                converter.convert(
                    audio_src_path=str(base_wav_path),
                    src_se=source_se,
                    tgt_se=target_se,
                    output_path=str(wavs_dir / cloned_name),
                    message='@sarina-voice-clone',
                )
                writer.writerow([cloned_name, sentence])

    return metadata_path


def _load_real_dependencies(args: argparse.Namespace):
    """Construct the real Piper TTS + OpenVoice tone color converter objects.
    Kept separate from build_corpus()/extract_target_embedding() so tests
    never need to import torch or openvoice.
    """
    import torch
    from openvoice import se_extractor
    from openvoice.api import ToneColorConverter

    from sarina.voice.tts import TextToSpeech

    device = 'cuda' if torch.cuda.is_available() else 'cpu'
    converter = ToneColorConverter(str(args.converter_config), device=device)
    converter.load_ckpt(str(args.converter_checkpoint))

    tts = TextToSpeech(args.piper_model, config_path=args.piper_config)

    # Source embedding: Piper's own stock voice, measured from a clip that
    # same voice just synthesized -- not a human reference. Needs enough
    # detected speech for OpenVoice's VAD splitter to form at least one
    # ~10-second segment (a single short sentence hits its "input audio is
    # too short" assertion), so this reads a short paragraph, not one line.
    reference_text = (
        "This is a reference clip used only to measure this voice's own tone color. "
        "It needs to be long enough for the voice activity detector to split it into "
        "segments. A few sentences of natural speech are enough for that purpose."
    )
    with tempfile.TemporaryDirectory(prefix='sarina-voice-clone-') as tmp_dir:
        reference_wav = Path(tmp_dir) / 'piper_reference.wav'
        reference_wav.write_bytes(tts.synthesize(reference_text))
        source_se, _ = se_extractor.get_se(str(reference_wav), converter, vad=True)

    target_se = extract_target_embedding(args.samples, converter, get_se=se_extractor.get_se)
    return tts, converter, source_se, target_se


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--sentences', type=Path, default=_SCRIPT_DIR / 'sentences.txt')
    parser.add_argument(
        '--samples', type=Path, nargs='+',
        default=sorted((_SCRIPT_DIR / 'samples').glob('*')),
        help='reference voice clip(s) to clone; averaged if more than one',
    )
    parser.add_argument('--piper-model', type=Path, required=True, help='stock Piper voice .onnx used as the base speaker')
    parser.add_argument('--piper-config', type=Path, help="Piper voice config; omit to use Piper's own discovery")
    parser.add_argument('--converter-config', type=Path, required=True, help='OpenVoice checkpoints_v2/converter/config.json')
    parser.add_argument('--converter-checkpoint', type=Path, required=True, help='OpenVoice checkpoints_v2/converter/checkpoint.pth')
    parser.add_argument('--output-dir', type=Path, default=_SCRIPT_DIR / 'corpus')
    args = parser.parse_args(argv)

    sentences = load_sentences(args.sentences)
    tts, converter, source_se, target_se = _load_real_dependencies(args)
    metadata_path = build_corpus(
        sentences, tts=tts, converter=converter, source_se=source_se, target_se=target_se,
        output_dir=args.output_dir,
    )
    print(f'Wrote {len(sentences)} sentences to {metadata_path}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
