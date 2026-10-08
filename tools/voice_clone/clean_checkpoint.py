"""Strip a legacy rhasspy/piper training checkpoint down to the
hyperparameters the newer piper1-gpl `VitsModel` actually accepts.

Checkpoints published on `rhasspy/piper-checkpoints` (Hugging Face) predate
piper1-gpl's rewrite and store one big flat hyperparameter dict mixing model
architecture, old PyTorch Lightning Trainer args, and dataset-prep options
together -- many of which (e.g. `sample_bytes`) no longer exist as CLI
options at all. Lightning's CLI auto-applies every stored hyperparameter as
a `--model.<key>` default when resuming from a checkpoint, and hard-errors
on the first one it doesn't recognize (`Subcommand 'fit' does not accept
option 'model.sample_bytes'`).

This only rewrites the `hyper_parameters` dict; `state_dict` (the actual
trained weights) is untouched.

Usage:
    python clean_checkpoint.py checkpoints/piper/amy-medium-base.ckpt \\
        checkpoints/piper/amy-medium-base.cleaned.ckpt
"""

from __future__ import annotations

import argparse
import inspect
import pathlib
from typing import Iterable

#: VitsModel.__init__'s own accepted keyword arguments, minus 'self'/'kwargs'.
#: Imports lazily (see _load_accepted_model_keys) so this module -- and
#: clean_hyperparameters(), the part worth testing -- stays importable
#: without torch/piper installed, matching build_corpus.py's convention.


def _load_accepted_model_keys() -> set[str]:
    from piper.train.vits.lightning import VitsModel

    return set(inspect.signature(VitsModel.__init__).parameters.keys()) - {'self', 'kwargs'}


def clean_hyperparameters(hyper_parameters: dict, *, accepted_keys: Iterable[str]) -> dict:
    """Keep only the keys ``accepted_keys`` (VitsModel's real parameters) names."""
    accepted = set(accepted_keys)
    return {key: value for key, value in hyper_parameters.items() if key in accepted}


def main(argv: list[str] | None = None) -> int:
    import pathlib as _pathlib

    import torch.serialization

    # Needed to load the checkpoint at all: see train.py's own docstring for
    # why a plain PosixPath, saved on Linux, can't just be read back on Windows.
    torch.serialization.add_safe_globals([(_pathlib.PurePosixPath, 'pathlib.PosixPath')])

    import torch

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('checkpoint', type=pathlib.Path)
    parser.add_argument('output', type=pathlib.Path)
    args = parser.parse_args(argv)

    ckpt = torch.load(str(args.checkpoint), map_location='cpu', weights_only=True)
    before = set(ckpt['hyper_parameters'].keys())
    ckpt['hyper_parameters'] = clean_hyperparameters(
        ckpt['hyper_parameters'], accepted_keys=_load_accepted_model_keys(),
    )
    dropped = before - set(ckpt['hyper_parameters'].keys())
    print(f'dropping {len(dropped)} obsolete keys: {sorted(dropped)}')

    torch.save(ckpt, str(args.output))
    print(f'wrote {args.output}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
